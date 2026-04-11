//! Iterative Kademlia lookups over QUIC.
//!
//! These functions perform multi-hop DHT walks using the node's identity for
//! QUIC connections (TOFU TLS — fingerprint `[0u8; 32]`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info};

use bitcord_dht::{CommunityPeerRecord, DhtState, NodeAddr, NodeId, PeerInfoRecord};

use crate::{identity::NodeIdentity, network::client::NodeClient};

/// Shared connection cache type for DHT-only QUIC connections.
pub(super) type DhtConnCache = Arc<RwLock<HashMap<NodeAddr, NodeClient>>>;

/// Return the cached `NodeClient` for `addr` if already connected, otherwise
/// open a fresh QUIC connection with TOFU certificate validation.
pub(super) async fn dht_connect(
    addr: &NodeAddr,
    conn_cache: &DhtConnCache,
    identity: Arc<NodeIdentity>,
) -> Option<NodeClient> {
    if let Some(c) = conn_cache.read().await.get(addr).cloned() {
        return Some(c);
    }
    match NodeClient::connect(addr.clone(), [0u8; 32], identity).await {
        Ok((c, _, _)) => {
            conn_cache.write().await.insert(addr.clone(), c.clone());
            Some(c)
        }
        Err(_) => None,
    }
}

/// Iterative Kademlia mailbox lookup.
///
/// Walks the DHT routing table using `FIND_NODE` RPCs to locate the node
/// holding the mailbox for `target_pk`.  Returns the `NodeAddr` of the
/// mailbox-holding node if found within `MAX_ROUNDS` rounds.
pub(super) async fn kademlia_lookup(
    target_pk: &[u8; 32],
    state: &Arc<DhtState>,
    identity: Arc<NodeIdentity>,
    conn_cache: DhtConnCache,
) -> Option<NodeAddr> {
    const ALPHA: usize = 3;
    const MAX_ROUNDS: usize = 8;
    const K_SEED: usize = 20;

    let target = NodeId(*target_pk);
    let mut to_visit: Vec<(NodeId, NodeAddr)> = state.closest_peers(&target, K_SEED);
    if to_visit.is_empty() {
        debug!("kademlia_lookup: routing table empty, cannot resolve mailbox");
        return None;
    }

    let mut visited: HashSet<[u8; 32]> = HashSet::new();

    for _ in 0..MAX_ROUNDS {
        let batch: Vec<(NodeId, NodeAddr)> = to_visit
            .drain(..to_visit.len().min(ALPHA))
            .filter(|(id, _)| !visited.contains(&id.0))
            .collect();

        if batch.is_empty() {
            break;
        }

        let mut handles = Vec::new();
        for (node_id, node_addr) in &batch {
            visited.insert(node_id.0);
            let addr = node_addr.clone();
            let id_clone = Arc::clone(&identity);
            let cache = Arc::clone(&conn_cache);
            let target_bytes = *target_pk;
            handles.push(tokio::spawn(async move {
                let client = dht_connect(&addr, &cache, id_clone).await?;
                client.find_node(target_bytes).await.ok()
            }));
        }

        for handle in handles {
            if let Ok(Some((closer_peers, mailbox))) = handle.await {
                if let Some(addr) = mailbox {
                    state.add_mailbox_record(*target_pk, addr.clone());
                    return Some(addr);
                }
                for (peer_id_bytes, peer_addr) in closer_peers {
                    if !visited.contains(&peer_id_bytes) {
                        state.add_peer(NodeId(peer_id_bytes), peer_addr.clone());
                        to_visit.push((NodeId(peer_id_bytes), peer_addr));
                    }
                }
            }
        }

        to_visit.sort_by_key(|(id, _)| target.distance(id));
        to_visit.dedup_by_key(|(id, _)| id.0);
    }

    None
}

