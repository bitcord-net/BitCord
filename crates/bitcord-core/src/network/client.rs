//! High-level QUIC client for connecting to a BitCord node.
//!
//! # Auth flow
//! 1. Establish QUIC connection (TLS with cert-fingerprint pinning).
//! 2. Generate a fresh 32-byte random nonce.
//! 3. Sign the nonce with the client's Ed25519 signing key.
//! 4. Send `ClientRequest::Authenticate { pk, sig, nonce }`.
//! 5. Assert the server responds `NodeResponse::Authenticated`.
//! 6. Subscribe to the push-event channel.
//!
//! # Example
//! ```no_run
//! use std::sync::Arc;
//! use bitcord_core::{identity::NodeIdentity, network::{NodeAddr, client::NodeClient}};
//!
//! # async fn example() -> anyhow::Result<()> {
//! let identity = Arc::new(NodeIdentity::generate());
//! let addr: NodeAddr = "127.0.0.1:9000".parse()?;
//! let fingerprint = [0u8; 32]; // from invite link
//!
//! let (client, _node_pk, mut push_rx) = NodeClient::connect(addr, fingerprint, identity).await?;
//!
//! tokio::spawn(async move {
//!     while let Some(event) = push_rx.recv().await {
//!         println!("push: {event:?}");
//!     }
//! });
//! # Ok(())
//! # }
//! ```

use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result, bail};
use rand::RngCore as _;
use tokio::sync::mpsc;
use tracing::info;
use ulid::Ulid;

use crate::{
    crypto::{certificate::HostingCert, dm::DmEnvelope},
    identity::NodeIdentity,
    model::{channel::ChannelManifest, community::SignedManifest, membership::MembershipRecord},
    network::{
        NodeAddr,
        connection_manager::ConnectionManager,
        protocol::{ClientRequest, NodePush, NodeResponse},
    },
    state::message_log::LogEntry,
};
use bitcord_dht::CommunityPeerRecord;

/// Default channel capacity for the push-event receiver.
const PUSH_CHANNEL_CAPACITY: usize = 512;

// ── NodeClient ────────────────────────────────────────────────────────────────

/// High-level client for a single BitCord node connection.
///
/// Wraps [`ConnectionManager`] with application-level methods and handles
/// the authentication handshake on connect.
///
/// Clone is cheap (backed by `Arc`).
#[derive(Clone)]
pub struct NodeClient {
    mgr: Arc<ConnectionManager>,
}

impl NodeClient {
    /// Connect to a node, authenticate, and subscribe to push events.
    ///
    /// Returns `(NodeClient, node_pk, push_receiver)`.
    ///
    /// # Arguments
    /// * `addr`             — node address (IP + port)
    /// * `cert_fingerprint` — SHA-256 of the node's TLS certificate DER bytes
    /// * `identity`         — this client's cryptographic identity
    pub async fn connect(
        addr: NodeAddr,
        cert_fingerprint: [u8; 32],
        identity: Arc<NodeIdentity>,
    ) -> Result<(Self, [u8; 32], mpsc::Receiver<NodePush>)> {
        let socket_addr = SocketAddr::new(addr.ip, addr.port);
        let mgr = Arc::new(
            ConnectionManager::new(socket_addr, cert_fingerprint)
                .await
                .context("create connection manager")?,
        );

        // ── Authenticate ──────────────────────────────────────────────────
        let mut nonce = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce);

        let sig_bytes: [u8; 64] = identity.sign(&nonce).to_bytes();
        let pk: [u8; 32] = identity.verifying_key().to_bytes();
        let mut sig_r = [0u8; 32];
        let mut sig_s = [0u8; 32];
        sig_r.copy_from_slice(&sig_bytes[..32]);
        sig_s.copy_from_slice(&sig_bytes[32..]);

        let resp = mgr
            .request(&ClientRequest::Authenticate {
                pk,
                sig_r,
                sig_s,
                nonce,
            })
            .await
            .context("authentication request")?;

        let node_pk = match resp {
            NodeResponse::Authenticated { pk } => {
                info!("authenticated to node at {addr}");
                pk
            }
            NodeResponse::Error { code, msg } => {
                bail!("node rejected authentication (code={code}): {msg}");
            }
            other => {
                bail!("unexpected authentication response: {other:?}");
            }
        };

