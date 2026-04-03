pub mod event_processor;
pub mod push_broadcaster;
pub mod rpc_server;
pub mod types;
pub mod ws_handler;

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Raw gossip messages buffered during a channel key rotation.
/// Each entry is a `(topic, data)` pair that could not be decrypted with the
/// old key and will be replayed once the updated key arrives.
pub type PendingRotationBuffer = Arc<tokio::sync::Mutex<HashMap<String, Vec<(String, Vec<u8>)>>>>;

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use http::HeaderValue;
use jsonrpsee::server::{Server, ServerHandle};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{debug, info, warn};

/// Minimal peer info retained for DM delivery after a shared community is disbanded.
/// Keyed by peer_id (hex); persisted to `dm_peers.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DmPeerInfo {
    pub display_name: String,
    pub x25519_public_key: [u8; 32],
}

// ── ApiHandle ─────────────────────────────────────────────────────────────────

/// A running API server handle that includes the bound socket address.
pub struct ApiHandle {
    /// The actual address the server is listening on (important when `port = 0`).
    pub local_addr: SocketAddr,
    inner: ServerHandle,
}

impl ApiHandle {
    /// Returns the address the server is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Request the server to stop. Safe to call multiple times.
    pub fn stop(&self) {
        self.inner.stop().ok();
    }

    /// Future that resolves once the server has fully shut down.
    pub async fn stopped(self) {
        self.inner.stopped().await;
    }
}

use crate::{
    config::NodeConfig,
    crypto::channel_keys::ChannelKey,
    dht::DhtHandle,
    model::{
        channel::ChannelManifest, community::SignedManifest, membership::MembershipRecord,
        message::MessageContent,
    },
    network::NetworkCommand,
    resource::metrics::NodeMetrics,
    state::MessageLog,
};
use ulid::Ulid;

pub use event_processor::process_swarm_events;
pub use push_broadcaster::{PushBroadcaster, PushEvent};
pub use rpc_server::build_rpc_module;
pub use types::{DmMessageInfo, DmPacket, IdentityState, PeerSummary, UserStatus};

// ── Community helpers ─────────────────────────────────────────────────────────

/// Remove all local state for a community: manifest, members, channels, channel keys.
/// Also unsubscribes from the community and all its channel GossipSub topics.
pub(crate) async fn remove_community_local(ctx: &AppState, community_id: &str) {
    // Collect channel IDs belonging to this community.
    let channel_ids: Vec<String> = {
        let channels = ctx.channels.read().await;
        channels
            .values()
            .filter(|c| c.community_id.to_string() == community_id)
            .map(|c| c.id.to_string())
            .collect()
    };

    // Capture the community public key before removing, for NodeStore cleanup.
    let community_pk: Option<[u8; 32]> = {
        let communities = ctx.communities.read().await;
        communities.get(community_id).map(|s| s.manifest.public_key)
    };

    {
        let mut communities = ctx.communities.write().await;
        communities.remove(community_id);
        save_table(
            &ctx.data_dir.join("communities.json"),
            &*communities,
            ctx.encryption_key.as_ref(),
        );
    }
    {
        let mut members = ctx.members.write().await;
        members.remove(community_id);
        save_table(
            &ctx.data_dir.join("members.json"),
            &*members,
            ctx.encryption_key.as_ref(),
        );
    }
    {
        let mut channels = ctx.channels.write().await;
        for ch_id in &channel_ids {
            channels.remove(ch_id);
        }
        save_table(
            &ctx.data_dir.join("channels.json"),
            &*channels,
            ctx.encryption_key.as_ref(),
        );
    }
    {
        let mut keys = ctx.channel_keys.write().await;
        for ch_id in &channel_ids {
            keys.remove(ch_id);
        }
        save_table(
            &ctx.data_dir.join("channel_keys.json"),
            &*keys,
            ctx.encryption_key.as_ref(),
        );
    }
    {
        let mut passwords = ctx.hosting_passwords.write().await;
        passwords.remove(community_id);
        save_table(
            &ctx.data_dir.join("hosting_passwords.json"),
            &*passwords,
            ctx.encryption_key.as_ref(),
        );
    }

    // Clean up NodeStore data for this community.
    if let (Some(store), Some(pk)) = (&ctx.node_store, community_pk) {
        if let Err(e) = store.remove_community(&pk) {
            tracing::warn!("failed to remove community from NodeStore: {e}");
        }
    }

    let community_topic = format!("/bitcord/community/{community_id}/1.0.0");
    let _ = ctx
        .swarm_cmd_tx
        .send(NetworkCommand::Unsubscribe(community_topic))
        .await;
    for ch_id in &channel_ids {
        let channel_topic = format!("/bitcord/channel/{ch_id}/1.0.0");
        let _ = ctx
            .swarm_cmd_tx
            .send(NetworkCommand::Unsubscribe(channel_topic))
            .await;
    }
}

