use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
    crypto::certificate::HostingCert,
    identity::NodeIdentity,
    network::{client::NodeClient, node_addr::NodeAddr},
    node::dht::Dht,
};
use ulid::Ulid;

use super::kademlia::kademlia_lookup;
use super::reconnect::reconnect_seed_loop;
use super::types::{NetworkCommand, NetworkEvent, PeerRegistration, SeedPeerInfo, ServerPushTx};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_command(
    cmd: NetworkCommand,
    peers: &mut HashMap<String, NodeClient>,
    seed_peers: &mut HashMap<String, SeedPeerInfo>,
    peer_addrs: &HashMap<NodeAddr, String>,
    identity: &Arc<NodeIdentity>,
    peer_reg_tx: &mpsc::Sender<PeerRegistration>,
    gossip_evt_tx: &mpsc::Sender<NetworkEvent>,
    own_pk_hex: &str,
    evt_tx: &mpsc::Sender<NetworkEvent>,
    server_push_tx: Option<&ServerPushTx>,
    dht: &Arc<Dht>,
    pending_mailbox_announcements: &mut Vec<([u8; 32], NodeAddr)>,
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
            tokio::spawn(async move {
                match NodeClient::connect(
                    addr.clone(),
                    cert_fingerprint,
                    Arc::clone(&identity_clone),
                )
                .await
                {
                    Ok((client, node_pk, push_rx)) => {
                        let peer_id = node_pk
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>();
                        info!(%peer_id, %is_seed, "gossip: connected to remote peer");

                        let _ = reg_tx
                            .send(PeerRegistration {
                                peer_id: peer_id.clone(),
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

                        // If a specific community join was requested (e.g. for a seed),
                        // issue a HostingCert and call JoinCommunity immediately.
                        if let Some((community_pk, community_id)) = join_community {
                            // We assume the caller (admin) knows that this node is authorized.
                            // For seeds dialed by the admin, the admin is the community_sk holder.
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
                                info!(%peer_id, "gossip: connected to seed as a member (HostingCert not issued — not the community admin)");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(%is_seed, "gossip: dial failed: {e:#}");
                        // For seed peers, schedule a retry loop so we stay
                        // connected to always-on infrastructure even if the
                        // initial attempt hit a transient error.
                        if is_seed {
                            // Notify the API layer immediately so the UI can
                            // mark the community unreachable. Without this the
                            // frontend never learns about the failure because
                            // SeedPeerDisconnected is only emitted for peers
                            // that were previously connected.
                            if let Some((_, ref community_id)) = join_community {
                                let _ = evt_tx
                                    .send(NetworkEvent::SeedPeerDisconnected {
                                        community_id: community_id.clone(),
                                    })
                                    .await;
                            }
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
                            ));
                        }
                    }
                }
            });
        }

        NetworkCommand::Publish { topic, data } => {
            // Relay to all outgoing peer connections (nodes we dialed).
            let mut dead = Vec::new();
            for (peer_id, client) in peers.iter() {
                if let Err(e) = client.send_gossip(topic.clone(), data.clone()).await {
                    warn!(%peer_id, "gossip: publish failed: {e}");
                    dead.push(peer_id.clone());
                }
            }
            for id in dead {
                peers.remove(&id);
                let _ = evt_tx
                    .send(NetworkEvent::PeerDisconnected(id.clone()))
                    .await;
                // If this was a seed peer, schedule an auto-reconnect loop so the
                // local cache node stays connected to always-on infrastructure.
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
                    tokio::spawn(reconnect_seed_loop(
                        addr,
                        identity_clone,
                        reg_tx,
                        evt_fwd,
                        own_pk,
                        join_community,
                        join_community_password,
                        cert_fingerprint,
                    ));
                }
            }
            // Also broadcast to all clients that dialed into our own QUIC server,
            // so that nodes which connected to us (without us dialing them back)
            // also receive the gossip message.
            if let Some(push_tx) = server_push_tx {
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
            debug!("gossip: subscribe {topic}");
        }

        NetworkCommand::Unsubscribe(topic) => {
            debug!("gossip: unsubscribe {topic}");
        }

        NetworkCommand::FetchManifest {
            peer_id,
            community_id,
            community_pk,
        } => {
            let Some(client) = peers.get(&peer_id).cloned() else {
                debug!(%peer_id, "gossip: fetch_manifest skipped: peer not in outbound table (inbound connection)");
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
                            // The peer no longer hosts this community — it may
                            // have been deleted while we were offline.
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
                // Join first to satisfy server-side session requirements.
                let dummy_cert = HostingCert {
                    community_pk: community_pk_bytes,
                    node_pk: [0u8; 32],
                    expires_at: u64::MAX,
                    signature: ed25519_dalek::Signature::from_bytes(&[0u8; 64]),
                };

                debug!(
                    community_pk = %bs58::encode(community_pk_bytes).into_string(),
                    %peer_id,
                    "requesting history: sending JoinCommunity (with retries)"
                );

                // Retry loop for the join step — the MemberJoined gossip might still be in flight.
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
                            // Password-protected node — retrying without a password will
                            // never succeed, so bail immediately instead of spamming the
                            // server with 15 doomed attempts.
                            warn!(
                                %peer_id,
                                "gossip: fetch_history join rejected: node requires a password"
                            );
                            break;
                        }
                        Err(e) if e.to_string().contains("403") => {
                            debug!(
                                %peer_id,
                                attempt,
                                "gossip: fetch_history join 403, retrying in 1s..."
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                        Err(e) => {
                            warn!(%peer_id, "gossip: fetch_history join fatal error: {e}");
                            break;
                        }
                    }
                }

                if !success {
                    warn!(
                        %peer_id,
                        community_pk = %bs58::encode(community_pk_bytes).into_string(),
                        "gossip: fetch_history join failed after retries"
                    );
                    return;
                }

                debug!(
                    community_pk = %bs58::encode(community_pk_bytes).into_string(),
                    channel_id,
                    %since_seq,
                    "gossip: history join success, requesting messages"
                );

                match client
                    .get_messages(community_pk_bytes, channel_ulid, since_seq)
                    .await
                {
                    Ok(entries) => {
                        info!(
                            count = entries.len(),
                            %peer_id,
                            channel_id,
                            "gossip: received channel history"
                        );
                        let _ = evt_tx
                            .send(NetworkEvent::ChannelHistoryReceived {
                                community_id,
                                channel_id,
                                entries,
                            })
                            .await;
                    }
                    Err(e) => {
                        warn!(
                            %peer_id,
                            community_pk = %bs58::encode(community_pk_bytes).into_string(),
                            "gossip: fetch_history get_messages failed: {e}"
                        );
                    }
                }
            });
        }

        NetworkCommand::SendDm {
            peer_id,
            recipient_x25519_pk,
            envelope,
        } => {
            // 1. Try the recipient directly (known outbound connection).
            // 2. Try local DHT mailbox lookup — resolves if the record is cached.
            // 3. Iterative Kademlia lookup — query the K closest known peers, asking
            //    each for closer nodes and/or the mailbox record.
            // 4. Fall back to any available seed peer for store-and-forward.
            let direct_client = peers.get(&peer_id).cloned().or_else(|| {
                dht.lookup_mailbox(&recipient_x25519_pk)
                    .and_then(|mailbox_addr| {
                        peer_addrs
                            .get(&mailbox_addr)
                            .and_then(|id| peers.get(id).cloned())
                    })
            });

            if let Some(client) = direct_client {
                tokio::spawn(async move {
                    if let Err(e) = client.send_dm(recipient_x25519_pk, envelope).await {
                        warn!(%peer_id, "dm: send failed: {e}");
                    }
                });
            } else {
                // No direct route — try iterative DHT lookup then fall back to seed.
                let dht_clone = Arc::clone(dht);
                let identity_clone = Arc::clone(identity);
                let seed_client = seed_peers
                    .keys()
                    .next()
                    .and_then(|id| peers.get(id).cloned());
                tokio::spawn(async move {
                    if let Some(mailbox_addr) = kademlia_lookup(
                        &recipient_x25519_pk,
                        &dht_clone,
                        Arc::clone(&identity_clone),
                    )
                    .await
                    {
                        // Connect directly to the mailbox-holding node.
                        match NodeClient::connect(
                            mailbox_addr,
                            [0u8; 32],
                            Arc::clone(&identity_clone),
                        )
                        .await
                        {
                            Ok((client, _, _)) => {
                                if let Err(e) = client.send_dm(recipient_x25519_pk, envelope).await
                                {
                                    warn!(%peer_id, "dm: kademlia route send failed: {e}");
                                }
                            }
                            Err(e) => {
                                warn!(%peer_id, "dm: could not connect to kademlia-resolved addr: {e}");
                            }
                        }
                    } else if let Some(seed) = seed_client {
                        // Last resort: store-and-forward via seed peer.
                        if let Err(e) = seed.send_dm(recipient_x25519_pk, envelope).await {
                            warn!(%peer_id, "dm: seed fallback send failed: {e}");
                        }
                    } else {
                        warn!(%peer_id, "dm: no route to peer (not connected, DHT empty)");
                    }
                });
            }
        }

        NetworkCommand::PropagateDhtRecord { user_pk, self_addr } => {
            // Send StoreDhtRecord to the K closest peers we know about.
            // This spreads the routing record so other nodes can find the mailbox
            // via iterative lookup even without a direct connection to this node.
            use crate::node::dht::NodeId;
            let closest = dht.closest_peers(&NodeId(user_pk), 20);
            for (_node_id, peer_addr) in closest {
                let identity_clone = Arc::clone(identity);
                let addr_copy = self_addr.clone();
                tokio::spawn(async move {
                    match NodeClient::connect(peer_addr, [0u8; 32], identity_clone).await {
                        Ok((client, _, _)) => {
                            if let Err(e) = client.store_dht_record(user_pk, addr_copy).await {
                                debug!("dht propagate store_dht_record failed: {e}");
                            }
                        }
                        Err(e) => {
                            debug!("dht propagate connect failed: {e}");
                        }
                    }
                });
            }
        }

        NetworkCommand::AnnouncePreferredMailbox { user_pk, addr } => {
            // 1. Update our own local DHT so incoming find_node queries return
            //    the preferred address immediately, even before propagation.
            dht.add_mailbox_record(user_pk, addr.clone());
            // 2. Propagate to the K closest peers so the rest of the network
            //    learns about the preferred mailbox node.
            use crate::node::dht::NodeId;
            let closest = dht.closest_peers(&NodeId(user_pk), 20);
            if closest.is_empty() {
                // No peers yet — queue for the next peer that connects.
                info!(
                    mailbox = %addr,
                    "DHT: no peers connected, queuing preferred mailbox announcement"
                );
                pending_mailbox_announcements.push((user_pk, addr));
            } else {
                info!(
                    peers = closest.len(),
                    mailbox = %addr,
                    "DHT: propagating preferred mailbox preference"
                );
                for (_node_id, peer_addr) in closest {
                    let identity_clone = Arc::clone(identity);
                    let addr_copy = addr.clone();
                    tokio::spawn(async move {
                        match NodeClient::connect(peer_addr, [0u8; 32], identity_clone).await {
                            Ok((client, _, _)) => {
                                if let Err(e) = client.store_dht_record(user_pk, addr_copy).await {
                                    debug!(
                                        "preferred mailbox propagate store_dht_record failed: {e}"
                                    );
                                }
                            }
                            Err(e) => {
                                debug!("preferred mailbox propagate connect failed: {e}");
                            }
                        }
                    });
                }
            }
        }

        NetworkCommand::AddListenAddr(addr) => {
            info!(%addr, "NAT: injecting externally discovered listen address");
            let _ = evt_tx.send(NetworkEvent::NewListenAddr(addr)).await;
        }

        NetworkCommand::NotifyCommunityJoined(community_pk, community_id) => {
            let _ = evt_tx
                .send(NetworkEvent::CommunityJoined(community_pk, community_id))
                .await;
        }

        NetworkCommand::FetchMailbox { peer_id } => {
            // Pull any queued DMs from the peer's mailbox and emit a DmReceived
            // event for each entry so handle_dm_received can decrypt and persist them.
            if let Some(client) = peers.get(&peer_id).cloned() {
                let evt_fwd = evt_tx.clone();
                // These entries are addressed to us — use our own X25519 public key
                // as the recipient_pk so handle_dm_received decrypts them.
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
