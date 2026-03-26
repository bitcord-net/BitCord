use tokio::sync::mpsc;
use tracing::debug;

use crate::network::protocol::NodePush;

use super::types::NetworkEvent;

/// Reads `NodePush` events from a remote peer's push channel and translates
/// them into `NetworkEvent`s, filtering out messages originating from ourselves
/// (reflected back after we published them).
pub(crate) async fn push_reader(
    mut push_rx: mpsc::Receiver<NodePush>,
    evt_tx: mpsc::Sender<NetworkEvent>,
    peer_id: String,
    own_pk_hex: String,
) {
    while let Some(push) = push_rx.recv().await {
        let evt = match push {
            NodePush::GossipMessage {
                topic,
                source,
                data,
            } => {
                // Skip messages we originally published (reflected back).
                if source == own_pk_hex {
                    continue;
                }
                NetworkEvent::MessageReceived {
                    topic,
                    source: Some(source),
                    data,
                }
            }
            NodePush::NewDm {
                entry,
                recipient_pk,
            } => NetworkEvent::DmReceived {
                entry,
                recipient_pk,
            },
            _ => continue,
        };

        if evt_tx.send(evt).await.is_err() {
            break;
        }
    }

    debug!(%peer_id, "gossip: push reader for remote peer exited");
}