// ── Persistence helpers ───────────────────────────────────────────────────────

pub fn load_table<T: DeserializeOwned>(
    path: &Path,
    encryption_key: Option<&[u8; 32]>,
) -> HashMap<String, T> {
    let raw = match std::fs::read(path) {
        Ok(data) => data,
        Err(_) => return HashMap::new(),
    };

    let json_bytes = match encryption_key {
        Some(key) => match crate::crypto::encrypted_io::decrypt_bytes(&raw, key) {
            Ok(decrypted) => decrypted,
            Err(_) => {
                // Try reading as plaintext JSON (migration path from unencrypted).
                match serde_json::from_slice(&raw) {
                    Ok(table) => return table,
                    Err(_) => return HashMap::new(),
                }
            }
        },
        None => raw,
    };

    serde_json::from_slice(&json_bytes).unwrap_or_default()
}

pub fn save_table<T: Serialize>(
    path: &Path,
    data: &HashMap<String, T>,
    encryption_key: Option<&[u8; 32]>,
) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_vec(data) {
        Ok(json_bytes) => {
            let to_write = match encryption_key {
                Some(key) => match crate::crypto::encrypted_io::encrypt_bytes(&json_bytes, key) {
                    Ok(encrypted) => encrypted,
                    Err(e) => {
                        warn!("failed to encrypt {:?}: {e}", path);
                        return;
                    }
                },
                None => json_bytes,
            };
            if let Err(e) = std::fs::write(path, to_write) {
                warn!("failed to persist {:?}: {e}", path);
            }
        }
        Err(e) => warn!("failed to serialize {:?}: {e}", path),
    }
}

// ── AppState ──────────────────────────────────────────────────────────────────

