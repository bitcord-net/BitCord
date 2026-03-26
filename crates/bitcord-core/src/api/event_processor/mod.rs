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

use super::push_broadcaster::{CommunityEventData, PushEvent, SeedStatusData};
use super::{AppState, remove_community_local, save_table};
use crate::{
    model::{
        community::{CommunityManifest, SignedManifest},
        types::CommunityId,
    },
    network::{NetworkCommand, NetworkEvent as SwarmEvent},
};

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
            SwarmEvent::PeerConnected(peer_id) => {
                peer::handle_peer_connected(&state, peer_id).await;
            }

            SwarmEvent::PeerDisconnected(peer_id) => {
                peer::handle_peer_disconnected(&state, peer_id).await;
            }

            SwarmEvent::SeedPeerConnected { community_id } => {
                use tracing::info;
                info!(%community_id, "seed peer connected for community");
                state
                    .seed_connected_communities
                    .write()
                    .await
                    .insert(community_id.clone());
                state
                    .broadcaster
                    .send(PushEvent::SeedStatusChanged(SeedStatusData {
                        community_id,
                        connected: true,
                    }));
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
                use tracing::info;
                // A peer we queried no longer hosts this community.  If we are
                // not the community admin (owner), this means the community was
                // likely deleted while we were offline — remove it locally.
                let should_remove = {
                    let communities = state.communities.read().await;
                    match communities.get(&community_id) {
                        Some(signed) => {
                            let own_pk = state.signing_key.verifying_key().to_bytes();
                            signed.manifest.public_key != own_pk
                        }
                        None => false, // Already removed.
                    }
                };
                if should_remove {
                    info!(
                        %community_id,
                        %peer_id,
                        "peer returned 404 for community manifest; removing stale community"
                    );
                    remove_community_local(&state, &community_id).await;
                    state
                        .broadcaster
                        .send(PushEvent::CommunityDeleted(CommunityEventData {
                            community_id,
                            version: 0,
                            reason: "community no longer found on known peers".to_string(),
                        }));
                }
            }

            SwarmEvent::CommunityJoined(community_pk, id) => {
                use tracing::debug;
                let community_pk_hex = bs58::encode(community_pk).into_string();
                debug!(%community_pk_hex, %id, "community joined via RPC; ensuring topics and sync");

                // If we have the manifest in NodeStore, load it into state.communities.
                if let Some(store) = &state.node_store {
                    if let Ok(Some(meta)) = store.get_community_meta(&community_pk) {
                        let mut comms = state.communities.write().await;
                        if !comms.contains_key(&id) {
                            match meta.manifest {
                                Some(manifest) => {
                                    // Real manifest available — persist it.
                                    comms.insert(id.clone(), manifest);
                                    save_table(
                                        &state.data_dir.join("communities.json"),
                                        &*comms,
                                        state.encryption_key.as_ref(),
                                    );
                                }
                                None => {
                                    // No manifest yet. Insert an in-memory placeholder so that
                                    // PeerConnected can queue a FetchManifest using its public key.
                                    // Do NOT persist this to disk — the zeroed signature would be
                                    // invalid, and the real manifest will overwrite this entry
                                    // when it arrives via gossip.
                                    comms.insert(
                                        id.clone(),
                                        SignedManifest {
                                            manifest: CommunityManifest {
                                                id: ulid::Ulid::from_string(&id)
                                                    .map(CommunityId)
                                                    .unwrap_or_else(|_| CommunityId::new()),
                                                name: "Syncing...".to_string(),
                                                description: String::new(),
                                                public_key: community_pk,
                                                created_at: chrono::Utc::now(),
                                                admin_ids: vec![],
                                                channel_ids: vec![],
                                                seed_nodes: vec![],
                                                version: 0,
                                                deleted: false,
                                            },
                                            signature: vec![0u8; 64],
                                        },
                                    );
                                    let mut pending = state.pending_manifest_syncs.lock().await;
                                    if !pending.contains(&id) {
                                        pending.push(id.clone());
                                    }
                                }
                            }
                        }
                    }
                }

                // Subscribe to community topic regardless of whether we have the manifest yet.
                let topic = format!("/bitcord/community/{id}/1.0.0");
                let _ = state
                    .swarm_cmd_tx
                    .send(NetworkCommand::Subscribe(topic))
                    .await;
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
                            if let Some(fp) = &state.local_tls_fingerprint_hex {
                                use tracing::info;
                                info!(addr = %a, fingerprint = %fp, "public address discovered");
                            }
                        }
                    }
                }
            }

            _ => {}
        }
    }
}
