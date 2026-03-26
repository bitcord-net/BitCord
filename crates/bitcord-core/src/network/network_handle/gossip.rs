use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info};

use crate::{
    identity::NodeIdentity,
    network::{node_addr::NodeAddr, protocol::NodePush},
    node::dht::Dht,
};

use super::command::handle_command;
use super::push_reader::push_reader;
use super::types::{NetworkCommand, NetworkEvent, PeerRegistration, SeedPeerInfo, ServerPushTx};

// ── Internal: gossip task ─────────────────────────────────────────────────────

pub(super) async fn gossip_task(
    identity: Arc<NodeIdentity>,
    local_listen_addrs: Vec<String>,
    mut cmd_rx: mpsc::Receiver<NetworkCommand>,
    evt_tx: mpsc::Sender<NetworkEvent>,
    server_push_tx: Option<ServerPushTx>,
    dht: Arc<Dht>,
) {
    info!("NetworkHandle gossip task started");

    // Emit local listen addresses so the app can build invite links.
    for addr in local_listen_addrs {
        let _ = evt_tx.send(NetworkEvent::NewListenAddr(addr)).await;
    }

    // Hex-encoded public key of this node — used to filter reflected gossip.
    let own_pk_hex: String = identity
        .verifying_key()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    // Channel for completed dial tasks to hand back the NodeClient.
    let (peer_reg_tx, mut peer_reg_rx) = mpsc::channel::<PeerRegistration>(64);

    // Channel for gossip events received from remote peer nodes.
    let (gossip_evt_tx, mut gossip_evt_rx) = mpsc::channel::<NetworkEvent>(512);

    // Active peer connections: peer_id → NodeClient.
    let mut peers: HashMap<String, crate::network::client::NodeClient> = HashMap::new();

    // Reverse map: NodeAddr → peer_id. Used to route DMs via DHT mailbox lookup.
    let mut peer_addrs: HashMap<NodeAddr, String> = HashMap::new();

    // Seed peer addresses for auto-reconnect: peer_id → (NodeAddr, Option<(community_pk, community_id)>).
    // When a seed peer drops, we spawn a reconnect loop so the embedded node
    // stays connected to always-on infrastructure.
    let mut seed_peers: HashMap<String, SeedPeerInfo> = HashMap::new();

    // Mailbox announcements that couldn't be propagated yet because no peers
    // were connected at announcement time.  Flushed to each new peer as it
    // registers so the preference reaches the network without waiting up to
    // an hour for the re-announcement loop.
    let mut pending_mailbox_announcements: Vec<([u8; 32], NodeAddr)> = Vec::new();

    // ── Inbound gossip relay task ─────────────────────────────────────────────
    // Subscribes to our own NodeServer's push channel to see gossip from
    // nodes that dialed into us.
    if let Some(push_tx) = &server_push_tx {
        let mut inbound_push_rx = push_tx.subscribe();
        let evt_fwd = gossip_evt_tx.clone();
        let own_pk = own_pk_hex.clone();
        tokio::spawn(async move {
            loop {
                match inbound_push_rx.recv().await {
                    Ok((
                        _,
                        NodePush::GossipMessage {
                            topic,
                            source,
                            data,
                        },
                    )) => {
                        // Skip messages we originally published (reflected back).
                        if source == own_pk {
                            continue;
                        }
                        let _ = evt_fwd
                            .send(NetworkEvent::MessageReceived {
                                topic,
                                source: Some(source),
                                data,
                            })
                            .await;
                    }
                    Ok((
                        _,
                        NodePush::NewDm {
                            entry,
                            recipient_pk,
                        },
                    )) => {
                        let _ = evt_fwd
                            .send(NetworkEvent::DmReceived {
                                entry,
                                recipient_pk,
                            })
                            .await;
                    }
                    Ok(_) => {} // Ignore other push types (NewMessage, etc).
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
        });
    }

    loop {
        tokio::select! {
            // ── New command from the application layer ────────────────────
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else {
                    info!("NetworkHandle: command channel closed");
                    break;
                };
                if !handle_command(
                    cmd,
                    &mut peers,
                    &mut seed_peers,
                    &peer_addrs,
                    &identity,
                    &peer_reg_tx,
                    &gossip_evt_tx,
                    &own_pk_hex,
                    &evt_tx,
                    server_push_tx.as_ref(),
                    &dht,
                    &mut pending_mailbox_announcements,
                )
                .await {
                    break;
                }
            }

            // ── Newly registered peer (dial completed) ────────────────────
            reg = peer_reg_rx.recv() => {
                if let Some(reg) = reg {
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        peers.entry(reg.peer_id.clone())
                    {
                        // Spawn push_reader only for the first connection per peer.
                        tokio::spawn(push_reader(
                            reg.push_rx,
                            reg.evt_fwd,
                            reg.peer_id.clone(),
                            reg.own_pk,
                        ));
                        // Record addr→peer_id for DHT mailbox routing.
                        peer_addrs.insert(reg.addr.clone(), reg.peer_id.clone());
                        // Track seed peer address so we can reconnect if it drops.
                        if reg.is_seed {
                            seed_peers.insert(reg.peer_id.clone(), (reg.addr, reg.join_community.clone(), reg.join_community_password, reg.cert_fingerprint));
                            info!(peer_id = %reg.peer_id, "seed peer connected");
                            if let Some((_, community_id)) = reg.join_community {
                                let _ = evt_tx
                                    .send(NetworkEvent::SeedPeerConnected { community_id })
                                    .await;
                            }
                        }
                        let _ = evt_tx
                            .send(NetworkEvent::PeerConnected(reg.peer_id.clone()))
                            .await;
                        let new_client = reg.client;
                        // Flush any mailbox announcements that were queued while
                        // no peers were connected (e.g. on startup).
                        if !pending_mailbox_announcements.is_empty() {
                            let to_flush = std::mem::take(&mut pending_mailbox_announcements);
                            let identity_clone = Arc::clone(&identity);
                            let client_clone = new_client.clone();
                            tokio::spawn(async move {
                                for (user_pk, addr) in to_flush {
                                    if let Err(e) =
                                        client_clone.store_dht_record(user_pk, addr).await
                                    {
                                        debug!("pending mailbox flush store_dht_record failed: {e}");
                                    }
                                }
                                drop(identity_clone);
                            });
                        }
                        e.insert(new_client);
                    } else {
                        // Duplicate connection to the same peer (e.g. multiple
                        // addresses from a multi-homed host). Drop the extras to
                        // prevent N-fold message delivery.
                        debug!(peer_id = %reg.peer_id, "gossip: duplicate connection dropped");
                        // Still emit SeedPeerConnected for the new community so
                        // the API layer marks it as reachable even when the seed
                        // was already connected for a different community.
                        if reg.is_seed {
                            if let Some((_, community_id)) = reg.join_community {
                                let _ = evt_tx
                                    .send(NetworkEvent::SeedPeerConnected { community_id })
                                    .await;
                            }
                        }
                    }
                }
            }

            // ── Gossip event forwarded from a remote peer ─────────────────
            evt = gossip_evt_rx.recv() => {
                if let Some(evt) = evt {
                    let _ = evt_tx.send(evt).await;
                }
            }
        }
    }

    info!("NetworkHandle gossip task stopped");
}