/// Iterative Kademlia community peer lookup.
///
/// Two-phase: walk routing table with FIND_NODE, then query K closest nodes
/// with FindCommunityPeers.
pub(super) async fn kademlia_find_community_peers(
    community_pk: &[u8; 32],
    state: &Arc<DhtState>,
    identity: Arc<NodeIdentity>,
    conn_cache: DhtConnCache,
) -> Vec<CommunityPeerRecord> {
    const ALPHA: usize = 3;
    const MAX_ROUNDS: usize = 8;
    const K_QUERY: usize = 20;

    let target = NodeId(*community_pk);
    let community_pk_hex: String = community_pk.iter().map(|b| format!("{b:02x}")).collect();

    let mut to_visit: Vec<(NodeId, NodeAddr)> = state.closest_peers(&target, K_QUERY);

    info!(
        community_pk = %community_pk_hex,
        routing_table_seeds = to_visit.len(),
        local_cached = state.lookup_community_peers(community_pk).len(),
        "kademlia: starting community peer discovery"
    );

    let mut visited: HashSet<[u8; 32]> = HashSet::new();
    let mut closest_found: Vec<(NodeId, NodeAddr)> = Vec::new();

    if !to_visit.is_empty() {
        for _ in 0..MAX_ROUNDS {
            let batch: Vec<(NodeId, NodeAddr)> = to_visit
                .drain(..to_visit.len().min(ALPHA))
                .filter(|(id, _)| !visited.contains(&id.0))
                .collect();

            if batch.is_empty() {
                break;
            }

            let mut handles = Vec::new();
            for (node_id, node_addr) in &batch {
                visited.insert(node_id.0);
                closest_found.push((*node_id, node_addr.clone()));
                let addr = node_addr.clone();
                let id_clone = Arc::clone(&identity);
                let cache = conn_cache.clone();
                let target_bytes = *community_pk;
                handles.push(tokio::spawn(async move {
                    let client = dht_connect(&addr, &cache, id_clone).await?;
                    client.find_node(target_bytes).await.ok()
                }));
            }

            for handle in handles {
                if let Ok(Some((closer_peers, _))) = handle.await {
                    for (peer_id_bytes, peer_addr) in closer_peers {
                        if !visited.contains(&peer_id_bytes) {
                            state.add_peer(NodeId(peer_id_bytes), peer_addr.clone());
                            to_visit.push((NodeId(peer_id_bytes), peer_addr));
                        }
                    }
                }
            }

            to_visit.sort_by_key(|(id, _)| target.distance(id));
            to_visit.dedup_by_key(|(id, _)| id.0);
        }
    }

    // Phase 2: query closest nodes for community peer records.
    closest_found.sort_by_key(|(id, _)| target.distance(id));
    closest_found.dedup_by_key(|(id, _)| id.0);
    closest_found.truncate(K_QUERY);
    debug!(
        community_pk = %community_pk_hex,
        nodes_to_query = closest_found.len(),
        "kademlia: phase 1 complete"
    );

    let cached_records: Vec<CommunityPeerRecord> = state.lookup_community_peers(community_pk);
    let mut all_records: Vec<CommunityPeerRecord> = Vec::new();
    let mut seen_node_pks: HashSet<[u8; 32]> = HashSet::new();

    let mut handles = Vec::new();
    for (_, node_addr) in &closest_found {
        let addr = node_addr.clone();
        let id_clone = Arc::clone(&identity);
        let cache = Arc::clone(&conn_cache);
        let cpk = *community_pk;
        handles.push(tokio::spawn(async move {
            let client = dht_connect(&addr, &cache, id_clone).await?;
            client.find_community_peers(cpk).await.ok()
        }));
    }

    for handle in handles {
        if let Ok(Some(records)) = handle.await {
            for record in records {
                if seen_node_pks.insert(record.node_pk) {
                    all_records.push(record);
                }
            }
        }
    }

    for record in cached_records {
        if seen_node_pks.insert(record.node_pk) {
            all_records.push(record);
        }
    }

    info!(
        community_pk = %community_pk_hex,
        peers_found = all_records.len(),
        "kademlia: community peer discovery complete"
    );
    all_records
}

/// Iterative Kademlia peer info lookup.
///
/// Walks the DHT using FIND_NODE, then queries nodes closest to `peer_id`
/// with FindPeerInfo.  Returns the first `PeerInfoRecord` found.
pub(super) async fn kademlia_find_peer_info(
    peer_id: &[u8; 32],
    state: &Arc<DhtState>,
    identity: Arc<NodeIdentity>,
    conn_cache: DhtConnCache,
) -> Option<PeerInfoRecord> {
    const ALPHA: usize = 3;
    const MAX_ROUNDS: usize = 8;
    const K_SEED: usize = 20;

    let target = NodeId(*peer_id);
    let mut to_visit: Vec<(NodeId, NodeAddr)> = state.closest_peers(&target, K_SEED);
    if to_visit.is_empty() {
        debug!("kademlia_find_peer_info: routing table empty, cannot find peer");
        return None;
    }

    let mut visited: HashSet<[u8; 32]> = HashSet::new();
    let mut closest_found: Vec<(NodeId, NodeAddr)> = Vec::new();

    // Phase 1: FIND_NODE walk to get closest nodes.
    for _ in 0..MAX_ROUNDS {
        let batch: Vec<(NodeId, NodeAddr)> = to_visit
            .drain(..to_visit.len().min(ALPHA))
            .filter(|(id, _)| !visited.contains(&id.0))
            .collect();

        if batch.is_empty() {
            break;
        }

        let mut handles = Vec::new();
        for (node_id, node_addr) in &batch {
            visited.insert(node_id.0);
            closest_found.push((*node_id, node_addr.clone()));
            let addr = node_addr.clone();
            let id_clone = Arc::clone(&identity);
            let cache = conn_cache.clone();
            let target_bytes = *peer_id;
            handles.push(tokio::spawn(async move {
                let client = dht_connect(&addr, &cache, id_clone).await?;
                client.find_node(target_bytes).await.ok()
            }));
        }

        for handle in handles {
            if let Ok(Some((closer_peers, _))) = handle.await {
                for (peer_id_bytes, peer_addr) in closer_peers {
                    if !visited.contains(&peer_id_bytes) {
                        state.add_peer(NodeId(peer_id_bytes), peer_addr.clone());
                        to_visit.push((NodeId(peer_id_bytes), peer_addr));
                    }
                }
            }
        }

        to_visit.sort_by_key(|(id, _)| target.distance(id));
        to_visit.dedup_by_key(|(id, _)| id.0);
    }

    // Phase 2: query closest nodes with FindPeerInfo, returning on first hit.
    closest_found.sort_by_key(|(id, _)| target.distance(id));
    closest_found.dedup_by_key(|(id, _)| id.0);
    closest_found.truncate(K_SEED);

    let mut set = tokio::task::JoinSet::new();
    for (_, node_addr) in &closest_found {
        let addr = node_addr.clone();
        let id_clone = Arc::clone(&identity);
        let cache = Arc::clone(&conn_cache);
        let pid = *peer_id;
        set.spawn(async move {
            let client = dht_connect(&addr, &cache, id_clone).await?;
            client.find_peer_info(pid).await.ok().flatten()
        });
    }

    while let Some(result) = set.join_next().await {
        if let Ok(Some(record)) = result {
            info!("kademlia_find_peer_info: found peer info via DHT walk");
            return Some(record);
        }
    }

    info!("kademlia_find_peer_info: peer info not found after DHT walk");
    None
}