/// Shared application state injected into every JSON-RPC handler.
///
/// All fields are `Arc`-wrapped so the state can be cheaply cloned across
/// async tasks without copying the underlying data.
pub struct AppState {
    /// Hex-encoded SHA-256 of the local Ed25519 verifying key (immutable).
    pub peer_id: String,
    /// Hex-encoded Ed25519 verifying key bytes (immutable).
    pub public_key_hex: String,
    /// Base58-encoded Ed25519 public key — the node's canonical network address.
    pub node_address: String,
    /// Local Ed25519 signing key — used to sign community manifests and messages.
    pub signing_key: Arc<SigningKey>,
    /// Mutable identity state: display name and presence status.
    pub identity_state: Arc<RwLock<IdentityState>>,
    /// Node configuration — may be updated via `node_set_config`.
    pub config: Arc<RwLock<NodeConfig>>,
    /// Append-only encrypted message log — all channel message data.
    pub message_log: Arc<Mutex<MessageLog>>,
    /// Channel for sending commands to the network handle.
    pub swarm_cmd_tx: tokio::sync::mpsc::Sender<NetworkCommand>,
    /// Live node operational metrics (lock-free atomic reads).
    pub metrics: Arc<NodeMetrics>,
    /// Broadcaster for server-to-client push events.
    pub broadcaster: PushBroadcaster,
    /// Snapshot of currently connected peers, updated by the swarm event loop.
    pub connected_peers: Arc<RwLock<HashMap<String, Vec<PeerSummary>>>>,
    /// Community store: community_id → signed manifest (persisted to data_dir).
    pub communities: Arc<RwLock<HashMap<String, SignedManifest>>>,
    /// Channel store: channel_id → channel manifest (persisted to data_dir).
    pub channels: Arc<RwLock<HashMap<String, ChannelManifest>>>,
    /// Per-channel symmetric key material: channel_id → raw 32-byte key (persisted to data_dir).
    pub channel_keys: Arc<RwLock<HashMap<String, [u8; 32]>>>,
    /// DM message store: other_peer_id → ordered list of messages (persisted to data_dir).
    pub dms: Arc<RwLock<HashMap<String, Vec<DmMessageInfo>>>>,
    /// DM peer cache: peer_id → minimal info needed for DM delivery (persisted to data_dir).
    /// Populated from community member records; NOT cleared when a community is disbanded so
    /// that DMs can still be sent/received after all shared communities are gone.
    pub dm_peers: Arc<RwLock<HashMap<String, DmPeerInfo>>>,
    /// Member store: community_id → membership records keyed by user_id hex (persisted to data_dir).
    pub members: Arc<RwLock<HashMap<String, HashMap<String, MembershipRecord>>>>,
    /// Ban list: community_id → list of banned user_id strings (persisted to data_dir).
    pub bans: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Base data directory — used for persisting communities, channels, and keys.
    pub data_dir: PathBuf,
    /// Path to the config file this node was loaded from — used to persist display_name changes.
    pub config_path: PathBuf,
    /// Persistent node store (headless node only, or embedded node).
    pub node_store: Option<Arc<crate::node::store::NodeStore>>,
    /// List of community IDs that need a manifest sync from the next available peer.
    pub pending_manifest_syncs: Arc<tokio::sync::Mutex<Vec<String>>>,
    /// Actual resolved listen addresses reported by the swarm (includes /p2p/<peer_id> suffix).
    /// Populated as NewListenAddr events arrive; used to build invite links.
    pub actual_listen_addrs: Arc<RwLock<Vec<String>>>,
    /// The single publicly routable address for this node (set from NAT/STUN discovery).
    /// This is the address used in invite links; it replaces the full `actual_listen_addrs` list
    /// to avoid advertising private/loopback IPs to remote clients.
    pub public_addr: Arc<RwLock<Option<String>>>,
    /// Presence map: peer_id (hex) → current UserStatus.
    /// Updated by local heartbeat RPC and incoming P2P presence gossip.
    pub presence: Arc<RwLock<HashMap<String, UserStatus>>>,
    /// Per-channel read state: channel_id → last-read sequence number (persisted to data_dir).
    pub read_state: Arc<RwLock<HashMap<String, u64>>>,
    /// Per-community hosting passwords: community_id → password for private seed nodes
    /// (persisted to data_dir).
    pub hosting_passwords: Arc<RwLock<HashMap<String, String>>>,
    /// Set of community IDs whose seed peer is currently connected.
    /// Empty for self-hosted communities (we ARE the seed) or communities without seed nodes.
    pub seed_connected_communities: Arc<RwLock<HashSet<String>>>,
    /// Symmetric encryption key for at-rest table persistence.
    /// Derived from the user's passphrase via Argon2id.  `None` when running
    /// without a passphrase (tests / headless with empty passphrase).
    pub encryption_key: Option<[u8; 32]>,
    /// Messages buffered while waiting for a new channel key after rotation.
    /// Keyed by channel_id; each entry holds the raw gossip (topic, data) pairs
    /// that could not be decrypted with the old key.  Drained once the updated
    /// key arrives via `FetchManifest`.
    pub pending_rotation_messages: PendingRotationBuffer,
    /// Previous channel keys retained after a key rotation so that messages
    /// encrypted with the old key (e.g. from history catch-up) can still be
    /// decrypted.  Keyed by channel_id → old 32-byte key.
    pub previous_channel_keys: Arc<RwLock<HashMap<String, [u8; 32]>>>,
    /// TLS certificate fingerprints for known seed nodes: seed address string → SHA-256 fingerprint.
    /// Populated from invite links and used for certificate pinning on reconnect.
    pub seed_fingerprints: Arc<RwLock<HashMap<String, [u8; 32]>>>,
    /// SHA-256 fingerprint of this node's own TLS certificate (64-char hex).
    /// Present when the embedded QUIC server is running; `None` otherwise.
    pub local_tls_fingerprint_hex: Option<String>,
    /// DHT handle for mailbox/community routing; `None` for `GossipClient` mode.
    pub dht: Option<Arc<DhtHandle>>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        peer_id: String,
        public_key_hex: String,
        node_address: String,
        signing_key: SigningKey,
        config: NodeConfig,
        config_path: PathBuf,
        message_log: MessageLog,
        swarm_cmd_tx: tokio::sync::mpsc::Sender<NetworkCommand>,
        metrics: Arc<NodeMetrics>,
        node_store: Option<Arc<crate::node::store::NodeStore>>,
        encryption_key: Option<[u8; 32]>,
        local_tls_fingerprint_hex: Option<String>,
        dht: Option<Arc<DhtHandle>>,
    ) -> Self {
        let data_dir = config.data_dir.clone();
        let ek = encryption_key.as_ref();
        let mut communities: HashMap<String, SignedManifest> =
            load_table(&data_dir.join("communities.json"), ek);
        let mut channels: HashMap<String, ChannelManifest> =
            load_table(&data_dir.join("channels.json"), ek);
        // Channel keys are stored as arrays of numbers by serde_json ([u8;32] → [0,1,...]).
        let mut channel_keys: HashMap<String, [u8; 32]> =
            load_table(&data_dir.join("channel_keys.json"), ek);
        let previous_channel_keys: HashMap<String, [u8; 32]> =
            load_table(&data_dir.join("previous_channel_keys.json"), ek);
        let dms: HashMap<String, Vec<DmMessageInfo>> = load_table(&data_dir.join("dms.json"), ek);
        let mut members: HashMap<String, HashMap<String, MembershipRecord>> =
            load_table(&data_dir.join("members.json"), ek);
        let bans: HashMap<String, Vec<String>> = load_table(&data_dir.join("bans.json"), ek);
        let read_state: HashMap<String, u64> = load_table(&data_dir.join("read_state.json"), ek);
        let hosting_passwords: HashMap<String, String> =
            load_table(&data_dir.join("hosting_passwords.json"), ek);
        let seed_fingerprints: HashMap<String, [u8; 32]> =
            load_table(&data_dir.join("seed_fingerprints.json"), ek);

        let mut message_log = message_log;
        if let Some(store) = &node_store {
            // ── Sync members to NodeStore ─────────────────────────────────
            for (comm_id_str, comm_members) in &members {
                if let Some(comm) = communities.get(comm_id_str) {
                    let pk = comm.manifest.public_key;
                    if let Ok(Some(mut meta)) = store.get_community_meta(&pk) {
                        // Merge members.
                        let mut changed = false;
                        for member in comm_members.values() {
                            if let std::collections::hash_map::Entry::Vacant(e) =
                                meta.members.entry(member.user_id.to_string())
                            {
                                e.insert(member.clone());
                                changed = true;
                            }
                        }
                        if changed {
                            let _ = store.set_community_meta(&pk, &meta);
                        }
                    }
                }
            }

            // ── Back-populate in-memory state from NodeStore ─────────────
            // Server (relay) nodes do not have communities.json / channels.json
            // because they never own communities.  Load everything from redb so
            // that gossip handlers can find community entries after a restart.
            if let Ok(community_pks) = store.all_communities() {
                for cpk in &community_pks {
                    if let Ok(Some(meta)) = store.get_community_meta(cpk) {
                        if let Some(signed) = &meta.manifest {
                            let comm_id = signed.manifest.id.to_string();
                            communities
                                .entry(comm_id.clone())
                                .or_insert_with(|| signed.clone());
                            for ch in &meta.channels {
                                let ch_id = ch.id.to_string();
                                channels.entry(ch_id.clone()).or_insert_with(|| ch.clone());
                            }
                            for (ch_id, key_bytes) in &meta.channel_keys {
                                if key_bytes.len() == 32 {
                                    let mut arr = [0u8; 32];
                                    arr.copy_from_slice(key_bytes);
                                    channel_keys.entry(ch_id.clone()).or_insert(arr);
                                }
                            }
                            let comm_members = members.entry(comm_id).or_default();
                            for (uid, member) in &meta.members {
                                comm_members
                                    .entry(uid.clone())
                                    .or_insert_with(|| member.clone());
                            }
                        }
                    }
                }
            }

            // ── Load message history and rebuild reactions cache ──────────
            for (ch_id_str, manifest) in &channels {
                if let Ok(ch_id) = Ulid::from_string(ch_id_str) {
                    if let Some(comm) = communities.get(&manifest.community_id.to_string()) {
                        let pk = comm.manifest.public_key;
                        if let Ok(entries) = store.get_messages(&pk, &ch_id, 0) {
                            let key_bytes = channel_keys.get(ch_id_str).copied();
                            let prev_key_bytes = previous_channel_keys.get(ch_id_str).copied();
                            for entry in entries {
                                // Rebuild reactions cache from stored reaction entries.
                                // Try current key first, then fall back to previous
                                // (pre-rotation) key so old messages remain readable.
                                let plain = key_bytes
                                    .and_then(|kb| {
                                        ChannelKey::from_bytes(kb)
                                            .decrypt_message(&entry.nonce, &entry.ciphertext)
                                            .ok()
                                    })
                                    .or_else(|| {
                                        prev_key_bytes.and_then(|kb| {
                                            ChannelKey::from_bytes(kb)
                                                .decrypt_message(&entry.nonce, &entry.ciphertext)
                                                .ok()
                                        })
                                    });
                                if let Some(plain) = plain {
                                    if let Some(MessageContent::Reaction {
                                        ref target_message_id,
                                        ref emoji,
                                        is_add,
                                    }) = MessageContent::decode(&plain)
                                    {
                                        if is_add {
                                            message_log.react(
                                                target_message_id,
                                                emoji,
                                                &entry.author_id,
                                            );
                                        } else {
                                            message_log.unreact(
                                                target_message_id,
                                                emoji,
                                                &entry.author_id,
                                            );
                                        }
                                    }
                                }
                                message_log.append_entry(ch_id_str, entry);
                            }
                        }
                    }
                }
            }
        }

        // Build dm_peers from the persisted cache, then seed any missing entries
        // from the current community member records.  This ensures that DMs can
        // still be delivered even after a shared community is disbanded.
        let mut dm_peers: HashMap<String, DmPeerInfo> =
            load_table(&data_dir.join("dm_peers.json"), ek);
        for list in members.values() {
            for rec in list.values() {
                dm_peers
                    .entry(rec.user_id.to_string())
                    .or_insert_with(|| DmPeerInfo {
                        display_name: rec.display_name.clone(),
                        x25519_public_key: rec.x25519_public_key,
                    });
            }
        }

        let identity_state = IdentityState {
            display_name: config.display_name.clone(),
            ..IdentityState::default()
        };
        Self {
            peer_id,
            public_key_hex,
            node_address,
            signing_key: Arc::new(signing_key),
            identity_state: Arc::new(RwLock::new(identity_state)),
            config: Arc::new(RwLock::new(config)),
            message_log: Arc::new(Mutex::new(message_log)),
            swarm_cmd_tx,
            metrics,
            broadcaster: PushBroadcaster::new(),
            connected_peers: Arc::new(RwLock::new(HashMap::new())),
            communities: Arc::new(RwLock::new(communities)),
            channels: Arc::new(RwLock::new(channels)),
            channel_keys: Arc::new(RwLock::new(channel_keys)),
            dms: Arc::new(RwLock::new(dms)),
            dm_peers: Arc::new(RwLock::new(dm_peers)),
            members: Arc::new(RwLock::new(members)),
            bans: Arc::new(RwLock::new(bans)),
            data_dir,
            config_path,
            node_store,
            pending_manifest_syncs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            actual_listen_addrs: Arc::new(RwLock::new(Vec::new())),
            public_addr: Arc::new(RwLock::new(None)),
            presence: Arc::new(RwLock::new(HashMap::new())),
            read_state: Arc::new(RwLock::new(read_state)),
            hosting_passwords: Arc::new(RwLock::new(hosting_passwords)),
            seed_connected_communities: Arc::new(RwLock::new(HashSet::new())),
            encryption_key,
            pending_rotation_messages: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            previous_channel_keys: Arc::new(RwLock::new(previous_channel_keys)),
            seed_fingerprints: Arc::new(RwLock::new(seed_fingerprints)),
            local_tls_fingerprint_hex,
            dht,
        }
    }

    /// Re-subscribe to all known community and channel topics.
    ///
    /// This should be called once after the network handle and event processor
    /// are started.
    pub async fn bootstrap_network(&self) -> Result<()> {
        let config = self.config.read().await;
        let communities = self.communities.read().await;
        let channels = self.channels.read().await;
        let hosting_passwords = self.hosting_passwords.read().await;
        let seed_fps = self.seed_fingerprints.read().await;

        let mut pending = self.pending_manifest_syncs.lock().await;

        // Bootstrap DHT routing table via DhtHandle (fires-and-forgets each dial).
        if let Some(dht) = &self.dht {
            let dht = Arc::clone(dht);
            tokio::spawn(async move { dht.bootstrap().await });
        }

        for (id, signed) in communities.iter() {
            // Mark for sync on connect to ensure we have the latest member list and keys.
            if !pending.contains(id) {
                pending.push(id.clone());
            }

            // Proactively try to fetch from any ALREADY connected peers.
            let peers_for_community = {
                let peers_map = self.connected_peers.read().await;
                peers_map.get(id).cloned().unwrap_or_default()
            };
            for peer in peers_for_community {
                let _ = self
                    .swarm_cmd_tx
                    .send(NetworkCommand::FetchManifest {
                        peer_id: peer.peer_id.clone(),
                        community_id: id.clone(),
                        community_pk: signed.manifest.public_key,
                    })
                    .await;
            }

            let topic = format!("/bitcord/community/{}/1.0.0", id);
            debug!(%topic, "bootstrapping community subscription");
            let _ = self
                .swarm_cmd_tx
                .send(NetworkCommand::Subscribe(topic))
                .await;

            // Dial known seed nodes for this community, skipping our own addresses.
            let own_quic_port = config.quic_port;

            let own_ips: std::collections::HashSet<String> = {
                let mut ips: std::collections::HashSet<String> = if_addrs::get_if_addrs()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|i| i.ip().to_string())
                    .collect();
                // Always consider loopback as "self".
                ips.insert("127.0.0.1".to_string());
                ips.insert("::1".to_string());
                ips
            };
            // Also collect NAT-discovered / previously reported listen addresses
            // (e.g. the external IP auto-added to the manifest at creation time).
            let own_listen_addrs: std::collections::HashSet<crate::network::NodeAddr> = {
                self.actual_listen_addrs
                    .read()
                    .await
                    .iter()
                    .filter_map(|a| a.parse::<crate::network::NodeAddr>().ok())
                    .collect()
            };
            // Also include the NAT-mapped public address (may differ from all local interfaces).
            let own_public_addr: Option<String> = self.public_addr.read().await.clone();
            for addr_str in &signed.manifest.seed_nodes {
                if let Ok(addr) = addr_str.parse::<crate::network::NodeAddr>() {
                    // Skip addresses that point back at our own QUIC server:
                    // local interface IP, known listen addr, or NAT-mapped public addr.
                    if (own_ips.contains(&addr.ip.to_string()) && addr.port == own_quic_port)
                        || own_listen_addrs.contains(&addr)
                        || own_public_addr.as_deref() == Some(addr_str.as_str())
                    {
                        debug!(%addr, "bootstrapping: skipping self-dial");
                        continue;
                    }
                    debug!(%addr, "bootstrapping seed node dial");
                    let fp = seed_fps.get(addr_str).copied().unwrap_or_else(|| {
                        warn!(%addr, "no stored fingerprint for seed node; connecting without certificate pinning (TOFU)");
                        [0u8; 32]
                    });
                    let _ = self
                        .swarm_cmd_tx
                        .send(NetworkCommand::Dial {
                            addr,
                            is_seed: true,
                            join_community: Some((signed.manifest.public_key, id.clone())),
                            join_community_password: hosting_passwords.get(id).cloned(),
                            cert_fingerprint: fp,
                        })
                        .await;
                }
            }
        }

        for (id, _) in channels.iter() {
            let topic = format!("/bitcord/channel/{}/1.0.0", id);
            debug!(%topic, "bootstrapping channel subscription");
            let _ = self
                .swarm_cmd_tx
                .send(NetworkCommand::Subscribe(topic))
                .await;
        }

        info!(
            communities = communities.len(),
            channels = channels.len(),
            "bootstrapped network subscriptions"
        );
        Ok(())
    }
}

