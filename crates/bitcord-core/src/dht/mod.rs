//! DHT handle — clean interface for the rest of `bitcord-core`.
//!
//! `DhtHandle` wraps [`bitcord_dht::DhtState`] (in-memory routing table and
//! record store) with QUIC-based Kademlia iterative lookups.  All DHT
//! operations are accessed through this handle; no DHT logic leaks into the
//! gossip layer.
//!
//! # Public API
//! - [`DhtHandle::find_mailbox_peers`] — locate the node(s) holding a user's mailbox.
//! - [`DhtHandle::find_community_peers`] — discover members of a community.
//! - [`DhtHandle::register_mailbox`] — advertise that this node holds a user's mailbox.
//! - [`DhtHandle::register_community_peer`] — advertise membership in a community.
//! - [`DhtHandle::update_self_addr`] — set our publicly reachable address.
//! - [`DhtHandle::add_known_peer`] — seed the routing table with a known peer.
//! - [`DhtHandle::bootstrap`] — populate the routing table from bootstrap nodes.

mod kademlia;

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use bitcord_dht::{CommunityPeerRecord, DhtState, DhtStore, NodeAddr, NodeId, spawn_expiry_task};

use crate::identity::NodeIdentity;
use crate::network::client::NodeClient;

use kademlia::{DhtConnCache, kademlia_find_community_peers, kademlia_lookup};

// ── DhtConfig ─────────────────────────────────────────────────────────────────

pub struct DhtConfig {
    /// This node's Ed25519 public key (32 bytes).
    pub node_pk: [u8; 32],
    /// Optional initial self-address hint (set before STUN discovery).
    pub self_addr: Option<NodeAddr>,
    /// Path for the DHT persistence database.
    pub store_path: std::path::PathBuf,
    /// This node's identity (for authenticating outbound DHT QUIC connections).
    pub identity: Arc<NodeIdentity>,
}

// ── DhtHandle ─────────────────────────────────────────────────────────────────

struct DhtHandleInner {
    state: Arc<DhtState>,
    store: Arc<DhtStore>,
    identity: Arc<NodeIdentity>,
    /// Connection cache for DHT-only QUIC connections (separate from gossip).
    conn_cache: DhtConnCache,
}

/// Clean DHT interface — the only DHT object the rest of `bitcord-core` holds.
///
/// Arc-wrapped; cheaply cloneable.
#[derive(Clone)]
pub struct DhtHandle(Arc<DhtHandleInner>);

impl DhtHandle {
    /// Create and start a DHT handle.
    ///
    /// Opens the persistent store, pre-populates the in-memory routing table
    /// from disk, and spawns background expiry tasks.
    pub async fn new(cfg: DhtConfig) -> Result<Self> {
        let state = Arc::new(DhtState::new(cfg.node_pk, cfg.self_addr));

        // Open (or create) the persistent DHT store.
        if let Some(parent) = cfg.store_path.parent() {
            std::fs::create_dir_all(parent).context("create DHT store dir")?;
        }
        let store = Arc::new(DhtStore::open(&cfg.store_path).context("open DHT store")?);

        // Pre-populate in-memory routing table from persistent community peer records.
        match store.all_community_peer_records() {
            Ok(records) => {
                let count = records.len();
                for (community_pk, record) in records {
                    state.add_community_peer_record(community_pk, record);
                }
                if count > 0 {
                    info!(
                        count,
                        "DHT community peers pre-populated from persistent store"
                    );
                }
            }
            Err(e) => warn!("failed to pre-populate DHT community peers: {e}"),
        }

        let conn_cache: DhtConnCache = Arc::new(RwLock::new(HashMap::new()));

        spawn_expiry_task(Arc::clone(&state), Arc::clone(&store));

        Ok(Self(Arc::new(DhtHandleInner {
            state,
            store,
            identity: cfg.identity,
            conn_cache,
        })))
    }

