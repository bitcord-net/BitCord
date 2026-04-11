use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::{
    crypto::certificate::HostingCert, identity::NodeIdentity, network::client::NodeClient,
};
use ulid::Ulid;

use super::reconnect::reconnect_seed_loop;
use super::types::{NetworkCommand, NetworkEvent, PeerRegistration, SeedPeerInfo, ServerPushTx};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_command(
    cmd: NetworkCommand,
    peers: &mut HashMap<String, NodeClient>,
    sha256_map: &mut HashMap<String, String>,
    seed_peers: &mut HashMap<String, SeedPeerInfo>,
    seed_reconnect_cancels: &mut HashMap<String, oneshot::Sender<()>>,
    identity: &Arc<NodeIdentity>,
    peer_reg_tx: &mpsc::Sender<PeerRegistration>,
    gossip_evt_tx: &mpsc::Sender<NetworkEvent>,
    own_pk_hex: &str,
    evt_tx: &mpsc::Sender<NetworkEvent>,
    server_push_tx: Option<&ServerPushTx>,
    own_addrs: &Arc<RwLock<HashSet<String>>>,
) -> bool {
    match cmd {
        NetworkCommand::Dial {
            addr,
            is_seed,
            join_community,
            join_community_password,
            cert_fingerprint,
        } => {
            let identity_clone = Arc::clone(identity);
            let reg_tx = peer_reg_tx.clone();
            let evt_fwd = gossip_evt_tx.clone();
            let own_pk = own_pk_hex.to_string();
            let evt_tx = evt_tx.clone();
            let own_addrs = Arc::clone(own_addrs);

            // Pre-create a cancel channel so the reconnect loop (if spawned on
            // dial failure) can be stopped later via NetworkCommand cancellation.
            // The Sender is stored immediately; if dial succeeds the Receiver is
            // dropped and the entry becomes a harmless closed channel.
            let cancel_rx = if is_seed {
                let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
                seed_reconnect_cancels.insert(addr.to_string(), cancel_tx);
                Some(cancel_rx)
            } else {
                None
            };

            tokio::spawn(async move {
                match NodeClient::connect(
                    addr.clone(),
                    cert_fingerprint,
                    Arc::clone(&identity_clone),
                )
                .await
                {
                    Ok((client, node_pk, push_rx)) => {
                        // Dial succeeded — drop the cancel receiver so the stored
                        // Sender closes cleanly. A new cancel channel will be
                        // created if this peer later drops and needs a reconnect loop.
                        drop(cancel_rx);

                        let peer_id = node_pk
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>();
                        let source = if is_seed {
                            "seed"
                        } else if join_community.is_some() {
                            "dht_or_manual"
                        } else {
                            "mdns_or_lan"
                        };
                        info!(%peer_id, %is_seed, source, "gossip: connected to remote peer");

                        let _ = reg_tx
                            .send(PeerRegistration {
                                peer_id: peer_id.clone(),
                                node_pk,
                                client: client.clone(),
                                is_seed,
                                addr,
                                push_rx,
                                evt_fwd,
                                own_pk,
                                join_community: join_community.clone(),
                                join_community_password: join_community_password.clone(),
                                cert_fingerprint,
                            })
                            .await;

                        if let Some((community_pk, community_id)) = join_community {
                            let sk = identity_clone.signing_key();
                            if sk.verifying_key().to_bytes() == community_pk {
                                let cert = HostingCert::new(&sk, node_pk, u64::MAX);
                                debug!(%peer_id, %community_id, "gossip: auto-joining seed to community");
                                if let Err(e) = client
                                    .join_community(
                                        cert,
                                        Some(community_id.clone()),
                                        join_community_password,
                                    )
                                    .await
                                {
                                    warn!(%peer_id, "gossip: auto-join failed: {e}");
                                    let _ = evt_tx
                                        .send(NetworkEvent::CommunityJoinFailed {
                                            community_id,
                                            reason: e.to_string(),
                                        })
                                        .await;
                                }
                            } else {
                                info!(%peer_id, "gossip: connected to seed as a member");
                            }
                        }
                    }
                    Err(e) => {
                        let source = if is_seed {
                            "seed"
                        } else if join_community.is_some() {
                            "dht_or_manual"
                        } else {
                            "mdns_or_lan"
                        };
                        warn!(%is_seed, source, "gossip: dial failed: {e:#}");
                        if is_seed {
                            let addr_str = addr.to_string();
                            if own_addrs.read().await.contains(&addr_str) {
                                drop(cancel_rx);
                                info!(%addr, "gossip: dial target is own address; skipping reconnect loop");
                                if let Some((_, community_id)) = join_community {
                                    // Self-hosted seed: no remote peer_id available.
                                    let _ = evt_tx
                                        .send(NetworkEvent::SeedPeerConnected {
                                            community_id,
                                            peer_id: String::new(),
                                        })
                                        .await;
                                }
                            } else {
                                if let Some((_, ref community_id)) = join_community {
                                    let _ = evt_tx
                                        .send(NetworkEvent::SeedPeerDisconnected {
                                            community_id: community_id.clone(),
                                        })
                                        .await;
                                }
                                // cancel_rx is Some here since is_seed is true
                                let cancel_rx = cancel_rx.expect("cancel_rx set for seeds");
                                info!(%addr, "scheduling reconnect loop after initial dial failure");
                                tokio::spawn(reconnect_seed_loop(
                                    addr,
                                    identity_clone,
                                    reg_tx,
                                    evt_fwd,
                                    own_pk,
                                    join_community,
                                    join_community_password,
                                    cert_fingerprint,
                                    own_addrs,
                                    cancel_rx,
                                ));
                            }
                        }
                    }
                }
            });
        }

        NetworkCommand::DiscoverAndDial {
            peers: discovered,
            community_pk,
            community_id,
        } => {
            // The caller has already done the DHT lookup; we just dial each peer.
            let own_pk_bytes = identity.verifying_key().to_bytes();
            for (node_pk, addr) in discovered {
                if node_pk == own_pk_bytes {
                    continue; // never dial ourselves
                }
                let identity_clone = Arc::clone(identity);
                let reg_tx = peer_reg_tx.clone();
                let evt_fwd = gossip_evt_tx.clone();
                let own_pk = own_pk_hex.to_string();
                let community_id2 = community_id.clone();
                tokio::spawn(async move {
                    match NodeClient::connect(addr.clone(), [0u8; 32], Arc::clone(&identity_clone))
                        .await
                    {
                        Ok((client, node_pk_bytes, push_rx)) => {
                            let peer_id = node_pk_bytes
                                .iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<String>();
                            info!(%peer_id, %addr, "dht discovery: connected to community peer");
                            let sk = identity_clone.signing_key();
                            if sk.verifying_key().to_bytes() == community_pk {
                                let cert = HostingCert::new(&sk, node_pk_bytes, u64::MAX);
                                if let Err(e) = client
                                    .join_community(cert, Some(community_id2.clone()), None)
                                    .await
                                {
                                    debug!(%peer_id, "dht discovery: auto-join failed: {e:#}");
                                }
                            } else {
                                let dummy_cert = HostingCert {
                                    community_pk,
                                    node_pk: node_pk_bytes,
                                    expires_at: u64::MAX,
                                    signature: ed25519_dalek::Signature::from_bytes(&[0u8; 64]),
                                };
                                if let Err(e) = client
                                    .join_community(dummy_cert, Some(community_id2.clone()), None)
                                    .await
                                {
                                    debug!(%peer_id, "dht discovery: member join failed: {e:#}");
                                }
                            }
                            let _ = reg_tx
                                .send(PeerRegistration {
                                    peer_id,
                                    node_pk: node_pk_bytes,
                                    client,
                                    is_seed: false,
                                    addr,
                                    push_rx,
                                    evt_fwd,
                                    own_pk,
                                    join_community: Some((community_pk, community_id2)),
                                    join_community_password: None,
                                    cert_fingerprint: [0u8; 32],
                                })
                                .await;
                        }
                        Err(e) => {
                            info!(%addr, "dht discovery: failed to connect: {e:#}");
                        }
                    }
                });
            }
        }

        NetworkCommand::Publish { topic, data } => {
            let mut dead = Vec::new();
            let mut peer_count = 0;
            for (peer_id, client) in peers.iter() {
                debug!(%peer_id, %topic, bytes = data.len(), "gossip: relaying publish to outbound peer");
                if let Err(e) = client.send_gossip(topic.clone(), data.clone()).await {
                    warn!(%peer_id, "gossip: publish failed: {e}");
                    dead.push(peer_id.clone());
                }
                peer_count += 1;
            }
            if peer_count == 0 {
                debug!(%topic, "gossip: publish skipped, no outbound peers connected");
            }
            for id in dead {
                peers.remove(&id);
                // Clean up the SHA256 reverse index so it doesn't grow unboundedly.
                sha256_map.retain(|_, v| peers.contains_key(v));
                let _ = evt_tx
                    .send(NetworkEvent::PeerDisconnected(id.clone()))
                    .await;
                if let Some((addr, join_community, join_community_password, cert_fingerprint)) =
                    seed_peers.remove(&id)
                {
                    if let Some((_, ref community_id)) = join_community {
                        let _ = evt_tx
                            .send(NetworkEvent::SeedPeerDisconnected {
                                community_id: community_id.clone(),
                            })
                            .await;
                    }
                    let identity_clone = Arc::clone(identity);
                    let reg_tx = peer_reg_tx.clone();
                    let evt_fwd = gossip_evt_tx.clone();
                    let own_pk = own_pk_hex.to_string();
                    info!(peer_id = %id, %addr, "seed peer lost; scheduling reconnect loop");
                    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
                    seed_reconnect_cancels.insert(addr.to_string(), cancel_tx);
                    tokio::spawn(reconnect_seed_loop(
                        addr,
                        identity_clone,
                        reg_tx,
                        evt_fwd,
                        own_pk,
                        join_community,
                        join_community_password,
                        cert_fingerprint,
                        Arc::clone(own_addrs),
                        cancel_rx,
                    ));
                }
            }
            if let Some(push_tx) = server_push_tx {
                debug!(%topic, "gossip: broadcasting to all authenticated server clients");
                let _ = push_tx.send((
                    None,
                    crate::network::protocol::NodePush::GossipMessage {
                        topic,
                        source: own_pk_hex.to_string(),
                        data,
                    },
                ));
            }
        }

        NetworkCommand::Subscribe(topic) => {
            debug!(%topic, "gossip: subscribe to topic");
        }

        NetworkCommand::Unsubscribe(topic) => {
            debug!(%topic, "gossip: unsubscribe from topic");
        }

        NetworkCommand::FetchManifest {
            peer_id,
            community_id,
            community_pk,
        } => {
            let Some(client) = peers.get(&peer_id).cloned() else {
                debug!(%peer_id, "gossip: fetch_manifest skipped: peer not in outbound table");
                return true;
            };
            let evt_tx = evt_tx.clone();
            let pid = peer_id.clone();
            tokio::spawn(async move {
                match client.fetch_manifest(community_pk).await {
                    Ok((manifest, channels, channel_keys, members)) => {
                        let _ = evt_tx
                            .send(NetworkEvent::ManifestReceived {
                                from: pid,
                                community_id,
                                manifest: Box::new(manifest),
                                channels,
                                channel_keys,
                                members,
                            })
                            .await;
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("404") || msg.contains("not found") {
                            let _ = evt_tx
                                .send(NetworkEvent::ManifestNotFound {
                                    community_id,
                                    peer_id: pid,
                                })
                                .await;
                        } else {
                            warn!(%peer_id, "gossip: fetch_manifest failed: {e}");
                        }
                    }
                }
            });
        }

        NetworkCommand::FetchChannelHistory {
            peer_id,
            community_id,
            community_pk,
            channel_id,
            since_seq,
        } => {
            let Some(client) = peers.get(&peer_id).cloned() else {
                warn!(%peer_id, "gossip: fetch_history failed: peer not found");
                return true;
            };
            let evt_tx = evt_tx.clone();
            let channel_ulid = match Ulid::from_string(&channel_id) {
                Ok(u) => u,
                Err(e) => {
                    warn!("gossip: fetch_history failed: invalid channel_id {channel_id}: {e}");
                    return true;
                }
            };
            let community_pk_bytes = community_pk;
            tokio::spawn(async move {
                let dummy_cert = HostingCert {
                    community_pk: community_pk_bytes,
                    node_pk: [0u8; 32],
                    expires_at: u64::MAX,
                    signature: ed25519_dalek::Signature::from_bytes(&[0u8; 64]),
                };
                let mut success = false;
                for attempt in 1..=15 {
                    match client
                        .join_community(dummy_cert.clone(), Some(community_id.clone()), None)
                        .await
                    {
                        Ok(_) => {
                            success = true;
                            break;
                        }
                        Err(e) if e.to_string().contains("invalid node join password") => {
                            warn!(%peer_id, "gossip: fetch_history join rejected: node requires a password");
                            break;
                        }
                        Err(e) if e.to_string().contains("403") => {
                            debug!(%peer_id, attempt, "gossip: fetch_history join 403, retrying in 1s...");
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                        Err(e) => {
                            warn!(%peer_id, "gossip: fetch_history join fatal error: {e}");
                            break;
                        }
                    }
                }
                if !success {
                    warn!(%peer_id, "gossip: fetch_history join failed after retries");
                    return;
                }
                match client
                    .get_messages(community_pk_bytes, channel_ulid, since_seq)
                    .await
                {
                    Ok(entries) => {
                        info!(count = entries.len(), %peer_id, channel_id, "gossip: received channel history");
                        let _ = evt_tx
                            .send(NetworkEvent::ChannelHistoryReceived {
                                community_id,
                                channel_id,
                                entries,
                            })
                            .await;
                    }
                    Err(e) => {
                        warn!(%peer_id, "gossip: fetch_history get_messages failed: {e}");
                    }
                }
            });
        }

        NetworkCommand::SendDm {
            peer_id,
            message_id,
            recipient_x25519_pk,
            envelope,
            mailbox_addr,
            peer_node_addr,
        } => {
            // Priority 1: already-connected direct peer.
            // peer_id may be SHA256(node_pk) — resolve to raw pk via the index first.
            let raw_pk = sha256_map
                .get(&peer_id)
                .map(|s| s.as_str())
                .unwrap_or(&peer_id);
            let direct_client = peers.get(raw_pk).cloned();
            if let Some(client) = direct_client {
                let evt_tx = evt_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = client.send_dm(recipient_x25519_pk, envelope).await {
                        warn!(%peer_id, "dm: direct send failed: {e}");
                        let _ = evt_tx
                            .send(NetworkEvent::DmSendFailed {
                                peer_id,
                                message_id,
                            })
                            .await;
                    }
                });
            } else if let Some(addr) = mailbox_addr {
                // Priority 2: pre-resolved mailbox address (store-and-forward).
                let identity_clone = Arc::clone(identity);
                let evt_tx = evt_tx.clone();
                tokio::spawn(async move {
                    match NodeClient::connect(addr.clone(), [0u8; 32], identity_clone).await {
                        Ok((client, _, _)) => {
                            if let Err(e) = client.send_dm(recipient_x25519_pk, envelope).await {
                                warn!(%peer_id, "dm: mailbox send failed: {e}");
                                let _ = evt_tx
                                    .send(NetworkEvent::DmSendFailed {
                                        peer_id,
                                        message_id,
                                    })
                                    .await;
                            }
                        }
                        Err(e) => {
                            warn!(%peer_id, "dm: connect to mailbox failed: {e}");
                            let _ = evt_tx
                                .send(NetworkEvent::DmSendFailed {
                                    peer_id,
                                    message_id,
                                })
                                .await;
                        }
                    }
                });
            } else if let Some(addr) = peer_node_addr {
                // Priority 3: DHT-discovered direct peer address (online-only delivery).
                let identity_clone = Arc::clone(identity);
                let evt_tx = evt_tx.clone();
                tokio::spawn(async move {
                    match NodeClient::connect(addr.clone(), [0u8; 32], identity_clone).await {
                        Ok((client, _, _)) => {
                            if let Err(e) = client.send_dm(recipient_x25519_pk, envelope).await {
                                warn!(%peer_id, "dm: direct-addr send failed: {e}");
                                let _ = evt_tx
                                    .send(NetworkEvent::DmSendFailed {
                                        peer_id,
                                        message_id,
                                    })
                                    .await;
                            }
                        }
                        Err(e) => {
                            warn!(%peer_id, "dm: connect to peer addr failed (peer likely offline): {e:#}");
                            let _ = evt_tx
                                .send(NetworkEvent::DmSendFailed {
                                    peer_id,
                                    message_id,
                                })
                                .await;
                        }
                    }
                });
            } else {
                // No mailbox and no known peer address — peer is mailbox-less and not online.
                warn!(%peer_id, "dm: no route to peer (no mailbox, not connected, no known addr)");
                let _ = evt_tx
                    .send(NetworkEvent::DmSendFailed {
                        peer_id,
                        message_id,
                    })
                    .await;
            }
        }

        NetworkCommand::AddListenAddr(addr) => {
            info!(%addr, "NAT: injecting externally discovered listen address");
            own_addrs.write().await.insert(addr.clone());
            let _ = evt_tx.send(NetworkEvent::NewListenAddr(addr)).await;
        }

        NetworkCommand::NotifyCommunityJoined(community_pk, community_id) => {
            let _ = evt_tx
                .send(NetworkEvent::CommunityJoined(community_pk, community_id))
                .await;
        }

        NetworkCommand::FetchMailbox { peer_id } => {
            if let Some(client) = peers.get(&peer_id).cloned() {
                let evt_fwd = evt_tx.clone();
                let our_x25519_pk = identity.x25519_public_key_bytes();
                tokio::spawn(async move {
                    match client.get_dms(0).await {
                        Ok(entries) => {
                            for entry in entries {
                                let _ = evt_fwd
                                    .send(NetworkEvent::DmReceived {
                                        recipient_pk: our_x25519_pk,
                                        entry,
                                    })
                                    .await;
                            }
                        }
                        Err(e) => debug!(%peer_id, "fetch_mailbox: get_dms failed: {e}"),
                    }
                });
            }
        }

        NetworkCommand::Shutdown => {
            info!("NetworkHandle: shutdown command received");
            return false;
        }
    }
    true
}