// ── ApiServer ─────────────────────────────────────────────────────────────────

/// JSON-RPC 2.0 over WebSocket server.
///
/// Binds on `addr` (default `127.0.0.1:7331`) and serves all registered RPC
/// methods. CORS is restricted to `tauri://localhost` and
/// `http://localhost:1420`.
pub struct ApiServer;

impl ApiServer {
    /// Start the API server and return an [`ApiHandle`] that keeps it alive.
    ///
    /// The handle must be kept alive (stored in a variable or awaited via
    /// `handle.stopped()`) for the server to continue accepting connections.
    pub async fn start(addr: SocketAddr, state: Arc<AppState>) -> Result<ApiHandle> {
        let cors = CorsLayer::new().allow_origin(AllowOrigin::list([
            "tauri://localhost"
                .parse::<HeaderValue>()
                .context("parse tauri origin")?,
            "http://localhost:1420"
                .parse::<HeaderValue>()
                .context("parse vite origin")?,
        ]));

        let server = Server::builder()
            .set_http_middleware(tower::ServiceBuilder::new().layer(cors))
            .build(addr)
            .await
            .context("bind JSON-RPC server")?;

        let local_addr = server.local_addr().context("get server local addr")?;
        info!(%local_addr, "JSON-RPC API server listening");

        let module = build_rpc_module(state).context("build RPC module")?;
        let inner = server.start(module);
        Ok(ApiHandle { local_addr, inner })
    }
}
