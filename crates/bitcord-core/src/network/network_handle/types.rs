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
///
/// This enum is **gossip-only**: all DHT operations are performed directly
/// through [`crate::dht::DhtHandle`] and never pass through `NetworkCommand`.
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
        /// `[0u8; 32]` means TOFU mode (accept any certificate).
        cert_fingerprint: [u8; 32],
    },
    /// Subscribe to a pub/sub topic (e.g. `/bitcord/channel/<id>/1.0.0`).
    Subscribe(String),
    /// Unsubscribe from a pub/sub topic.
    Unsubscribe(String),
    /// Publish data on a pub/sub topic.
    Publish { topic: String, data: Vec<u8> },
    /// Send a direct message to a peer.
    ///
    /// Delivery priority: direct connection → mailbox → peer_node_addr dial.
    /// When both `mailbox_addr` and `peer_node_addr` are `None` and the peer
    /// is not already connected, no delivery is attempted (mailbox-less + offline).
    SendDm {
        peer_id: String,
        message_id: String,
        recipient_x25519_pk: [u8; 32],
        envelope: DmEnvelope,
        /// Pre-resolved mailbox address from DhtHandle. `None` = no mailbox configured.
        mailbox_addr: Option<NodeAddr>,
        /// Peer's direct QUIC address from DHT peer info. Used for online-only delivery
        /// when the peer has no mailbox.
        peer_node_addr: Option<NodeAddr>,
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
    /// Inject an externally discovered listen address (e.g., from NAT traversal).
    AddListenAddr(String),
    /// Notify the event processor that a community was joined via the QUIC server.
    NotifyCommunityJoined([u8; 32], String),
    /// Fetch all queued DMs from a peer's mailbox.
    FetchMailbox { peer_id: String },
    /// Dial a set of DHT-discovered community peers and register them for gossip.
    ///
    /// The caller has already performed the DHT query via `DhtHandle`; this
    /// command just dials each returned peer and joins the community.
    DiscoverAndDial {
        peers: Vec<([u8; 32], NodeAddr)>,
        community_pk: [u8; 32],
        community_id: String,
    },
    /// Shut down the network handle.
    Shutdown,
}

// ── Events ───────────────────────────────────────────────────────────────────

/// Events emitted by the network layer to the application layer.
#[derive(Debug)]
pub enum NetworkEvent {
    /// A new peer connection was established (peer_id = hex-encoded Ed25519 public key).
    PeerConnected {
        peer_id: String,
        community_id: String,
    },
    /// A peer connected via mDNS/LAN without community context.
    /// The event processor will probe all joined communities with FetchManifest.
    LanPeerConnected { peer_id: String },
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
    CommunityJoinFailed {
        community_id: String,
        reason: String,
    },
    /// A `FetchManifest` request returned 404.
    ManifestNotFound {
        community_id: String,
        peer_id: String,
    },
    /// A seed peer connected for a specific community.
    SeedPeerConnected {
        community_id: String,
        peer_id: String,
    },
    /// A seed peer disconnected from a specific community.
    SeedPeerDisconnected { community_id: String },
    /// A DM could not be delivered (peer offline, no mailbox, and no reachable direct addr).
    DmSendFailed { peer_id: String, message_id: String },
    /// A gossip peer's QUIC address was resolved — used to seed the DHT routing table.
    PeerAddrKnown { node_pk: [u8; 32], addr: NodeAddr },
    /// A network-level error occurred.
    Error(String),
}

// ── Internal types ────────────────────────────────────────────────────────────

/// Sent from a dial task to the main loop once a connection is established.
pub(crate) struct PeerRegistration {
    pub(crate) peer_id: String,
    /// Raw Ed25519 public key bytes of the connected peer.
    pub(crate) node_pk: [u8; 32],
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
