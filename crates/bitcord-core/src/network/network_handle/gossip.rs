use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{debug, info};

use crate::{identity::NodeIdentity, network::protocol::NodePush};

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
) {
    info!("NetworkHandle gossip task started");

    // Track own addresses (local listen + STUN-discovered) so we can detect
    // and abort self-dials on self-hosted nodes.
    let own_addrs: Arc<RwLock<std::collections::HashSet<String>>> =
        Arc::new(RwLock::new(std::collections::HashSet::new()));

    // Emit local listen addresses so the app can build invite links, and seed
    // own_addrs so LAN self-dials are caught before STUN discovery.
    {
        let mut addrs = own_addrs.write().await;
        for addr in local_listen_addrs {
            addrs.insert(addr.clone());
            let _ = evt_tx.send(NetworkEvent::NewListenAddr(addr)).await;
        }
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

    // Active peer connections: raw_node_pk_hex → NodeClient.
    let mut peers: HashMap<String, crate::network::client::NodeClient> = HashMap::new();

    // Reverse index: SHA256(node_pk)_hex → raw_node_pk_hex.
    // Needed so SendDm can find connected peers by their application-layer peer_id.
    let mut sha256_map: HashMap<String, String> = HashMap::new();

    // Seed peer addresses for auto-reconnect: peer_id → (NodeAddr, Option<(community_pk, community_id)>).
    // When a seed peer drops, we spawn a reconnect loop so the embedded node
    // stays connected to always-on infrastructure.
    let mut seed_peers: HashMap<String, SeedPeerInfo> = HashMap::new();

    // Cancel senders for active seed reconnect loops, keyed by addr string.
    // When a seed peer drops, its cancel sender is stored here so the reconnect
    // loop can be stopped if needed in the future.
    let mut seed_reconnect_cancels: HashMap<String, tokio::sync::oneshot::Sender<()>> =
        HashMap::new();

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
                    &mut sha256_map,
                    &mut seed_peers,
                    &mut seed_reconnect_cancels,
                    &identity,
                    &peer_reg_tx,
                    &gossip_evt_tx,
                    &own_pk_hex,
                    &evt_tx,
                    server_push_tx.as_ref(),
                    &own_addrs,
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
                        // Seed the DHT routing table with this peer's address.
                        let _ = evt_tx
                            .send(NetworkEvent::PeerAddrKnown {
                                node_pk: reg.node_pk,
                                addr: reg.addr.clone(),
                            })
                            .await;

                        // Index SHA256(node_pk) → raw_pk_hex so SendDm can find
                        // connected peers when given an application-layer peer_id.
                        {
                            use sha2::{Digest, Sha256};
                            let sha256: [u8; 32] = Sha256::digest(reg.node_pk).into();
                            let sha256_hex: String =
                                sha256.iter().map(|b| format!("{b:02x}")).collect();
                            sha256_map.insert(sha256_hex, reg.peer_id.clone());
                        }

                        // Spawn push_reader only for the first connection per peer.
                        tokio::spawn(push_reader(
                            reg.push_rx,
                            reg.evt_fwd,
                            reg.peer_id.clone(),
                            reg.own_pk,
                        ));
                        // Track seed peer address so we can reconnect if it drops.
                        if reg.is_seed {
                            seed_peers.insert(reg.peer_id.clone(), (reg.addr.clone(), reg.join_community.clone(), reg.join_community_password.clone(), reg.cert_fingerprint));
                            info!(peer_id = %reg.peer_id, "seed peer connected");
                            if let Some((_, ref community_id)) = reg.join_community {
                                let _ = evt_tx
                                    .send(NetworkEvent::SeedPeerConnected {
                                        community_id: community_id.clone(),
                                        peer_id: reg.peer_id.clone(),
                                    })
                                    .await;
                            }
                        } else if let Some((_, ref community_id)) = reg.join_community {
                            let _ = evt_tx
                                .send(NetworkEvent::PeerConnected {
                                    peer_id: reg.peer_id.clone(),
                                    community_id: community_id.clone(),
                                })
                                .await;
                        } else {
                            // mDNS / LAN peer — community context unknown at dial time.
                            // The event processor will probe all joined communities.
                            let _ = evt_tx
                                .send(NetworkEvent::LanPeerConnected {
                                    peer_id: reg.peer_id.clone(),
                                })
                                .await;
                        }
                        e.insert(reg.client);
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
                                    .send(NetworkEvent::SeedPeerConnected {
                                        community_id,
                                        peer_id: reg.peer_id.clone(),
                                    })
                                    .await;
                            }
                        }
                    }
                }
            }

            // ── Gossip event forwarded from a remote peer ─────────────────
            evt = gossip_evt_rx.recv() => {
                if let Some(evt) = evt {
                    match &evt {
                        NetworkEvent::MessageReceived { topic, source, .. } => {
                            debug!(%topic, source = ?source, "gossip: message arrived from peer");
                        }
                        NetworkEvent::PeerConnected { peer_id, .. } => {
                            info!(%peer_id, "gossip: peer connection active");
                        }
                        NetworkEvent::PeerDisconnected(peer_id) => {
                            info!(%peer_id, "gossip: peer disconnected");
                        }
                        _ => {}
                    }
                    let _ = evt_tx.send(evt).await;
                }
            }
        }
    }

    info!("NetworkHandle gossip task stopped");
}