    /// Create a test-mode DHT handle (temporary in-memory state, no bootstrap).
    #[cfg(test)]
    pub async fn new_for_test(node_pk: [u8; 32]) -> Self {
        let tmp = tempfile::TempDir::new().expect("create temp dir");
        let store_path = tmp.path().join("dht_test.redb");
        // Keep the TempDir alive for the duration of the test by leaking it.
        std::mem::forget(tmp);

        let state = Arc::new(DhtState::new(node_pk, None));
        let store = Arc::new(DhtStore::open(&store_path).expect("open test DHT store"));
        let conn_cache: DhtConnCache = Arc::new(RwLock::new(HashMap::new()));
        // No expiry tasks or bootstrap for test nodes.
        Self(Arc::new(DhtHandleInner {
            state,
            store,
            identity: Arc::new(NodeIdentity::generate()),
            conn_cache,
        }))
    }

    // ── Public interface ──────────────────────────────────────────────────

    /// Find nodes holding the mailbox for `user_pk`.
    ///
    /// First checks the local in-memory cache, then performs an iterative
    /// Kademlia lookup if no local record exists.
    pub async fn find_mailbox_peers(&self, user_pk: [u8; 32]) -> Result<Vec<NodeAddr>> {
        let inner = &self.0;
        if let Some(addr) = inner.state.lookup_mailbox(&user_pk) {
            return Ok(vec![addr]);
        }
        if let Some(addr) = kademlia_lookup(
            &user_pk,
            &inner.state,
            Arc::clone(&inner.identity),
            Arc::clone(&inner.conn_cache),
        )
        .await
        {
            return Ok(vec![addr]);
        }
        Ok(vec![])
    }

    /// Find community peers for `community_pk` via iterative Kademlia lookup.
    pub async fn find_community_peers(
        &self,
        community_pk: [u8; 32],
    ) -> Result<Vec<CommunityPeerRecord>> {
        let inner = &self.0;
        let records = kademlia_find_community_peers(
            &community_pk,
            &inner.state,
            Arc::clone(&inner.identity),
            Arc::clone(&inner.conn_cache),
        )
        .await;
        Ok(records)
    }

    /// Advertise that this node holds the mailbox for `user_pk` and propagate
    /// the record to the K closest DHT peers.
    pub async fn register_mailbox(&self, user_pk: [u8; 32]) {
        let inner = &self.0;
        inner.state.announce_mailbox(user_pk);
        let Some(self_addr) = inner.state.self_addr() else {
            debug!("register_mailbox: self_addr unknown, skipping propagation");
            return;
        };
        let closest = inner.state.closest_peers(&NodeId(user_pk), 20);
        for (_, peer_addr) in closest {
            let identity = Arc::clone(&inner.identity);
            let addr_copy = self_addr.clone();
            let cache = Arc::clone(&inner.conn_cache);
            tokio::spawn(async move {
                if let Some(client) = kademlia::dht_connect(&peer_addr, &cache, identity).await {
                    if let Err(e) = client.store_dht_record(user_pk, addr_copy).await {
                        debug!("register_mailbox propagation failed: {e}");
                    }
                }
            });
        }
    }

    /// Advertise this node as a community member for `community_pk` and
    /// propagate the record to K closest DHT peers.
    pub async fn register_community_peer(&self, community_pk: [u8; 32]) {
        let inner = &self.0;
        // Rate-limit: skip if we announced within the last 60 seconds.
        if !inner
            .state
            .acquire_announce_slot(community_pk, Duration::from_secs(60))
        {
            debug!("register_community_peer: cooldown active, skipping");
            return;
        }
        let Some(self_addr) = inner.state.self_addr() else {
            debug!("register_community_peer: self_addr unknown, skipping");
            return;
        };
        let own_node_pk = inner.identity.verifying_key().to_bytes();
        // Store locally.
        inner
            .state
            .announce_community_peer(community_pk, own_node_pk, self_addr.clone());
        // Propagate to K closest peers.
        let closest = inner.state.closest_peers(&NodeId(community_pk), 20);
        info!(
            addr = %self_addr,
            peers = closest.len(),
            community_pk = %community_pk.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "DHT: announcing community presence"
        );
        for (_, peer_addr) in closest {
            let identity = Arc::clone(&inner.identity);
            let self_addr_copy = self_addr.clone();
            let cache = Arc::clone(&inner.conn_cache);
            tokio::spawn(async move {
                if let Some(client) = kademlia::dht_connect(&peer_addr, &cache, identity).await {
                    if let Err(e) = client
                        .store_community_peer(community_pk, own_node_pk, self_addr_copy)
                        .await
                    {
                        debug!("register_community_peer store failed: {e}");
                    }
                }
            });
        }
    }

