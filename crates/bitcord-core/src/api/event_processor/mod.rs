//! Swarm event processor — routes `SwarmEvent`s from the P2P swarm to
//! `AppState` mutations and push events for connected clients.
//!
//! Extracted from `bitcord-tauri` so that both the Tauri app and the headless
//! `bitcord-node` binary share the same event-handling logic.

mod addr;
mod channel_handler;
mod community_handler;
mod dm_handler;
mod manifest_handler;
mod peer;

use std::sync::Arc;

use tokio::sync::mpsc;

use super::push_broadcaster::{CommunityEventData, DmSendFailedData, PushEvent, SeedStatusData};
use super::{AppState, remove_community_local};
use crate::network::{NetworkCommand, NetworkEvent as SwarmEvent};

use addr::{expand_wildcard_addr, is_publicly_routable};

// ── Public API ────────────────────────────────────────────────────────────────

/// Reads events from the swarm runner and updates `AppState` accordingly.
///
/// Handles:
/// - `PeerConnected`/`PeerDisconnected` — maintains the `connected_peers` snapshot
/// - `DmReceived` — decodes the DM payload, persists it, and emits `PushEvent::DmNew`
/// - `MessageReceived` on channel topics — decodes `NetworkEvent`, decrypts, appends to CRDT
/// - `MessageReceived` on community topics — handles manifest updates and member joins
/// - `ManifestReceived` — applies community manifest updates received from peers
/// - `ChannelHistoryReceived` — handles channel history received from peers
/// - `NewListenAddr` — records actual listen addresses for invite link generation
pub async fn process_swarm_events(mut event_rx: mpsc::Receiver<SwarmEvent>, state: Arc<AppState>) {
    while let Some(event) = event_rx.recv().await {
        match event {
            SwarmEvent::PeerConnected {
                peer_id,
                community_id,
            } => {
                peer::handle_peer_connected(&state, peer_id, community_id).await;
            }

            SwarmEvent::LanPeerConnected { peer_id } => {
                // Probe every joined community — if the LAN peer hosts it, ManifestReceived
                // will fire, register the peer into connected_peers, and kick off history sync.
                let communities: Vec<(String, [u8; 32])> = {
                    let comms = state.communities.read().await;
                    comms
                        .iter()
                        .map(|(id, s)| (id.clone(), s.manifest.public_key))
                        .collect()
                };
                for (community_id, community_pk) in communities {
                    let _ = state
                        .swarm_cmd_tx
                        .send(crate::network::NetworkCommand::FetchManifest {
                            peer_id: peer_id.clone(),
                            community_id,
                            community_pk,
                        })
                        .await;
                }
            }

            SwarmEvent::PeerDisconnected(peer_id) => {
                peer::handle_peer_disconnected(&state, peer_id).await;
            }

            SwarmEvent::SeedPeerConnected {
                community_id,
                peer_id,
            } => {
                use tracing::info;
                info!(%community_id, %peer_id, "seed peer connected for community");
                state
                    .seed_connected_communities
                    .write()
                    .await
                    .insert(community_id.clone());
                state
                    .broadcaster
                    .send(PushEvent::SeedStatusChanged(SeedStatusData {
                        community_id: community_id.clone(),
                        connected: true,
                    }));

                // Fetch the community manifest from the seed peer so we pick up
                // any channels / members that were created while we were offline.
                if !peer_id.is_empty() {
                    let community_pk = {
                        let comms = state.communities.read().await;
                        comms.get(&community_id).map(|s| s.manifest.public_key)
                    };
                    if let Some(cpk) = community_pk {
                        let _ = state
                            .swarm_cmd_tx
                            .send(NetworkCommand::FetchManifest {
                                peer_id: peer_id.clone(),
                                community_id: community_id.clone(),
                                community_pk: cpk,
                            })
                            .await;
                    }
                    // Also fetch any DMs queued in the seed's mailbox.
                    let _ = state
                        .swarm_cmd_tx
                        .send(NetworkCommand::FetchMailbox { peer_id })
                        .await;
                }
            }

            SwarmEvent::SeedPeerDisconnected { community_id } => {
                use tracing::info;
                info!(%community_id, "seed peer disconnected for community");
                state
                    .seed_connected_communities
                    .write()
                    .await
                    .remove(&community_id);
                state
                    .broadcaster
                    .send(PushEvent::SeedStatusChanged(SeedStatusData {
                        community_id,
                        connected: false,
                    }));
            }

            SwarmEvent::DmReceived {
                entry,
                recipient_pk,
            } => {
                dm_handler::handle_dm_received(&state, entry, recipient_pk).await;
            }

            SwarmEvent::MessageReceived {
                topic,
                data,
                source,
            } => {
                if topic.starts_with("/bitcord/channel/") {
                    channel_handler::handle_channel_message(&state, topic, data).await;
                } else if topic.starts_with("/bitcord/community/") {
                    community_handler::handle_community_message(&state, topic, data, source).await;
                }
            }

            SwarmEvent::ManifestReceived {
                from,
                community_id,
                manifest,
                channels,
                channel_keys,
                members,
            } => {
                manifest_handler::handle_manifest_received(
                    &state,
                    from,
                    community_id,
                    *manifest,
                    channels,
                    channel_keys,
                    members,
                )
                .await;
            }

            SwarmEvent::ChannelHistoryReceived {
                community_id,
                channel_id,
                entries,
            } => {
                channel_handler::handle_channel_history_received(
                    &state,
                    community_id,
                    channel_id,
                    entries,
                )
                .await;
            }

            SwarmEvent::ManifestNotFound {
                community_id,
                peer_id,
            } => {
                use tracing::debug;
                // A peer we queried does not host this community.  This does NOT
                // mean the community is deleted — the queried peer may be a global
                // bootstrap/seed node that never hosted the manifest (only the admin
                // node or dedicated seed nodes hold it).  Removing the community here
                // based on a single 404 causes data loss on every restart when the
                // first connected peer happens to be a bootstrap node.
                //
                // Instead, kick off a DHT peer discovery pass so we can locate and
                // connect to the node that actually hosts the manifest.
                debug!(
                    %community_id,
                    %peer_id,
                    "peer returned 404 for community manifest; triggering DHT discovery"
                );
                let community_pk = {
                    let communities = state.communities.read().await;
                    communities
                        .get(&community_id)
                        .map(|s| s.manifest.public_key)
                };
                if let Some(cpk) = community_pk {
                    if let Some(dht) = &state.dht {
                        let dht = dht.clone();
                        let cmd_tx = state.swarm_cmd_tx.clone();
                        tokio::spawn(async move {
                            let peers = dht.find_community_peers(cpk).await.unwrap_or_default();
                            if !peers.is_empty() {
                                let peer_addrs: Vec<([u8; 32], crate::network::NodeAddr)> =
                                    peers.into_iter().map(|r| (r.node_pk, r.addr)).collect();
                                let _ = cmd_tx
                                    .send(NetworkCommand::DiscoverAndDial {
                                        peers: peer_addrs,
                                        community_pk: cpk,
                                        community_id,
                                    })
                                    .await;
                            }
                        });
                    }
                }
            }

            SwarmEvent::CommunityJoined(community_pk, id) => {
                community_handler::handle_community_joined(&state, community_pk, id).await;
            }

            SwarmEvent::CommunityJoinFailed {
                community_id,
                reason,
            } => {
                use tracing::warn;
                warn!(
                    %community_id,
                    %reason,
                    "removing community after seed join failure"
                );
                remove_community_local(&state, &community_id).await;
                state
                    .broadcaster
                    .send(PushEvent::CommunityDeleted(CommunityEventData {
                        community_id,
                        version: 0,
                        reason,
                    }));
            }

            SwarmEvent::NewListenAddr(addr) => {
                // Expand wildcard addresses (0.0.0.0 / ::) to real interface IPs so that
                // invite links contain routable addresses when shared across machines.
                let peer_suffix = format!("/p2p/{}", state.node_address);
                let expanded = expand_wildcard_addr(&addr, &peer_suffix);
                let mut addrs = state.actual_listen_addrs.write().await;
                let mut newly_public = false;
                for a in &expanded {
                    if !addrs.contains(a) {
                        addrs.push(a.clone());
                    }
                    // If this is a publicly routable address (e.g. from STUN/UPnP), record
                    // it as the canonical public endpoint used in invite links.
                    if is_publicly_routable(a) {
                        let mut public = state.public_addr.write().await;
                        if public.is_none() {
                            *public = Some(a.clone());
                            newly_public = true;
                            if let Some(fp) = &state.local_tls_fingerprint_hex {
                                use tracing::info;
                                info!(addr = %a, fingerprint = %fp, "public address discovered");
                            }
                        }
                    }
                }
                // On first public address discovery, announce this node's presence in
                // every community it belongs to via DhtHandle.  This is needed for
                // Tauri embedded nodes whose DHT self_addr is None at startup — it gets
                // populated when the STUN-discovered address fires AddListenAddr.
                if newly_public {
                    drop(addrs); // release write lock before acquiring communities read lock
                    if let Some(dht) = &state.dht {
                        let comms = state.communities.read().await.clone();
                        for signed in comms.values() {
                            let cpk = signed.manifest.public_key;
                            let dht2 = dht.clone();
                            tokio::spawn(async move { dht2.register_community_peer(cpk).await });
                        }
                        // Also re-announce peer info now that self_addr is known.
                        let sk_bytes = state.signing_key.to_bytes();
                        let identity =
                            crate::identity::NodeIdentity::from_signing_key_bytes(&sk_bytes);
                        let peer_id_bytes = *identity.to_peer_id().as_bytes();
                        let x25519_pk = identity.x25519_public_key_bytes();
                        let display_name = state
                            .config
                            .read()
                            .await
                            .display_name
                            .clone()
                            .unwrap_or_default();
                        let dht3 = dht.clone();
                        tokio::spawn(async move {
                            dht3.register_peer_info(peer_id_bytes, x25519_pk, display_name)
                                .await;
                        });
                    }
                }
            }

            SwarmEvent::DmSendFailed {
                peer_id,
                message_id,
            } => {
                use tracing::debug;
                debug!(%peer_id, %message_id, "dm delivery failed: peer offline, no mailbox, no reachable addr");
                state
                    .broadcaster
                    .send(PushEvent::DmSendFailed(DmSendFailedData {
                        peer_id,
                        message_id,
                    }));
            }

            SwarmEvent::PeerAddrKnown { node_pk, addr } => {
                // Seed the DHT routing table so Kademlia walks have a starting point.
                if let Some(dht) = &state.dht {
                    use tracing::info;
                    info!(%addr, "DHT: seeded routing table from gossip peer");
                    dht.add_known_peer(node_pk, addr);

                    // Eagerly re-announce our own peer info to this new peer.
                    // This fixes the startup race where register_peer_info fires
                    // before bootstrap populates the routing table, causing our
                    // x25519 key + address to not reach the new peer for 3600 s.
                    let dht_clone = Arc::clone(dht);
                    let signing_key_bytes = state.signing_key.to_bytes();
                    let config_clone = Arc::clone(&state.config);
                    tokio::spawn(async move {
                        use crate::identity::NodeIdentity;
                        let ni = NodeIdentity::from_signing_key_bytes(&signing_key_bytes);
                        let peer_id = *ni.to_peer_id().as_bytes();
                        let x25519_pk = ni.x25519_public_key_bytes();
                        let display_name = config_clone
                            .read()
                            .await
                            .display_name
                            .clone()
                            .unwrap_or_default();
                        dht_clone
                            .register_peer_info(peer_id, x25519_pk, display_name)
                            .await;
                    });
                }
            }

            _ => {}
        }
    }
}
