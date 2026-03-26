use std::sync::Arc;

use tracing::info;

use super::super::AppState;
use super::super::types::PeerSummary;
use crate::{
    model::network_event::{ChannelManifestBroadcastPayload, NetworkEvent},
    network::NetworkCommand,
};

/// Handle a `PeerConnected` swarm event.
pub(super) async fn handle_peer_connected(state: &Arc<AppState>, peer_id: String) {
    info!(%peer_id, "peer connected");
    let peer_id_str = peer_id;
    {
        let mut peers = state.connected_peers.write().await;
        if !peers.iter().any(|p| p.peer_id == peer_id_str) {
            peers.push(PeerSummary {
                peer_id: peer_id_str.clone(),
                addresses: vec![],
                latency_ms: None,
                relay_capable: false,
                reputation: 0,
            });
        }
        state
            .metrics
            .connected_peers
            .store(peers.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }

    // Check pending manifest fetches for this peer.
    let pending_fetches = {
        let map = state.pending_manifest_syncs.lock().await;
        map.clone()
    };
    for community_id in pending_fetches {
        // Try to get the public key from our local state (placeholder or existing).
        let community_pk = {
            let comms = state.communities.read().await;
            comms.get(&community_id).map(|c| c.manifest.public_key)
        };
        if let Some(cpk) = community_pk {
            let _ = state
                .swarm_cmd_tx
                .send(NetworkCommand::FetchManifest {
                    peer_id: peer_id_str.clone(),
                    community_id,
                    community_pk: cpk,
                })
                .await;
        }
    }

    // For communities we already have manifests for (not in pending_manifest_syncs),
    // also request a fresh manifest fetch so we trigger history sync on reconnect.
    // Without this, pending_manifest_syncs is empty after the first successful sync
    // and FetchManifest is never sent on subsequent PeerConnected events, which
    // means FetchChannelHistory is never triggered and missed messages aren't received.
    {
        let pending = state.pending_manifest_syncs.lock().await;
        let comms = state.communities.read().await;
        let known_non_pending: Vec<(String, [u8; 32])> = comms
            .iter()
            .filter(|(id, _)| !pending.contains(*id))
            .map(|(id, signed)| (id.clone(), signed.manifest.public_key))
            .collect();
        drop(comms);
        drop(pending);
        for (community_id, cpk) in known_non_pending {
            let _ = state
                .swarm_cmd_tx
                .send(NetworkCommand::FetchManifest {
                    peer_id: peer_id_str.clone(),
                    community_id,
                    community_pk: cpk,
                })
                .await;
        }
    }

    // Fetch any DMs queued in the peer's mailbox while we were offline.
    let _ = state
        .swarm_cmd_tx
        .send(NetworkCommand::FetchMailbox {
            peer_id: peer_id_str.clone(),
        })
        .await;

    // Re-publish owned community manifests and channel broadcasts to the newly
    // connected peer.  This ensures relay nodes that were offline (or not yet
    // connected) when communities/channels were created receive the data they
    // need to serve FetchManifest to other members.
    //
    // We only do this when we are the community admin (our signing key == the
    // community public key) to avoid non-admin nodes spamming gossip.
    let own_pk = state.signing_key.verifying_key().to_bytes();
    let communities_snapshot = state.communities.read().await.clone();
    let channels_snapshot = state.channels.read().await.clone();
    let members_snapshot = state.members.read().await.clone();
    for (community_id_str, signed) in &communities_snapshot {
        if signed.manifest.public_key != own_pk {
            continue;
        }
        let topic = format!("/bitcord/community/{community_id_str}/1.0.0");
        // Push the current signed manifest so the relay updates its NodeStore.
        if let Ok(encoded) = NetworkEvent::ManifestUpdate(signed.clone()).encode() {
            let _ = state
                .swarm_cmd_tx
                .send(NetworkCommand::Publish {
                    topic: topic.clone(),
                    data: encoded,
                })
                .await;
        }
        // Push each channel manifest so the relay can serve wrapped keys.
        for ch_id in &signed.manifest.channel_ids {
            let ch_id_str = ch_id.to_string();
            if let Some(ch) = channels_snapshot.get(&ch_id_str) {
                if let Ok(encoded) =
                    NetworkEvent::ChannelManifestBroadcast(ChannelManifestBroadcastPayload {
                        manifest: ch.clone(),
                    })
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
        // Publish MemberJoined for all known members so relay/seed nodes
        // populate their NodeStore member list.  Without this, joiners
        // that fetch the manifest from a seed would get an empty member
        // list (the admin was never announced via MemberJoined gossip).
        if let Some(member_list) = members_snapshot.get(community_id_str) {
            for member in member_list.values() {
                if let Ok(encoded) = NetworkEvent::MemberJoined(member.clone()).encode() {
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
    let peer_id_str = peer_id;
    let mut peers = state.connected_peers.write().await;
    peers.retain(|p| p.peer_id != peer_id_str);
    state
        .metrics
        .connected_peers
        .store(peers.len() as u64, std::sync::atomic::Ordering::Relaxed);
}
