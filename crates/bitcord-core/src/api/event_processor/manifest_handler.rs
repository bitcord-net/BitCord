use std::collections::HashMap;

use ed25519_dalek::Signer as _;
use tracing::{debug, info, warn};

use super::super::push_broadcaster::{CommunityEventData, PushEvent};
use super::super::{AppState, DmPeerInfo, remove_community_local, save_table};
use crate::{
    crypto::channel_keys::ChannelKey,
    identity::NodeIdentity,
    model::{
        channel::ChannelManifest,
        community::SignedManifest,
        membership::{MembershipRecord, Role},
        network_event::{ChannelManifestBroadcastPayload, NetworkEvent},
        types::UserId,
    },
    network::NetworkCommand,
};

pub(super) async fn handle_manifest_received(
    state: &AppState,
    from: String,
    _community_id: String,
    manifest: SignedManifest,
    channels: Vec<ChannelManifest>,
    channel_keys: HashMap<String, Vec<u8>>,
    peer_members: Vec<MembershipRecord>,
) {
    // Verify the manifest signature before accepting it.
    if !manifest.verify() {
        warn!(
            "received invalid (unverifiable) manifest for community {}; discarding",
            manifest.manifest.id
        );
        return;
    }

    let community_id_str = manifest.manifest.id.to_string();
    let community_id_typed = manifest.manifest.id.clone(); // capture before manifest is moved
    let version = manifest.manifest.version;

    // Remove from pending syncs list if version is > 0.
    if version > 0 {
        let mut pending = state.pending_manifest_syncs.lock().await;
        pending.retain(|id| id != &community_id_str);
    }

    // If the manifest is a deletion tombstone, remove the community locally
    // and notify the frontend.  This handles the case where a node was offline
    // when the deletion gossip was broadcast and discovers the deletion by
    // querying a peer that still holds the tombstone manifest.
    if manifest.manifest.deleted {
        remove_community_local(state, &community_id_str).await;
        state
            .broadcaster
            .send(PushEvent::CommunityDeleted(CommunityEventData {
                community_id: community_id_str,
                version,
                reason: String::new(),
            }));
        return;
    }

    // Replace the placeholder manifest with the real one.
    {
        let mut communities = state.communities.write().await;
        communities.insert(community_id_str.clone(), manifest.clone());
        save_table(
            &state.data_dir.join("communities.json"),
            &*communities,
            state.encryption_key.as_ref(),
        );
    }

    // Store received channel manifests and evict any locally cached channels
    // for this community that are no longer in the manifest.  Without this,
    // a node that was offline during a deletion keeps the stale channel in
    // channels.json indefinitely — FetchManifest never re-delivers deleted IDs.
    let live_ids: std::collections::HashSet<String> = manifest
        .manifest
        .channel_ids
        .iter()
        .map(|id| id.to_string())
        .collect();
    let channel_ids: Vec<String> = channels.iter().map(|c| c.id.to_string()).collect();
    {
        let mut ch_store = state.channels.write().await;
        for ch in &channels {
            ch_store.insert(ch.id.to_string(), ch.clone());
        }
        ch_store.retain(|id, ch| {
            ch.community_id.to_string() != community_id_str || live_ids.contains(id)
        });
        save_table(
            &state.data_dir.join("channels.json"),
            &*ch_store,
            state.encryption_key.as_ref(),
        );
    }

    // Store received channel keys.
    //
    // Primary path (E2EE): decrypt per-member wrapped keys from each
    // ChannelManifest's `encrypted_channel_key` field.  Each entry is an
    // X25519-ECDH + XChaCha20-Poly1305 ciphertext produced by the hosting node
    // specifically for this client's public key.
    //
    // Fallback path (legacy / same-process): accept plaintext keys from the
    // `channel_keys` map, which older nodes and the local embedded node still
    // populate.  Entries already resolved via the E2EE path are not overwritten.
    let needs_key_refetch;
    let rotated_channels;
    {
        // Derive this node's X25519 static secret from its Ed25519 signing key.
        let x25519_sk = {
            let key_bytes = state.signing_key.to_bytes();
            NodeIdentity::from_signing_key_bytes(&key_bytes).x25519_secret()
        };
        // Derive this node's UserId (SHA-256 of the verifying key, hex-encoded
        // and then re-parsed as raw bytes — same derivation used at member-join).
        let own_user_id = {
            let peer_id_hex = &state.peer_id;
            let uid_bytes: [u8; 32] = (0..peer_id_hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&peer_id_hex[i..i + 2], 16).unwrap_or(0))
                .collect::<Vec<u8>>()
                .try_into()
                .unwrap_or([0u8; 32]);
            UserId(uid_bytes)
        };

        let mut keys = state.channel_keys.write().await;
        let mut prev_keys = state.previous_channel_keys.write().await;

        // Track channels whose key was actually updated so we only drain the
        // pending-rotation buffer for those (avoiding premature drain when
        // FetchManifest returned stale data from a peer that hasn't rotated yet).
        let mut rotated_channels_inner: Vec<String> = Vec::new();

        // Skip the E2EE key-update step when we are the admin of this community.
        // The admin generates channel keys authoritatively via `channel_rotate_key`
        // and never needs to receive them from peers.  Accepting wrapped keys from
        // a peer that hasn't yet applied the latest rotation would silently overwrite
        // the current (new) key with a stale (old) one, causing the admin's outgoing
        // messages to be encrypted with the wrong key.
        let is_admin = {
            let own_pk = state.signing_key.verifying_key().to_bytes();
            manifest.manifest.public_key == own_pk
        };

        // E2EE path: unwrap keys encrypted for us in each ChannelManifest.
        if !is_admin {
            for ch in &channels {
                let ch_id = ch.id.to_string();
                if let Some(wrapped) = ch.encrypted_channel_key.get(&own_user_id) {
                    match ChannelKey::decrypt_for_self(&x25519_sk, wrapped) {
                        Ok(ck) => {
                            // Retain the old key so messages encrypted with it
                            // (e.g. from history catch-up) can still be decrypted.
                            if let Some(&old) = keys.get(&ch_id) {
                                if old != *ck.as_bytes() {
                                    prev_keys.insert(ch_id.clone(), old);
                                    rotated_channels_inner.push(ch_id.clone());
                                    info!(
                                        channel_id = ch_id.as_str(),
                                        peer = from.as_str(),
                                        "received new channel key via manifest sync \
                                         (was offline during rotation)"
                                    );
                                }
                            }
                            keys.insert(ch_id, *ck.as_bytes());
                        }
                        Err(e) => {
                            warn!("failed to decrypt channel key for {ch_id}: {e}");
                        }
                    }
                }
            }
        }

        // Fallback: accept plaintext keys for channels not resolved above
        // (e.g. local embedded node or pre-E2EE peers).
        for (ch_id, key_bytes) in &channel_keys {
            if !keys.contains_key(ch_id) && key_bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(key_bytes);
                keys.insert(ch_id.clone(), arr);
            }
        }

        save_table(
            &state.data_dir.join("channel_keys.json"),
            &*keys,
            state.encryption_key.as_ref(),
        );
        if !rotated_channels_inner.is_empty() {
            save_table(
                &state.data_dir.join("previous_channel_keys.json"),
                &*prev_keys,
                state.encryption_key.as_ref(),
            );
        }
        needs_key_refetch =
            !channel_ids.is_empty() && channel_ids.iter().any(|id| !keys.contains_key(id));
        rotated_channels = rotated_channels_inner;
    }

    // Drain any messages that were buffered during key rotation and replay
    // them now that the new key is available.  Only drain channels whose key
    // was actually updated; channels where FetchManifest returned stale data
    // keep their buffer so messages continue to be buffered until the correct
    // key arrives.
    {
        let mut pending = state.pending_rotation_messages.lock().await;
        for ch_id in &rotated_channels {
            if let Some(buffered) = pending.remove(ch_id) {
                if !buffered.is_empty() {
                    info!(
                        channel_id = ch_id.as_str(),
                        count = buffered.len(),
                        "replaying messages buffered during key rotation"
                    );
                    for (topic, data) in buffered {
                        // Use Box::pin to allow the recursive async call.
                        Box::pin(super::channel_handler::handle_channel_message(
                            state, topic, data,
                        ))
                        .await;
                    }
                }
            }
        }

        // For channels that still have a pending rotation (key wasn't updated),
        // schedule a retry FetchManifest from a different peer.
        let stale_rotation_channels: Vec<String> = channel_ids
            .iter()
            .filter(|ch_id| pending.contains_key(*ch_id) && !rotated_channels.contains(ch_id))
            .cloned()
            .collect();
        if !stale_rotation_channels.is_empty() {
            debug!(
                channels = ?stale_rotation_channels,
                "key rotation still pending after FetchManifest; will retry"
            );
            // Try fetching from a different peer than the one we just used.
            let retry_peer = {
                let peers = state.connected_peers.read().await;
                peers
                    .iter()
                    .find(|p| p.peer_id != from)
                    .map(|p| p.peer_id.clone())
            };
            if let Some(peer_id) = retry_peer {
                let _ = state
                    .swarm_cmd_tx
                    .send(NetworkCommand::FetchManifest {
                        peer_id,
                        community_id: community_id_str.clone(),
                        community_pk: manifest.manifest.public_key,
                    })
                    .await;
            }
        }
    }

    // Sync to persistent NodeStore if we are a hosting node (or for relaying).
    if let Some(store) = &state.node_store {
        // Build channel_keys for the NodeStore from the decrypted keys in
        // state.channel_keys rather than from the (empty) response map.
        // FetchManifest responses no longer include plaintext keys; they are
        // distributed via encrypted_channel_key in each ChannelManifest and
        // decrypted into state.channel_keys above.  The NodeStore needs the
        // raw key bytes so the QUIC server can re-wrap them for other peers.
        let store_keys = {
            let keys = state.channel_keys.read().await;
            channel_ids
                .iter()
                .filter_map(|ch_id| keys.get(ch_id).map(|k| (ch_id.clone(), k.to_vec())))
                .collect::<HashMap<String, Vec<u8>>>()
        };
        let meta = crate::node::store::CommunityMeta {
            cert: crate::crypto::certificate::HostingCert {
                community_pk: manifest.manifest.public_key,
                node_pk: (0..state.public_key_hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&state.public_key_hex[i..i + 2], 16).unwrap_or(0))
                    .collect::<Vec<u8>>()
                    .try_into()
                    .unwrap_or([0u8; 32]),
                expires_at: u64::MAX,
                signature: ed25519_dalek::Signature::from_bytes(&[0u8; 64]), // dummy cert for joiners
            },
            manifest: Some(manifest.clone()),
            channels: channels.clone(),
            channel_keys: store_keys,
            members: peer_members
                .iter()
                .map(|m| (m.user_id.to_string(), m.clone()))
                .collect(),
        };
        let _ = store.set_community_meta(&manifest.manifest.public_key, &meta);
    }

    // Upsert any member records included in the manifest response (e.g. pre-existing members
    // that joined before us and whose MemberJoined gossip we never received).
    if !peer_members.is_empty() {
        let mut members_store = state.members.write().await;
        let list = members_store.entry(community_id_str.clone()).or_default();
        for member in &peer_members {
            list.insert(member.user_id.to_string(), member.clone());
        }
        save_table(
            &state.data_dir.join("members.json"),
            &*members_store,
            state.encryption_key.as_ref(),
        );
        drop(members_store);

        // Also populate dm_peers so DMs can still be delivered after this
        // community is disbanded.  Members received here never went through
        // handle_member_joined (which is the normal dm_peers population path),
        // so without this step their X25519 keys would be lost on disband.
        {
            let mut dm_peers = state.dm_peers.write().await;
            for member in &peer_members {
                dm_peers
                    .entry(member.user_id.to_string())
                    .or_insert_with(|| DmPeerInfo {
                        display_name: member.display_name.clone(),
                        x25519_public_key: member.x25519_public_key,
                    });
            }
            save_table(
                &state.data_dir.join("dm_peers.json"),
                &*dm_peers,
                state.encryption_key.as_ref(),
            );
        }
    }

    // If this node is the community admin, wrap channel keys for any members that
    // joined while we were offline.  The relay stores their MemberJoined records and
    // includes them in the FetchManifest response, but the channel manifests on the
    // relay never had wrapped keys for them.  Re-broadcast updated ChannelManifestBroadcast
    // messages so the relay and new members receive their keys.
    {
        let own_pk = state.signing_key.verifying_key().to_bytes();
        if manifest.manifest.public_key == own_pk {
            let admin_community_topic = format!("/bitcord/community/{community_id_str}/1.0.0");
            let channel_keys_snap = state.channel_keys.read().await.clone();
            let all_members: Vec<MembershipRecord> = {
                let members = state.members.read().await;
                members
                    .get(&community_id_str)
                    .map(|list| list.values().cloned().collect())
                    .unwrap_or_default()
            };
            let mut channels_guard = state.channels.write().await;
            let mut any_updated = false;
            for ch_id_str in &channel_ids {
                if let Some(&key_bytes) = channel_keys_snap.get(ch_id_str) {
                    let ck = ChannelKey::from_bytes(key_bytes);
                    if let Some(ch) = channels_guard.get_mut(ch_id_str) {
                        let mut updated = false;
                        for member in &all_members {
                            if !ch.encrypted_channel_key.contains_key(&member.user_id) {
                                if let Ok(wrapped) =
                                    ck.encrypt_for_member(&member.x25519_public_key)
                                {
                                    ch.encrypted_channel_key
                                        .insert(member.user_id.clone(), wrapped);
                                    updated = true;
                                    info!(
                                        channel_id = ch_id_str,
                                        user_id = %member.user_id,
                                        "admin: wrapped key for member that joined while offline"
                                    );
                                }
                            }
                        }
                        if updated {
                            if let Ok(encoded) = NetworkEvent::ChannelManifestBroadcast(
                                ChannelManifestBroadcastPayload {
                                    manifest: ch.clone(),
                                },
                            )
                            .encode()
                            {
                                let _ = state
                                    .swarm_cmd_tx
                                    .send(NetworkCommand::Publish {
                                        topic: admin_community_topic.clone(),
                                        data: encoded,
                                    })
                                    .await;
                            }
                            any_updated = true;
                        }
                    }
                }
            }
            if any_updated {
                let channels_snap = channels_guard.clone();
                drop(channels_guard);
                save_table(
                    &state.data_dir.join("channels.json"),
                    &channels_snap,
                    state.encryption_key.as_ref(),
                );
            }
        }
    }

    // Subscribe to all channel GossipSub topics.
    for ch_id in &channel_ids {
        let topic = format!("/bitcord/channel/{ch_id}/1.0.0");
        let _ = state
            .swarm_cmd_tx
            .send(NetworkCommand::Subscribe(topic))
            .await;
    }

    // Subscribe to the community topic for future manifest/member updates.
    let community_topic = format!("/bitcord/community/{community_id_str}/1.0.0");
    let _ = state
        .swarm_cmd_tx
        .send(NetworkCommand::Subscribe(community_topic.clone()))
        .await;

    // Announce ourselves as a member on the community topic — but only if we
    // were already registered as a member (from community_create or
    // community_join).  Hosting-only relay/seed nodes that received the
    // manifest via FetchManifest should NOT appear in the member list.
    let (display_name_self, peer_id_hex, pub_key_hex) = {
        let id_state = state.identity_state.read().await;
        (
            id_state
                .display_name
                .clone()
                .unwrap_or_else(|| state.peer_id[..8.min(state.peer_id.len())].to_string()),
            state.peer_id.clone(),
            state.public_key_hex.clone(),
        )
    };
    let user_id_bytes: [u8; 32] = (0..peer_id_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&peer_id_hex[i..i + 2], 16).unwrap_or(0))
        .collect::<Vec<u8>>()
        .try_into()
        .unwrap_or([0u8; 32]);

    let already_member = {
        let members = state.members.read().await;
        members
            .get(&community_id_str)
            .map(|list| list.contains_key(&state.peer_id))
            .unwrap_or(false)
    };

    if already_member {
        let pub_key_bytes: [u8; 32] = (0..pub_key_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&pub_key_hex[i..i + 2], 16).unwrap_or(0))
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap_or([0u8; 32]);
        let self_x25519_pk = {
            let key_bytes = state.signing_key.to_bytes();
            NodeIdentity::from_signing_key_bytes(&key_bytes).x25519_public_key_bytes()
        };
        // If this node is listed as an admin in the manifest, restore Admin role —
        // this also repairs any corruption from older code that wrongly assigned Member.
        // Otherwise, preserve whatever role is already recorded in the local store.
        let roles = if manifest.manifest.admin_ids.contains(&UserId(user_id_bytes)) {
            vec![Role::Admin]
        } else {
            let members = state.members.read().await;
            members
                .get(&community_id_str)
                .and_then(|list| list.get(&state.peer_id))
                .map(|m| m.roles.clone())
                .unwrap_or_else(|| vec![Role::Member])
        };
        let self_record_joined_at = chrono::Utc::now();
        let self_record_sig = {
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(&user_id_bytes);
            to_sign.extend_from_slice(&community_id_typed.0.to_bytes());
            to_sign.extend_from_slice(display_name_self.as_bytes());
            to_sign.extend_from_slice(
                &(self_record_joined_at.timestamp_millis() as u64).to_le_bytes(),
            );
            to_sign.extend_from_slice(&postcard::to_allocvec(&roles).unwrap_or_default());
            state.signing_key.sign(&to_sign).to_bytes().to_vec()
        };
        let self_record = MembershipRecord {
            user_id: UserId(user_id_bytes),
            community_id: community_id_typed,
            display_name: display_name_self,
            avatar_cid: None,
            joined_at: self_record_joined_at,
            roles,
            public_key: pub_key_bytes,
            x25519_public_key: self_x25519_pk,
            signature: self_record_sig,
        };
        // Persist self to local members store.
        super::community_handler::handle_member_joined_pub(state, self_record.clone()).await;
        // Publish on community topic so peers learn we joined.
        if let Ok(encoded) = NetworkEvent::MemberJoined(self_record).encode() {
            let _ = state
                .swarm_cmd_tx
                .send(NetworkCommand::Publish {
                    topic: community_topic,
                    data: encoded,
                })
                .await;
        }
    }
    // Re-fetch the manifest from the peer that served it only when we are still
    // missing keys for some channels.  The relay now has our x25519 public key
    // (from the MemberJoined we just published) and can wrap channel keys for us.
    // When refetching, skip the history sync below — the next manifest response
    // will trigger it once we (hopefully) have the keys.
    if needs_key_refetch {
        // The MemberJoined we just published will cause the community admin to wrap
        // the channel key and broadcast a ChannelManifestBroadcast back to us via
        // the community topic.  Do NOT immediately re-send FetchManifest here — the
        // seed returns the same unwrapped manifest before the admin has responded,
        // creating an infinite loop.  History catch-up is triggered from the
        // ChannelManifestBroadcast handler once the key arrives.
        state
            .broadcaster
            .send(PushEvent::CommunityManifestUpdated(CommunityEventData {
                community_id: community_id_str.clone(),
                version,
                reason: String::new(),
            }));
        return;
    }

    // Emit push event so the frontend reloads the community.
    state
        .broadcaster
        .send(PushEvent::CommunityManifestUpdated(CommunityEventData {
            community_id: community_id_str.clone(),
            version,
            reason: String::new(),
        }));

    // Trigger history sync only for channels where we hold the decryption key.
    // Channels without a key would yield undecryptable ciphertext, so syncing
    // them would be wasteful and, worse, could loop indefinitely when the peer
    // responds with count=0 (no messages yet).
    let syncable_channels: Vec<String> = {
        let available_keys = state.channel_keys.read().await;
        channel_ids
            .iter()
            .filter(|ch_id| {
                if available_keys.contains_key(*ch_id) {
                    true
                } else {
                    debug!(channel_id = ch_id, "skipping history sync: no channel key");
                    false
                }
            })
            .cloned()
            .collect()
    };
    let community_pk = manifest.manifest.public_key;
    for ch_id in &syncable_channels {
        // Use the NodeStore's last persisted seq as the starting point so we correctly
        // resume after messages that arrived via gossip while we were connected (stored
        // in NodeStore) but may not be in the in-memory log (e.g. after a reconnect).
        let since_seq = if let Some(ref store) = state.node_store {
            let ch_ulid = ch_id.parse::<ulid::Ulid>().ok();
            if let Some(ulid) = ch_ulid {
                store
                    .last_seq(&community_pk, &ulid)
                    .ok()
                    .flatten()
                    .map(|s| s + 1)
                    .unwrap_or(0)
            } else {
                let log = state.message_log.lock().await;
                log.len(ch_id)
            }
        } else {
            let log = state.message_log.lock().await;
            log.len(ch_id)
        };
        info!(
            channel_id = ch_id,
            %from,
            %since_seq,
            "triggering history catch-up from peer"
        );

        // Notify frontend that sync has started.
        state.broadcaster.send(PushEvent::SyncProgress(
            super::super::push_broadcaster::SyncProgressData {
                channel_id: ch_id.clone(),
                progress: 0.0,
            },
        ));

        let _ = state
            .swarm_cmd_tx
            .send(NetworkCommand::FetchChannelHistory {
                peer_id: from.clone(),
                community_id: community_id_str.clone(),
                community_pk,
                channel_id: ch_id.clone(),
                since_seq,
            })
            .await;
    }
}
