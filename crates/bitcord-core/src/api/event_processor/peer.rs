use std::sync::Arc;

use tracing::info;

use super::super::AppState;
use super::super::types::PeerSummary;
use crate::network::NetworkCommand;

/// Handle a `PeerConnected` swarm event.
pub(super) async fn handle_peer_connected(
    state: &Arc<AppState>,
    peer_id: String,
    community_id: String,
) {
    info!(%peer_id, %community_id, "community peer connected");

    // Determine whether this peer is the community admin (their Ed25519 pk ==
    // the community public key stored in the manifest).
    let is_admin = {
        let comms = state.communities.read().await;
        comms.get(&community_id).is_some_and(|signed| {
            let admin_pk_hex: String = signed
                .manifest
                .public_key
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            admin_pk_hex == peer_id
        })
    };

    {
        let mut peers_map = state.connected_peers.write().await;
        let list = peers_map.entry(community_id.clone()).or_default();
        if !list.iter().any(|p| p.peer_id == peer_id) {
            list.push(PeerSummary {
                peer_id: peer_id.clone(),
                addresses: vec![],
                latency_ms: None,
                relay_capable: false,
                reputation: 0,
                is_admin,
                community_id: community_id.clone(),
            });
        }
        // Update metrics: count unique peers across all communities.
        let total: usize = peers_map.values().map(|v| v.len()).sum();
        state
            .metrics
            .connected_peers
            .store(total as u64, std::sync::atomic::Ordering::Relaxed);
    }

    // Only fetch the manifest for the specific community this peer belongs to.
    let community_pk = {
        let comms = state.communities.read().await;
        comms.get(&community_id).map(|c| c.manifest.public_key)
    };

    if let Some(cpk) = community_pk {
        let in_pending = {
            let pending = state.pending_manifest_syncs.lock().await;
            pending.contains(&community_id)
        };
        // Whether pending or already synced, request a fresh manifest to
        // catch up on any missed messages since the last connection.
        let _ = state
            .swarm_cmd_tx
            .send(NetworkCommand::FetchManifest {
                peer_id: peer_id.clone(),
                community_id: community_id.clone(),
                community_pk: cpk,
            })
            .await;
        let _ = in_pending; // suppress unused warning
    }

    // Fetch any DMs queued in the peer's mailbox while we were offline.
    let _ = state
        .swarm_cmd_tx
        .send(NetworkCommand::FetchMailbox {
            peer_id: peer_id.clone(),
        })
        .await;

    // Kick off DHT community peer discovery and presence announcement for all
    // known communities via DhtHandle (non-blocking, spawned tasks).
    if let Some(dht) = &state.dht {
        let comms = state.communities.read().await.clone();
        for (cid, signed) in &comms {
            let cpk = signed.manifest.public_key;
            let cid2 = cid.clone();
            let dht2 = dht.clone();
            let cmd_tx = state.swarm_cmd_tx.clone();
            tokio::spawn(async move {
                let peers = dht2.find_community_peers(cpk).await.unwrap_or_default();
                if !peers.is_empty() {
                    let peer_addrs: Vec<([u8; 32], crate::network::NodeAddr)> =
                        peers.into_iter().map(|r| (r.node_pk, r.addr)).collect();
                    let _ = cmd_tx
                        .send(NetworkCommand::DiscoverAndDial {
                            peers: peer_addrs,
                            community_pk: cpk,
                            community_id: cid2,
                        })
                        .await;
                }
            });
            let dht3 = dht.clone();
            tokio::spawn(async move { dht3.register_community_peer(cpk).await });
        }
    }

    // Re-publish owned community manifests to the newly connected peer (admin only).
    let own_pk = state.signing_key.verifying_key().to_bytes();
    let communities_snapshot = state.communities.read().await.clone();
    let channels_snapshot = state.channels.read().await.clone();
    let members_snapshot = state.members.read().await.clone();
    for (community_id_str, signed) in &communities_snapshot {
        if signed.manifest.public_key != own_pk {
            continue;
        }
        let topic = format!("/bitcord/community/{community_id_str}/1.0.0");
        if let Ok(encoded) =
            crate::model::network_event::NetworkEvent::ManifestUpdate(signed.clone()).encode()
        {
            let _ = state
                .swarm_cmd_tx
                .send(NetworkCommand::Publish {
                    topic: topic.clone(),
                    data: encoded,
                })
                .await;
        }
        for ch_id in &signed.manifest.channel_ids {
            let ch_id_str = ch_id.to_string();
            if let Some(ch) = channels_snapshot.get(&ch_id_str) {
                if let Ok(encoded) =
                    crate::model::network_event::NetworkEvent::ChannelManifestBroadcast(
                        crate::model::network_event::ChannelManifestBroadcastPayload {
                            manifest: ch.clone(),
                        },
                    )
                    .encode()
                {
                    let _ = state
                        .swarm_cmd_tx
                        .send(NetworkCommand::Publish {
                            topic: topic.clone(),
                            data: encoded,
                        })
                        .await;
                }
            }
        }
        if let Some(member_list) = members_snapshot.get(community_id_str) {
            for member in member_list.values() {
                if let Ok(encoded) =
                    crate::model::network_event::NetworkEvent::MemberJoined(member.clone()).encode()
                {
                    let _ = state
                        .swarm_cmd_tx
                        .send(NetworkCommand::Publish {
                            topic: topic.clone(),
                            data: encoded,
                        })
                        .await;
                }
            }
        }
    }
}

/// Handle a `PeerDisconnected` swarm event.
pub(super) async fn handle_peer_disconnected(state: &Arc<AppState>, peer_id: String) {
    info!(%peer_id, "peer disconnected");
    let mut peers_map = state.connected_peers.write().await;
    for list in peers_map.values_mut() {
        list.retain(|p| p.peer_id != peer_id);
    }
    let total: usize = peers_map.values().map(|v| v.len()).sum();
    state
        .metrics
        .connected_peers
        .store(total as u64, std::sync::atomic::Ordering::Relaxed);
}
