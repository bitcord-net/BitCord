use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Identity ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityInfo {
    pub peer_id: String,
    pub display_name: Option<String>,
    pub status: UserStatus,
    pub public_key_hex: String,
    pub public_addr: Option<String>,
    /// SHA-256 fingerprint of this node's TLS certificate (64-char hex).
    /// Present only when the embedded QUIC server is running.
    pub tls_fingerprint_hex: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    #[default]
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
    Offline,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetDisplayNameParams {
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetStatusParams {
    pub status: UserStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangePassphraseParams {
    pub old_passphrase: String,
    pub new_passphrase: String,
}

// ── Communities ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommunityInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub public_key_hex: String,
    pub admin_ids: Vec<String>,
    pub channel_ids: Vec<String>,
    pub seed_nodes: Vec<String>,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    /// `true` when the community has been fully synced with at least one peer
    /// (i.e. `version > 0`).  Placeholder communities that have never connected
    /// to a seed node have `reachable = false`.
    #[serde(default = "default_reachable")]
    pub reachable: bool,
}

fn default_reachable() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateCommunityParams {
    pub name: String,
    pub description: String,
    pub seed_nodes: Vec<String>,
    /// Password for private hosting nodes.  `None` for open nodes.
    #[serde(default)]
    pub hosting_password: Option<String>,
    /// SHA-256 TLS certificate fingerprint of the seed node (64-char hex).
    /// Required when `seed_nodes` contains external addresses so that
    /// certificate pinning is enforced from the start.
    ///
    /// A single fingerprint is applied to **all** addresses in `seed_nodes`.
    /// Communities with multiple seed nodes operated by different parties
    /// (and therefore different TLS certificates) are not currently supported.
    #[serde(default)]
    pub seed_fingerprint_hex: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinCommunityParams {
    /// Base64url-encoded invite payload: `{ community_id, name, seed_nodes[] }`.
    pub invite: String,
    /// Password for private hosting nodes.  `None` for open nodes.
    #[serde(default)]
    pub hosting_password: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateManifestParams {
    pub community_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub seed_nodes: Option<Vec<String>>,
    /// SHA-256 TLS certificate fingerprint of the new seed node (64-char hex).
    /// Required when changing `seed_nodes` so that certificate pinning is
    /// enforced for connections to the new seed.
    #[serde(default)]
    pub seed_fingerprint_hex: Option<String>,
}

// ── Channels ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: String,
    pub community_id: String,
    pub name: String,
    pub kind: ChannelKindDto,
    pub version: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKindDto {
    Text,
    Announcement,
    Voice,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateChannelParams {
    pub community_id: String,
    pub name: String,
    pub kind: ChannelKindDto,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteChannelParams {
    pub community_id: String,
    pub channel_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RotateKeyParams {
    pub community_id: String,
    pub channel_id: String,
}

// ── Messages ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageInfo {
    pub id: String,
    pub channel_id: String,
    pub community_id: String,
    pub author_id: String,
    pub timestamp: DateTime<Utc>,
    /// Decrypted message body (plaintext).
    pub body: String,
    pub reply_to: Option<String>,
    pub edited_at: Option<DateTime<Utc>>,
    pub deleted: bool,
    pub reactions: Vec<ReactionInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactionInfo {
    pub emoji: String,
    pub user_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageParams {
    pub community_id: String,
    pub channel_id: String,
    pub body: String,
    pub reply_to: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditMessageParams {
    pub community_id: String,
    pub channel_id: String,
    pub message_id: String,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteMessageParams {
    pub community_id: String,
    pub channel_id: String,
    pub message_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetHistoryParams {
    pub community_id: String,
    pub channel_id: String,
    pub before: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactionParams {
    pub community_id: String,
    pub channel_id: String,
    pub message_id: String,
    pub emoji: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkReadParams {
    pub community_id: String,
    pub channel_id: String,
    pub message_id: String,
}

// ── Members ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberInfo {
    pub user_id: String,
    pub display_name: String,
    pub avatar_cid: Option<String>,
    pub roles: Vec<RoleDto>,
    pub joined_at: DateTime<Utc>,
    pub public_key_hex: String,
    pub status: UserStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoleDto {
    Admin,
    Moderator,
    Member,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KickBanParams {
    pub community_id: String,
    pub user_id: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateRoleParams {
    pub community_id: String,
    pub user_id: String,
    pub role: RoleDto,
}

// ── Direct Messages ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DmMessageInfo {
    pub id: String,
    pub peer_id: String,
    pub author_id: String,
    pub timestamp: DateTime<Utc>,
    pub body: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    pub edited_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendDmParams {
    pub peer_id: String,
    pub body: String,
    pub reply_to: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetDmHistoryParams {
    pub peer_id: String,
    pub before: Option<String>,
    pub limit: Option<u32>,
}

// ── Node ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeLocalInfo {
    /// Base58-encoded Ed25519 public key — the node's canonical network address.
    pub node_address: String,
    /// Actual listen addresses reported by the network layer.
    pub listen_addrs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerSummary {
    pub peer_id: String,
    pub addresses: Vec<String>,
    pub latency_ms: Option<u64>,
    pub relay_capable: bool,
    pub reputation: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeConfigDto {
    pub listen_addrs: Vec<String>,
    pub seed_nodes: Vec<String>,
    pub max_connections: usize,
    pub storage_limit_mb: u64,
    pub bandwidth_limit_kbps: Option<u64>,
    pub is_seed_node: bool,
    pub seed_priority: u8,
    pub mdns_enabled: bool,
    pub log_level: String,
    pub server_enabled: bool,
    pub preferred_mailbox_node: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetConfigParams {
    pub listen_addrs: Option<Vec<String>>,
    pub seed_nodes: Option<Vec<String>>,
    pub max_connections: Option<usize>,
    pub storage_limit_mb: Option<u64>,
    pub bandwidth_limit_kbps: Option<Option<u64>>,
    pub is_seed_node: Option<bool>,
    pub seed_priority: Option<u8>,
    pub mdns_enabled: Option<bool>,
    pub log_level: Option<String>,
    pub server_enabled: Option<bool>,
    /// `Some(None)` clears the preference; `Some(Some(addr))` sets it.
    pub preferred_mailbox_node: Option<Option<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetPreferredMailboxCommunityParams {
    pub community_id: String,
}

/// Internal mutable identity state (display name, presence status).
#[derive(Clone, Debug, Default)]
pub struct IdentityState {
    pub display_name: Option<String>,
    pub status: UserStatus,
}

// ── DM packet (wire format for P2P DM delivery) ───────────────────────────────

/// msgpack-encoded payload transmitted over the `/bitcord/dm/1.0.0` protocol.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DmPacket {
    pub id: String,
    /// BitCord peer_id (SHA-256 hex) of the sender.
    pub author_id: String,
    pub body: String,
    pub timestamp: DateTime<Utc>,
}

impl DmPacket {
    /// Encode to postcard bytes for transmission.
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        postcard::to_allocvec(self).map_err(|e| anyhow::anyhow!(e))
    }

    /// Decode from postcard bytes.
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        postcard::from_bytes(bytes).map_err(|e| anyhow::anyhow!(e))
    }
}
