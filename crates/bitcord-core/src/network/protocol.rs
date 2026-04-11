//! QUIC wire-protocol types for BitCord.
//!
//! All messages are serialised with `postcard` and framed with a 4-byte
//! big-endian length prefix:
//!
//! ```text
//! [ len: u32 BE ][ postcard payload: len bytes ]
//! ```
//!
//! Streams:
//! - **Request / response**: client opens a bidirectional QUIC stream, writes
//!   one `ClientRequest` frame, reads one `NodeResponse` frame, then closes.
//! - **Server push**: the node opens unidirectional streams to the client;
//!   each stream carries one `NodePush` frame.

use crate::{
    crypto::{certificate::HostingCert, dm::DmEnvelope},
    model::{channel::ChannelManifest, community::SignedManifest, membership::MembershipRecord},
    state::message_log::LogEntry,
};
use bitcord_dht::CommunityPeerRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ulid::Ulid;

use super::NodeAddr;

// ── Client → Node ─────────────────────────────────────────────────────────────

/// Requests sent from a client to a node over a fresh bidirectional QUIC stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientRequest {
    /// Prove ownership of an Ed25519 key.
    ///
    /// `nonce` is client-generated (32 random bytes).
    /// `sig`   = Ed25519 signature over `nonce` (64 bytes, stored as two 32-byte halves).
    /// `pk`    = the client's Ed25519 verifying key (32 bytes).
    ///
    /// The signature is split into two `[u8; 32]` fields because serde's derive
    /// macros only support fixed-size arrays up to 32 elements.
    Authenticate {
        pk: [u8; 32],
        /// First 32 bytes of the Ed25519 signature.
        sig_r: [u8; 32],
        /// Last 32 bytes of the Ed25519 signature.
        sig_s: [u8; 32],
        nonce: [u8; 32],
    },

    /// Join (or re-join) a community by presenting a valid hosting certificate.
    JoinCommunity {
        cert: HostingCert,
        community_id: Option<String>,
        /// Password required by private nodes.  `None` for open nodes.
        #[serde(default)]
        password: Option<String>,
    },

    /// Send an encrypted message to a channel.
    SendMessage {
        community_pk: [u8; 32],
        channel_id: Ulid,
        nonce: [u8; 24],
        ciphertext: Vec<u8>,
    },

    /// Retrieve messages in `[since_seq, ∞)` from a channel.
    GetMessages {
        community_pk: [u8; 32],
        channel_id: Ulid,
        since_seq: u64,
    },

    /// Send an encrypted direct message to another user.
    SendDm {
        recipient_pk: [u8; 32],
        envelope: DmEnvelope,
    },

    /// Retrieve direct messages in `[since_seq, ∞)` from this client's mailbox.
    GetDms { since_seq: u64 },

    /// Request a full community manifest from this node.
    ///
    /// Returns `NodeResponse::Manifest`. Used to discover all channels and
    /// obtain their symmetric keys (pre-E2EE: keys are plaintext in manifest).
    FetchManifest { community_pk: [u8; 32] },

    /// Ask the node for the K peers closest to `target_id` in its routing table.
    ///
    /// If the node also holds a mailbox record for `target_id`, it is returned
    /// in `NodeResponse::ClosestPeers::mailbox` — equivalent to Kademlia FIND_VALUE.
    /// Does not require authentication.
    FindNode { target_id: [u8; 32] },

    /// Store a mailbox routing record on this node.
    ///
    /// `user_pk` — the user whose mailbox is hosted at `addr`.
    /// `addr`    — the QUIC node address holding the mailbox.
    /// Does not require authentication.
    StoreDhtRecord { user_pk: [u8; 32], addr: NodeAddr },

    /// Keep-alive; no meaningful response expected.
    Heartbeat,

    /// Relay a gossip message on a topic to all authenticated clients of this node.
    ///
    /// The node broadcasts a `NodePush::GossipMessage` to every authenticated
    /// client so they can receive pub/sub events (channel messages, presence,
    /// manifest updates, etc.) without a separate GossipSub implementation.
    Gossip { topic: String, data: Vec<u8> },

    /// Push a full community manifest (channels, keys, members) to a seed node.
    ///
    /// Used during seed-node promotion to transfer community metadata before
    /// pushing channel history.
    PushManifest {
        community_pk: [u8; 32],
        manifest: Box<SignedManifest>,
        channels: Vec<ChannelManifest>,
        channel_keys: HashMap<String, Vec<u8>>,
        members: Vec<MembershipRecord>,
    },

    /// Push a batch of historical log entries for a single channel.
    ///
    /// Used during seed-node promotion to transfer message history.
    PushHistory {
        community_pk: [u8; 32],
        channel_id: Ulid,
        entries: Vec<LogEntry>,
    },

    /// Store a community peer record on this node.
    ///
    /// Announces that `node_pk` is reachable at `addr` and is a member of
    /// `community_pk`.  No authentication required — public DHT operation.
    StoreCommunityPeer {
        community_pk: [u8; 32],
        node_pk: [u8; 32],
        addr: NodeAddr,
    },

    /// Ask this node for all known peers in `community_pk`.
    ///
    /// Returns `NodeResponse::CommunityPeers`.
    /// No authentication required — public DHT operation.
    FindCommunityPeers { community_pk: [u8; 32] },

    /// Store a peer info record (x25519_pk + QUIC addr) keyed by `peer_id`.
    ///
    /// `peer_id`     = SHA-256 of the peer's Ed25519 verifying key.
    /// `ed25519_pk`  = the full Ed25519 verifying key (allows receivers to check
    ///                 SHA-256(ed25519_pk) == peer_id and verify the signature).
    /// `sig`         = Ed25519 signature over `peer_id || x25519_pk` (64 bytes,
    ///                 split into two 32-byte halves — same convention as Authenticate).
    /// No authentication required — public DHT operation.
    StorePeerInfo {
        peer_id: [u8; 32],
        ed25519_pk: [u8; 32],
        x25519_pk: [u8; 32],
        addr: NodeAddr,
        display_name: String,
        /// First 32 bytes of the Ed25519 signature over `peer_id || x25519_pk`.
        sig_r: [u8; 32],
        /// Last 32 bytes of the Ed25519 signature over `peer_id || x25519_pk`.
        sig_s: [u8; 32],
    },

    /// Find peer info (x25519_pk + QUIC addr) for a given `peer_id`.
    ///
    /// Returns `NodeResponse::PeerInfo` if found, `NodeResponse::Error { 404 }` otherwise.
    /// No authentication required — public DHT operation.
    FindPeerInfo { peer_id: [u8; 32] },
}

