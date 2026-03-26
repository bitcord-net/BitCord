use std::collections::HashMap;

use tokio::sync::{broadcast, mpsc};

use crate::{
    crypto::dm::DmEnvelope,
    model::{channel::ChannelManifest, community::SignedManifest, membership::MembershipRecord},
    network::{client::NodeClient, node_addr::NodeAddr, protocol::NodePush},
    state::message_log::LogEntry,
};

/// The push-payload type used by `NodeServer` / `ConnectionHandler`.
pub(crate) type ServerPushTx = broadcast::Sender<(Option<[u8; 32]>, NodePush)>;

/// Seed peer entry: address + optional (community_pk, community_id) for auto-join + password + cert fingerprint.
pub(crate) type SeedPeerInfo = (
    NodeAddr,
    Option<([u8; 32], String)>,
    Option<String>,
    [u8; 32],
);

// ── Commands ─────────────────────────────────────────────────────────────────

/// Commands sent from the application layer to the network layer.
#[derive(Debug)]
pub enum NetworkCommand {
    /// Dial a peer at the given address.
    ///
    /// Set `is_seed = true` for always-on seed/relay nodes so the network
    /// layer can prioritise them and auto-reconnect when they drop.
    ///
    /// If `join_community` is `Some`, the network layer will automatically
    /// issue a `HostingCert` and call `JoinCommunity` on the remote peer
    /// immediately after authentication.
    Dial {
        addr: NodeAddr,
        is_seed: bool,
        join_community: Option<([u8; 32], String)>,
        /// Password for private nodes — forwarded in the `JoinCommunity` request.
        join_community_password: Option<String>,
        /// SHA-256 fingerprint of the remote node's TLS certificate.
        /// `[0u8; 32]` means TOFU mode (accept any certificate), used for
        /// DHT/bootstrap connections where the fingerprint is unknown.
        cert_fingerprint: [u8; 32],
    },
    /// Subscribe to a pub/sub topic (e.g. `/bitcord/channel/<id>/1.0.0`).
    Subscribe(String),
    /// Unsubscribe from a pub/sub topic.
    Unsubscribe(String),
    /// Publish data on a pub/sub topic.
    Publish { topic: String, data: Vec<u8> },
    /// Send a direct message to a peer (peer_id = hex-encoded Ed25519 public key).
    SendDm {
        peer_id: String,
        recipient_x25519_pk: [u8; 32],
        envelope: DmEnvelope,
    },
    /// Request a community manifest from a peer.
    FetchManifest {
        peer_id: String,
        community_id: String,
        community_pk: [u8; 32],
    },
    /// Request channel history from a peer.
    FetchChannelHistory {
        peer_id: String,
        community_id: String,
        community_pk: [u8; 32],
        channel_id: String,
        since_seq: u64,
    },
    /// Inject an externally discovered listen address (e.g., from NAT traversal)
    /// so that the event processor can add it to invite links.
    AddListenAddr(String),
    /// Notify the event processor that a community was joined via the QUIC server.
    NotifyCommunityJoined([u8; 32], String),
    /// Propagate a mailbox record to the K closest peers via `StoreDhtRecord`.
    ///
    /// Used after a DM is stored to spread the routing record across the network
    /// so other nodes can find the mailbox-holding node via iterative lookup.
    PropagateDhtRecord {
        user_pk: [u8; 32],
        self_addr: NodeAddr,
    },
    /// Announce a preferred mailbox node for `user_pk` to the local DHT and
    /// propagate it to the K closest peers.
    ///
    /// Unlike `PropagateDhtRecord` (which is sent by the server node that
    /// physically holds the mailbox), this command is sent by the *client* to
    /// claim a preferred hosting node before any DM has been stored there.
    AnnouncePreferredMailbox { user_pk: [u8; 32], addr: NodeAddr },
    /// Fetch all queued DMs from a peer's mailbox.
    ///
    /// Sent when a peer connects so the client pulls any DMs that arrived while
    /// it was offline.  The network handle calls `GetDms { since_seq: 0 }` on
    /// the peer and emits `DmReceived` for every entry returned.
    FetchMailbox { peer_id: String },
    /// Shut down the network handle.
    Shutdown,
}

// ── Events ───────────────────────────────────────────────────────────────────

/// Events emitted by the network layer to the application layer.
#[derive(Debug)]
pub enum NetworkEvent {
    /// A new peer connection was established (peer_id = hex-encoded Ed25519 public key).
    PeerConnected(String),
    /// A peer connection was closed.
    PeerDisconnected(String),
    /// A pub/sub message was received on the given topic.
    MessageReceived {
        topic: String,
        source: Option<String>,
        data: Vec<u8>,
    },
    /// The node is now listening on this address string.
    NewListenAddr(String),
    /// A direct message was received from a peer.
    DmReceived {
        entry: crate::state::message_log::LogEntry,
        recipient_pk: [u8; 32],
    },
    /// A peer sent us a full community manifest.
    ManifestReceived {
        from: String,
        community_id: String,
        manifest: Box<SignedManifest>,
        channels: Vec<ChannelManifest>,
        channel_keys: HashMap<String, Vec<u8>>,
        members: Vec<MembershipRecord>,
    },
    /// A peer sent us channel history.
    ChannelHistoryReceived {
        community_id: String,
        channel_id: String,
        entries: Vec<LogEntry>,
    },
    /// A community was joined via the QUIC server.
    CommunityJoined([u8; 32], String),
    /// Auto-join to a seed node failed (e.g. invalid hosting password).
    /// Carries the community_id so the local placeholder can be removed.
    CommunityJoinFailed {
        community_id: String,
        reason: String,
    },
    /// A `FetchManifest` request returned 404 — the queried peer no longer
    /// hosts this community.  Used to detect community deletion when an
    /// offline node reconnects and all known peers have purged the manifest.
    ManifestNotFound {
        community_id: String,
        peer_id: String,
    },
    /// A seed peer connected for a specific community.
    SeedPeerConnected { community_id: String },
    /// A seed peer disconnected from a specific community.
    SeedPeerDisconnected { community_id: String },
    /// A network-level error occurred.
    Error(String),
}

// ── Internal types ────────────────────────────────────────────────────────────

/// Sent from a dial task to the main loop once a connection is established.
pub(crate) struct PeerRegistration {
    pub(crate) peer_id: String,
    pub(crate) client: NodeClient,
    /// Whether this peer is a seed node (i.e. was dialled with `is_seed=true`).
    pub(crate) is_seed: bool,
    /// The address we connected to — stored for seed reconnect purposes.
    pub(crate) addr: NodeAddr,
    /// Push receiver for this connection — used to spawn a push_reader task.
    pub(crate) push_rx: mpsc::Receiver<NodePush>,
    /// Gossip event forwarder for the push_reader task.
    pub(crate) evt_fwd: mpsc::Sender<NetworkEvent>,
    /// Own public key hex — used by push_reader to filter reflected gossip.
    pub(crate) own_pk: String,
    /// Optional community to join immediately after connection.
    pub(crate) join_community: Option<([u8; 32], String)>,
    /// Password to supply when joining a private node.
    pub(crate) join_community_password: Option<String>,
    /// SHA-256 fingerprint of the remote node's TLS certificate (for reconnects).
    pub(crate) cert_fingerprint: [u8; 32],
}
