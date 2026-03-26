mod command;
mod gossip;
mod kademlia;
mod push_reader;
mod reconnect;
mod types;

pub(crate) use types::ServerPushTx;
pub use types::{NetworkCommand, NetworkEvent};

use gossip::gossip_task;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::{identity::NodeIdentity, node::dht::Dht};

// ── NetworkHandle ─────────────────────────────────────────────────────────────

/// Real gossip-relay network handle.
///
/// Maintains QUIC connections to remote peer nodes and relays pub/sub messages
/// through them using the `ClientRequest::Gossip` / `NodePush::GossipMessage`
/// protocol extension.  Each connected remote node fans the message out to all
/// of its own authenticated clients, providing hub-and-spoke delivery.
///
/// # Dialing
/// `NetworkCommand::Dial` connects to a remote QUIC node using the provided
/// TLS certificate fingerprint for certificate pinning.  When the fingerprint
/// is all-zeros (`[0u8; 32]`) the connection operates in TOFU (Trust-On-First-Use)
/// mode, accepting any certificate — this is used for DHT/bootstrap connections
/// where the fingerprint is not yet known.
///
/// # Addressing
/// `local_listen_addrs` (if non-empty) are emitted as `NetworkEvent::NewListenAddr`
/// immediately after spawn so the application layer can populate invite links.
pub struct NetworkHandle;

impl NetworkHandle {
    /// Spawn the gossip-relay task and return `(cmd_tx, event_rx)`.
    ///
    /// * `identity` — this node's cryptographic identity; used to
    ///   authenticate outgoing QUIC connections.
    /// * `local_listen_addrs` — addresses the embedded QUIC node is bound to;
    ///   emitted as `NewListenAddr` events on startup.
    pub fn spawn(
        identity: Arc<NodeIdentity>,
        local_listen_addrs: Vec<String>,
        server_push_tx: Option<ServerPushTx>,
        dht: Arc<Dht>,
    ) -> (mpsc::Sender<NetworkCommand>, mpsc::Receiver<NetworkEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<NetworkCommand>(256);
        let evt_rx =
            Self::spawn_with_channel(identity, local_listen_addrs, cmd_rx, server_push_tx, dht);
        (cmd_tx, evt_rx)
    }

    /// Internal variant of spawn that uses an existing command channel.
    /// Enables solving circular dependencies between the QUIC server and Gossip layer.
    pub fn spawn_with_channel(
        identity: Arc<NodeIdentity>,
        local_listen_addrs: Vec<String>,
        cmd_rx: mpsc::Receiver<NetworkCommand>,
        server_push_tx: Option<ServerPushTx>,
        dht: Arc<Dht>,
    ) -> mpsc::Receiver<NetworkEvent> {
        let (evt_tx, evt_rx) = mpsc::channel::<NetworkEvent>(512);

        tokio::spawn(gossip_task(
            identity,
            local_listen_addrs,
            cmd_rx,
            evt_tx,
            server_push_tx,
            dht,
        ));

        evt_rx
    }
}