// ── Node → Client (response) ─────────────────────────────────────────────────

/// Responses sent from a node back to a client on the same stream as the request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NodeResponse {
    /// Authentication succeeded.
    ///
    /// `pk` is the node's own Ed25519 public key.
    Authenticated { pk: [u8; 32] },

    /// Message stored at sequence number `seq`.
    MessageAck { seq: u64 },

    /// Log entries from a channel (response to `GetMessages`).
    Messages { entries: Vec<LogEntry> },

    /// DM stored at sequence number `seq`.
    DmAck { seq: u64 },

    /// DM log entries (response to `GetDms`).
    Dms { entries: Vec<LogEntry> },

    /// Full manifest data for a community (response to `FetchManifest`).
    Manifest {
        manifest: Box<SignedManifest>,
        channels: Vec<ChannelManifest>,
        channel_keys: HashMap<String, Vec<u8>>,
        members: Vec<MembershipRecord>,
    },

    /// Closest known peers to the queried target (response to `FindNode`).
    ///
    /// `peers`   — up to K `(node_pk, NodeAddr)` pairs sorted by XOR distance to target.
    /// `mailbox` — the NodeAddr holding the mailbox for `target_id`, if this node has it.
    ClosestPeers {
        peers: Vec<([u8; 32], NodeAddr)>,
        mailbox: Option<NodeAddr>,
    },

    /// DHT record stored successfully (response to `StoreDhtRecord`).
    DhtAck,

    /// Gossip message accepted and relayed to peers.
    GossipAck,

    /// Community data or history batch accepted.
    PushAck,

    /// Community peer record stored successfully (response to `StoreCommunityPeer`).
    CommunityPeerAck,

    /// Known peers in a community (response to `FindCommunityPeers`).
    CommunityPeers(Vec<CommunityPeerRecord>),

    /// Peer info (x25519_pk + QUIC addr + display name) returned for a `FindPeerInfo` query.
    PeerInfo {
        x25519_pk: [u8; 32],
        addr: NodeAddr,
        display_name: String,
    },

    /// Acknowledgement for `StorePeerInfo`.
    PeerInfoAck,

    /// Error response; `code` mirrors HTTP semantics (400 = bad request, 403 = forbidden, …).
    Error { code: u16, msg: String },
}

// ── Node → Client (push) ──────────────────────────────────────────────────────

