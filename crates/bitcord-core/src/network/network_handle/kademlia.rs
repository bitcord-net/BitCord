use std::collections::HashSet;
use std::sync::Arc;

use crate::{
    identity::NodeIdentity,
    network::client::NodeClient,
    network::node_addr::NodeAddr,
    node::dht::{Dht, NodeId},
};

/// Iterative Kademlia mailbox lookup.
///
/// Walks the DHT routing table using `FIND_NODE` RPCs to locate the node
/// holding the mailbox for `target_pk`.  Returns the `NodeAddr` of the
/// mailbox-holding node if found within `MAX_ROUNDS` rounds.
///
/// Uses α=3 parallel probes per round (standard Kademlia concurrency factor).
pub(crate) async fn kademlia_lookup(
    target_pk: &[u8; 32],
    dht: &Arc<Dht>,
    identity: Arc<NodeIdentity>,
) -> Option<NodeAddr> {
    const ALPHA: usize = 3;
    const MAX_ROUNDS: usize = 8;

    let target = NodeId(*target_pk);

    // Seed the candidate list with our K closest known peers.
    let mut to_visit: Vec<(NodeId, NodeAddr)> = dht.closest_peers(&target, ALPHA);
    if to_visit.is_empty() {
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

        // Probe each candidate in parallel.
        let mut handles = Vec::new();
        for (node_id, node_addr) in &batch {
            visited.insert(node_id.0);
            let addr = node_addr.clone();
            let id_clone = Arc::clone(&identity);
            let target_bytes = *target_pk;
            handles.push(tokio::spawn(async move {
                match NodeClient::connect(addr, [0u8; 32], id_clone).await {
                    Ok((client, _, _)) => client.find_node(target_bytes).await.ok(),
                    Err(_) => None,
                }
            }));
        }

        for handle in handles {
            if let Ok(Some((closer_peers, mailbox))) = handle.await {
                // If this node holds the mailbox, we're done.
                if let Some(addr) = mailbox {
                    // Cache the record locally for future lookups.
                    dht.add_mailbox_record(*target_pk, addr.clone());
                    return Some(addr);
                }
                // Merge newly discovered peers into the candidate list.
                for (peer_id_bytes, peer_addr) in closer_peers {
                    if !visited.contains(&peer_id_bytes) {
                        dht.add_peer(NodeId(peer_id_bytes), peer_addr.clone());
                        to_visit.push((NodeId(peer_id_bytes), peer_addr));
                    }
                }
            }
        }

        // Sort remaining candidates by distance to target for the next round.
        to_visit.sort_by_key(|(id, _)| target.distance(id));
        to_visit.dedup_by_key(|(id, _)| id.0);
    }

    None
}