    /// Update the publicly-routable address of this node (called after STUN/UPnP).
    pub fn update_self_addr(&self, addr: NodeAddr) {
        self.0.state.set_self_addr(addr);
    }

    /// Inject a known peer into the routing table (e.g. from an invite link).
    pub fn add_known_peer(&self, node_pk: [u8; 32], addr: NodeAddr) {
        self.0.state.add_peer(NodeId(node_pk), addr);
    }

    /// Local-only mailbox lookup (no QUIC).
    pub fn lookup_mailbox_local(&self, user_pk: [u8; 32]) -> Option<NodeAddr> {
        self.0.state.lookup_mailbox(&user_pk)
    }

    /// Local-only community peer lookup (no QUIC).
    pub fn lookup_community_peers_local(&self, community_pk: [u8; 32]) -> Vec<CommunityPeerRecord> {
        self.0.state.lookup_community_peers(&community_pk)
    }

    /// Inject a mailbox record received from another node.
    pub fn add_mailbox_record(&self, user_pk: [u8; 32], addr: NodeAddr) {
        self.0.state.add_mailbox_record(user_pk, addr);
    }

    /// Inject a community peer record received from another node.
    pub fn add_community_peer_record(&self, community_pk: [u8; 32], record: CommunityPeerRecord) {
        self.0.state.add_community_peer_record(community_pk, record);
    }

    /// Return closest peers to `target` from the local routing table.
    pub fn closest_peers(&self, target: [u8; 32], k: usize) -> Vec<(NodeId, NodeAddr)> {
        self.0.state.closest_peers(&NodeId(target), k)
    }

    /// Return this node's own address.
    pub fn self_addr(&self) -> Option<NodeAddr> {
        self.0.state.self_addr()
    }

    /// Snapshot all community peers (for persistence).
    pub fn all_community_peers(
        &self,
    ) -> std::collections::HashMap<[u8; 32], Vec<CommunityPeerRecord>> {
        self.0.state.all_community_peers()
    }

    /// Access the underlying state (used by handler.rs for find_node responses).
    pub fn state(&self) -> &Arc<DhtState> {
        &self.0.state
    }

    /// Access the underlying store (used by handler.rs for record injection).
    pub fn store(&self) -> &Arc<DhtStore> {
        &self.0.store
    }

    /// Bootstrap the routing table by dialing the hard-coded bootstrap nodes.
    ///
    /// Fires-and-forgets each dial; errors are logged but not propagated.
    pub async fn bootstrap(&self) {
        use crate::config::bootstrap::BOOTSTRAP_NODES;
        use std::net::ToSocketAddrs;

        for addr_str in BOOTSTRAP_NODES {
            let resolved: Vec<NodeAddr> = if let Ok(parsed) = addr_str.parse::<NodeAddr>() {
                vec![parsed]
            } else {
                match addr_str.to_socket_addrs() {
                    Ok(it) => it.map(|sa| NodeAddr::new(sa.ip(), sa.port())).collect(),
                    Err(e) => {
                        warn!("DHT bootstrap: failed to resolve {addr_str:?}: {e}");
                        continue;
                    }
                }
            };

            for addr in resolved {
                let inner = Arc::clone(&self.0);
                tokio::spawn(async move {
                    match NodeClient::connect(addr.clone(), [0u8; 32], Arc::clone(&inner.identity))
                        .await
                    {
                        Ok((client, node_pk, _)) => {
                            inner.state.add_peer(NodeId(node_pk), addr.clone());
                            inner.conn_cache.write().await.insert(addr, client);
                            debug!("DHT bootstrap: connected");
                        }
                        Err(e) => {
                            debug!("DHT bootstrap: connect failed: {e}");
                        }
                    }
                });
            }
        }
    }

    /// Lookup the `x25519_pk_for_member` across all stored community peer records.
    /// This delegates to the persistent store.
    pub fn store_ref(&self) -> &Arc<DhtStore> {
        &self.0.store
    }
}