/// Unsolicited push messages sent by the node on unidirectional QUIC streams.
///
/// The node opens a new unidirectional stream per push event and writes a
/// single framed `NodePush` message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NodePush {
    /// A new message arrived in a channel the client is subscribed to.
    NewMessage { channel_id: Ulid, entry: LogEntry },

    /// A new direct message arrived in this client's mailbox.
    NewDm {
        entry: LogEntry,
        recipient_pk: [u8; 32],
    },

    /// A connected peer changed their presence status.
    ///
    /// `status`: 0 = offline, 1 = online, 2 = away, 3 = busy.
    Presence { user_pk: [u8; 32], status: u8 },

    /// A gossip message broadcast from another client of this node.
    ///
    /// `source` is the hex-encoded Ed25519 public key of the originating client.
    GossipMessage {
        topic: String,
        source: String,
        data: Vec<u8>,
    },
}

// ── Framing helpers ───────────────────────────────────────────────────────────

/// Encode a value as a length-prefixed postcard frame.
///
/// Frame layout: `[ len: u32 BE ][ postcard_bytes: len bytes ]`.
pub fn encode_frame<T: Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    let payload =
        postcard::to_allocvec(value).map_err(|e| anyhow::anyhow!("postcard encode: {e}"))?;
    let len = u32::try_from(payload.len())
        .map_err(|_| anyhow::anyhow!("payload too large: {} bytes", payload.len()))?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode a value from raw postcard bytes (no length prefix).
pub fn decode_payload<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> anyhow::Result<T> {
    postcard::from_bytes(payload).map_err(|e| anyhow::anyhow!("postcard decode: {e}"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticate_roundtrip() {
        let req = ClientRequest::Authenticate {
            pk: [1u8; 32],
            sig_r: [2u8; 32],
            sig_s: [3u8; 32],
            nonce: [4u8; 32],
        };
        let bytes = postcard::to_allocvec(&req).unwrap();
        let restored: ClientRequest = postcard::from_bytes(&bytes).unwrap();
        assert!(matches!(restored, ClientRequest::Authenticate { .. }));
    }

    #[test]
    fn authenticated_response_roundtrip() {
        let resp = NodeResponse::Authenticated { pk: [42u8; 32] };
        let bytes = postcard::to_allocvec(&resp).unwrap();
        let restored: NodeResponse = postcard::from_bytes(&bytes).unwrap();
        if let NodeResponse::Authenticated { pk } = restored {
            assert_eq!(pk, [42u8; 32]);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn messages_response_roundtrip() {
        let resp = NodeResponse::Messages {
            entries: vec![LogEntry {
                seq: 0,
                nonce: [0u8; 24],
                ciphertext: vec![1, 2, 3],
                message_id: "msg-1".into(),
                author_id: "author-1".into(),
                timestamp_ms: 1_000_000,
                deleted: false,
            }],
        };
        let bytes = postcard::to_allocvec(&resp).unwrap();
        let restored: NodeResponse = postcard::from_bytes(&bytes).unwrap();
        if let NodeResponse::Messages { entries } = restored {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].seq, 0);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn push_presence_roundtrip() {
        let push = NodePush::Presence {
            user_pk: [4u8; 32],
            status: 1,
        };
        let bytes = postcard::to_allocvec(&push).unwrap();
        let restored: NodePush = postcard::from_bytes(&bytes).unwrap();
        assert!(matches!(restored, NodePush::Presence { status: 1, .. }));
    }

    #[test]
    fn frame_encode_decode_roundtrip() {
        let req = ClientRequest::Heartbeat;
        let frame = encode_frame(&req).unwrap();

        // Verify framing layout
        assert!(frame.len() >= 4);
        let len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(frame.len(), 4 + len);

        let decoded: ClientRequest = decode_payload(&frame[4..]).unwrap();
        assert!(matches!(decoded, ClientRequest::Heartbeat));
    }

    #[test]
    fn send_message_roundtrip() {
        let req = ClientRequest::SendMessage {
            community_pk: [0u8; 32],
            channel_id: Ulid::new(),
            nonce: [5u8; 24],
            ciphertext: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let bytes = postcard::to_allocvec(&req).unwrap();
        let restored: ClientRequest = postcard::from_bytes(&bytes).unwrap();
        if let ClientRequest::SendMessage {
            nonce, ciphertext, ..
        } = restored
        {
            assert_eq!(nonce, [5u8; 24]);
            assert_eq!(ciphertext, [0xDE, 0xAD, 0xBE, 0xEF]);
        } else {
            panic!("wrong variant");
        }
    }
}