        // ── Subscribe to push events ──────────────────────────────────────
        let push_rx = Arc::clone(&mgr).subscribe_push(PUSH_CHANNEL_CAPACITY).await;

        Ok((Self { mgr }, node_pk, push_rx))
    }

    // ── Community ─────────────────────────────────────────────────────────

    /// Present a hosting certificate to join (or re-join) a community.
    pub async fn join_community(
        &self,
        cert: HostingCert,
        community_id: Option<String>,
        password: Option<String>,
    ) -> Result<()> {
        let resp = self
            .mgr
            .request(&ClientRequest::JoinCommunity {
                cert,
                community_id,
                password,
            })
            .await?;
        match resp {
            NodeResponse::Authenticated { .. } => Ok(()), // server reuses Authenticated as "ok"
            NodeResponse::Error { code, msg } => {
                bail!("join_community failed (code={code}): {msg}")
            }
            other => bail!("unexpected join_community response: {other:?}"),
        }
    }

    /// Fetch a full community manifest (including channels and keys).
    pub async fn fetch_manifest(
        &self,
        community_pk: [u8; 32],
    ) -> Result<(
        SignedManifest,
        Vec<ChannelManifest>,
        std::collections::HashMap<String, Vec<u8>>,
        Vec<MembershipRecord>,
    )> {
        let resp = self
            .mgr
            .request(&ClientRequest::FetchManifest { community_pk })
            .await?;
        match resp {
            NodeResponse::Manifest {
                manifest,
                channels,
                channel_keys,
                members,
            } => Ok((*manifest, channels, channel_keys, members)),
            NodeResponse::Error { code, msg } => {
                bail!("fetch_manifest failed (code={code}): {msg}")
            }
            other => bail!("unexpected fetch_manifest response: {other:?}"),
        }
    }

    // ── Messaging ─────────────────────────────────────────────────────────

    /// Send an encrypted message and return the assigned sequence number.
    pub async fn send_message(
        &self,
        community_pk: [u8; 32],
        channel_id: Ulid,
        nonce: [u8; 24],
        ciphertext: Vec<u8>,
    ) -> Result<u64> {
        let resp = self
            .mgr
            .request(&ClientRequest::SendMessage {
                community_pk,
                channel_id,
                nonce,
                ciphertext,
            })
            .await?;
        match resp {
            NodeResponse::MessageAck { seq } => Ok(seq),
            NodeResponse::Error { code, msg } => {
                bail!("send_message failed (code={code}): {msg}")
            }
            other => bail!("unexpected send_message response: {other:?}"),
        }
    }

    /// Retrieve messages in `[since_seq, ∞)` from a channel.
    pub async fn get_messages(
        &self,
        community_pk: [u8; 32],
        channel_id: Ulid,
        since_seq: u64,
    ) -> Result<Vec<LogEntry>> {
        let resp = self
            .mgr
            .request(&ClientRequest::GetMessages {
                community_pk,
                channel_id,
                since_seq,
            })
            .await?;
        match resp {
            NodeResponse::Messages { entries } => Ok(entries),
            NodeResponse::Error { code, msg } => {
                bail!("get_messages failed (code={code}): {msg}")
            }
            other => bail!("unexpected get_messages response: {other:?}"),
        }
    }

    // ── Direct messages ───────────────────────────────────────────────────

    /// Send a DM envelope and return the sequence number in the recipient's mailbox.
    pub async fn send_dm(&self, recipient_pk: [u8; 32], envelope: DmEnvelope) -> Result<u64> {
        let resp = self
            .mgr
            .request(&ClientRequest::SendDm {
                recipient_pk,
                envelope,
            })
            .await?;
        match resp {
            NodeResponse::DmAck { seq } => Ok(seq),
            NodeResponse::Error { code, msg } => {
                bail!("send_dm failed (code={code}): {msg}")
            }
            other => bail!("unexpected send_dm response: {other:?}"),
        }
    }

    /// Retrieve DMs in `[since_seq, ∞)` from this client's mailbox.
    pub async fn get_dms(&self, since_seq: u64) -> Result<Vec<LogEntry>> {
        let resp = self
            .mgr
            .request(&ClientRequest::GetDms { since_seq })
            .await?;
        match resp {
            NodeResponse::Dms { entries } => Ok(entries),
            NodeResponse::Error { code, msg } => {
                bail!("get_dms failed (code={code}): {msg}")
            }
            other => bail!("unexpected get_dms response: {other:?}"),
        }
    }

    // ── Distributed DHT ──────────────────────────────────────────────────

    /// Ask the remote node for its K closest peers to `target_id`.
    ///
    /// Also returns the mailbox address for `target_id` if the node holds it
    /// (Kademlia FIND_VALUE semantics).  Does not require prior authentication.
    pub async fn find_node(
        &self,
        target_id: [u8; 32],
    ) -> Result<(Vec<([u8; 32], NodeAddr)>, Option<NodeAddr>)> {
        let resp = self
            .mgr
            .request(&ClientRequest::FindNode { target_id })
            .await?;
        match resp {
            NodeResponse::ClosestPeers { peers, mailbox } => Ok((peers, mailbox)),
            NodeResponse::Error { code, msg } => {
                bail!("find_node failed (code={code}): {msg}")
            }
            other => bail!("unexpected find_node response: {other:?}"),
        }
    }

    /// Store a mailbox record on the remote node.
    ///
    /// Tells the node that `user_pk`'s mailbox is hosted at `addr`.
    /// Does not require prior authentication.
    pub async fn store_dht_record(&self, user_pk: [u8; 32], addr: NodeAddr) -> Result<()> {
        let resp = self
            .mgr
            .request(&ClientRequest::StoreDhtRecord { user_pk, addr })
            .await?;
        match resp {
            NodeResponse::DhtAck => Ok(()),
            NodeResponse::Error { code, msg } => {
                bail!("store_dht_record failed (code={code}): {msg}")
            }
            other => bail!("unexpected store_dht_record response: {other:?}"),
        }
    }

    // ── Community peer DHT ────────────────────────────────────────────────

    /// Store a community peer record on the remote node.
    ///
    /// Tells the node that `node_pk` is a member of `community_pk` and is
    /// reachable at `addr`.  Does not require prior authentication.
    pub async fn store_community_peer(
        &self,
        community_pk: [u8; 32],
        node_pk: [u8; 32],
        addr: NodeAddr,
    ) -> Result<()> {
        let resp = self
            .mgr
            .request(&ClientRequest::StoreCommunityPeer {
                community_pk,
                node_pk,
                addr,
            })
            .await?;
        match resp {
            NodeResponse::CommunityPeerAck => Ok(()),
            NodeResponse::Error { code, msg } => {
                bail!("store_community_peer failed (code={code}): {msg}")
            }
            other => bail!("unexpected store_community_peer response: {other:?}"),
        }
    }

    /// Ask the remote node for all known peers in `community_pk`.
    ///
    /// Does not require prior authentication.
    pub async fn find_community_peers(
        &self,
        community_pk: [u8; 32],
    ) -> Result<Vec<CommunityPeerRecord>> {
        let resp = self
            .mgr
            .request(&ClientRequest::FindCommunityPeers { community_pk })
            .await?;
        match resp {
            NodeResponse::CommunityPeers(records) => Ok(records),
            NodeResponse::Error { code, msg } => {
                bail!("find_community_peers failed (code={code}): {msg}")
            }
            other => bail!("unexpected find_community_peers response: {other:?}"),
        }
    }

    // ── Peer info DHT ─────────────────────────────────────────────────────

    /// Store a peer info record (x25519_pk + addr) on the remote node.
    ///
    /// `ed25519_pk` and `sig` are the announcer's public key and Ed25519 signature
    /// over `peer_id || x25519_pk`; the remote node verifies them before accepting.
    ///
    /// Does not require prior authentication.
    pub async fn store_peer_info(
        &self,
        peer_id: [u8; 32],
        ed25519_pk: [u8; 32],
        x25519_pk: [u8; 32],
        addr: NodeAddr,
        display_name: String,
        sig: [u8; 64],
    ) -> Result<()> {
        let mut sig_r = [0u8; 32];
        let mut sig_s = [0u8; 32];
        sig_r.copy_from_slice(&sig[..32]);
        sig_s.copy_from_slice(&sig[32..]);
        let resp = self
            .mgr
            .request(&ClientRequest::StorePeerInfo {
                peer_id,
                ed25519_pk,
                x25519_pk,
                addr,
                display_name,
                sig_r,
                sig_s,
            })
            .await?;
        match resp {
            NodeResponse::PeerInfoAck => Ok(()),
            NodeResponse::Error { code, msg } => {
                bail!("store_peer_info failed (code={code}): {msg}")
            }
            other => bail!("unexpected store_peer_info response: {other:?}"),
        }
    }

    /// Query a remote node for peer info (x25519_pk + addr) by `peer_id`.
    ///
    /// Returns `None` if the remote node has no record for this peer.
    /// Does not require prior authentication.
    pub async fn find_peer_info(
        &self,
        peer_id: [u8; 32],
    ) -> Result<Option<bitcord_dht::PeerInfoRecord>> {
        let resp = self
            .mgr
            .request(&ClientRequest::FindPeerInfo { peer_id })
            .await?;
        match resp {
            NodeResponse::PeerInfo {
                x25519_pk,
                addr,
                display_name,
            } => Ok(Some(bitcord_dht::PeerInfoRecord {
                x25519_pk,
                addr,
                announced_at: bitcord_dht::DhtState::unix_now(),
                display_name,
            })),
            NodeResponse::Error { code: 404, .. } => Ok(None),
            NodeResponse::Error { code, msg } => {
                bail!("find_peer_info failed (code={code}): {msg}")
            }
            other => bail!("unexpected find_peer_info response: {other:?}"),
        }
    }

    // ── Gossip relay ──────────────────────────────────────────────────────

    /// Relay a gossip message on a topic via this node.
    ///
    /// The node broadcasts the message to all of its authenticated clients,
    /// enabling pub/sub without a separate GossipSub layer.
    pub async fn send_gossip(&self, topic: String, data: Vec<u8>) -> Result<()> {
        let resp = self
            .mgr
            .request(&ClientRequest::Gossip { topic, data })
            .await?;
        match resp {
            NodeResponse::GossipAck => Ok(()),
            NodeResponse::Error { code, msg } => {
                bail!("send_gossip failed (code={code}): {msg}")
            }
            other => bail!("unexpected send_gossip response: {other:?}"),
        }
    }

    // ── Keep-alive ────────────────────────────────────────────────────────

    /// Send a heartbeat to prevent connection idle-timeout.
    ///
    /// A `NodeResponse::Authenticated` or any non-error response is accepted
    /// as a successful acknowledgement.
    pub async fn heartbeat(&self) -> Result<()> {
        let resp = self.mgr.request(&ClientRequest::Heartbeat).await?;
        match resp {
            NodeResponse::Error { code, msg } => {
                bail!("heartbeat error (code={code}): {msg}")
            }
            _ => Ok(()),
        }
    }

    // ── Seed-node promotion ──────────────────────────────────────────────

    /// Push full community metadata to a seed node.
    pub async fn push_manifest(
        &self,
        community_pk: [u8; 32],
        manifest: Box<SignedManifest>,
        channels: Vec<ChannelManifest>,
        channel_keys: std::collections::HashMap<String, Vec<u8>>,
        members: Vec<MembershipRecord>,
    ) -> Result<()> {
        let resp = self
            .mgr
            .request(&ClientRequest::PushManifest {
                community_pk,
                manifest,
                channels,
                channel_keys,
                members,
            })
            .await?;
        match resp {
            NodeResponse::PushAck => Ok(()),
            NodeResponse::Error { code, msg } => {
                bail!("push_manifest failed (code={code}): {msg}")
            }
            other => bail!("unexpected push_manifest response: {other:?}"),
        }
    }

    /// Push a batch of historical log entries for a channel to a seed node.
    pub async fn push_history(
        &self,
        community_pk: [u8; 32],
        channel_id: Ulid,
        entries: Vec<LogEntry>,
    ) -> Result<()> {
        let resp = self
            .mgr
            .request(&ClientRequest::PushHistory {
                community_pk,
                channel_id,
                entries,
            })
            .await?;
        match resp {
            NodeResponse::PushAck => Ok(()),
            NodeResponse::Error { code, msg } => {
                bail!("push_history failed (code={code}): {msg}")
            }
            other => bail!("unexpected push_history response: {other:?}"),
        }
    }
}
