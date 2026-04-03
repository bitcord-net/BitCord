use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use tracing::{debug, info, warn};

use super::super::push_broadcaster::{
    ChannelEventData, CommunityEventData, MemberEventData, MemberRoleUpdatedData,
    PresenceChangedData, PushEvent,
};
use super::super::{AppState, DmPeerInfo, UserStatus, remove_community_local, save_table};
use crate::{
    crypto::channel_keys::ChannelKey,
    identity::NodeIdentity,
    model::{
        membership::{MembershipRecord, Role},
        network_event::{
            ChannelManifestBroadcastPayload, MemberLeftPayload, MemberRoleUpdatedPayload,
            NetworkEvent,
        },
        types::UserId,
    },
    network::NetworkCommand,
};

/// Handle a notification that this node has joined a community (e.g. via an
/// inbound JoinCommunity request or a PushManifest from the admin).
///
/// Ensures the node is subscribed to the community's GossipSub topic so it
/// can relay messages and receive updates.
pub(super) async fn handle_community_joined(
    state: &AppState,
    community_pk: [u8; 32],
    community_id: String,
) {
    let topic = format!("/bitcord/community/{community_id}/1.0.0");
    info!(%community_id, "subscribing to community topic after join notification");
    let _ = state
        .swarm_cmd_tx
        .send(NetworkCommand::Subscribe(topic))
        .await;

    // If we have the manifest in NodeStore, load it into state.communities.
    if let Some(store) = &state.node_store {
        if let Ok(Some(meta)) = store.get_community_meta(&community_pk) {
            let mut comms = state.communities.write().await;
            if !comms.contains_key(&community_id) {
                match meta.manifest {
                    Some(manifest) => {
                        // Real manifest available — load and persist to local cache.
                        comms.insert(community_id.clone(), manifest);
                        save_table(
                            &state.data_dir.join("communities.json"),
                            &*comms,
                            state.encryption_key.as_ref(),
                        );
                    }
                    None => {
                        use crate::model::community::{CommunityManifest, SignedManifest};
                        use crate::model::types::CommunityId;

                        // No manifest yet. Insert an in-memory placeholder so that
                        // PeerConnected can queue a FetchManifest using its public key.
                        // Do NOT persist this to disk — the zeroed signature would be
                        // invalid, and the real manifest will overwrite this entry
                        // when it arrives via gossip.
                        comms.insert(
                            community_id.clone(),
                            SignedManifest {
                                manifest: CommunityManifest {
                                    id: ulid::Ulid::from_string(&community_id)
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
                        if !pending.contains(&community_id) {
                            pending.push(community_id.clone());
                        }
                    }
                }
            }
        }
    }
}

/// Decode a GossipSub message received on a community topic and dispatch it.
pub(super) async fn handle_community_message(
    state: &AppState,
    topic: String,
    data: Vec<u8>,
    source: Option<String>,
) {
    let event = match NetworkEvent::decode(&data) {
        Ok(e) => e,
        Err(e) => {
            debug!(topic, "failed to decode community NetworkEvent: {e}");
            return;
        }
    };
    match event {
        NetworkEvent::ChannelManifestBroadcast(payload) => {
            let channel_id_str = payload.manifest.id.to_string();
            let community_id_str = payload.manifest.community_id.to_string();

            // Only process if we are a member of this community.
            let community_pk = {
                let communities = state.communities.read().await;
                communities
                    .get(&community_id_str)
                    .map(|s| s.manifest.public_key)
            };
            let Some(community_pk) = community_pk else {
                return;
            };

            // Store channel manifest.
            {
                let mut channels = state.channels.write().await;
                channels.insert(channel_id_str.clone(), payload.manifest.clone());
                save_table(
                    &state.data_dir.join("channels.json"),
                    &*channels,
                    state.encryption_key.as_ref(),
                );
            }
            // Unwrap the channel key for this node using ECIES.
            {
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
                if let Some(wrapped) = payload.manifest.encrypted_channel_key.get(&own_user_id) {
                    let x25519_sk = {
                        let key_bytes = state.signing_key.to_bytes();
                        NodeIdentity::from_signing_key_bytes(&key_bytes).x25519_secret()
                    };
                    match ChannelKey::decrypt_for_self(&x25519_sk, wrapped) {
                        Ok(ck) => {
                            {
                                let new_key = *ck.as_bytes();
                                let mut keys = state.channel_keys.write().await;
                                let mut prev_keys = state.previous_channel_keys.write().await;
                                // Retain the old key so messages encrypted with it
                                // (e.g. from history catch-up) can still be decrypted
                                // after a rotation.
                                if let Some(&old) = keys.get(&channel_id_str) {
                                    if old != new_key {
                                        prev_keys.insert(channel_id_str.clone(), old);
                                        save_table(
                                            &state.data_dir.join("previous_channel_keys.json"),
                                            &*prev_keys,
                                            state.encryption_key.as_ref(),
                                        );
                                    }
                                }
                                keys.insert(channel_id_str.clone(), new_key);
                                save_table(
                                    &state.data_dir.join("channel_keys.json"),
                                    &*keys,
                                    state.encryption_key.as_ref(),
                                );
                            }
                            // Key arrived via gossip — trigger history catch-up now.
                            // Prefer the admin peer; fall back to any community peer.
                            let peer_id = {
                                let peers_map = state.connected_peers.read().await;
                                let list = peers_map
                                    .get(&community_id_str)
                                    .map(|v| v.as_slice())
                                    .unwrap_or(&[]);
                                list.iter()
                                    .find(|p| p.is_admin)
                                    .or_else(|| list.first())
                                    .map(|p| p.peer_id.clone())
                            };
                            if let Some(peer_id) = peer_id {
                                let since_seq = if let Some(ref store) = state.node_store {
                                    let ch_ulid = channel_id_str.parse::<ulid::Ulid>().ok();
                                    if let Some(ulid) = ch_ulid {
                                        store
                                            .last_seq(&community_pk, &ulid)
                                            .ok()
                                            .flatten()
                                            .map(|s| s + 1)
                                            .unwrap_or(0)
                                    } else {
                                        let log = state.message_log.lock().await;
                                        log.len(&channel_id_str)
                                    }
                                } else {
                                    let log = state.message_log.lock().await;
                                    log.len(&channel_id_str)
                                };
                                info!(
                                    channel_id = channel_id_str,
                                    %peer_id,
                                    %since_seq,
                                    "triggering history catch-up after receiving channel key via gossip"
                                );
                                state.broadcaster.send(PushEvent::SyncProgress(
                                    super::super::push_broadcaster::SyncProgressData {
                                        channel_id: channel_id_str.clone(),
                                        progress: 0.0,
                                    },
                                ));
                                let _ = state
                                    .swarm_cmd_tx
                                    .send(NetworkCommand::FetchChannelHistory {
                                        peer_id,
                                        community_id: community_id_str.clone(),
                                        community_pk,
                                        channel_id: channel_id_str.clone(),
                                        since_seq,
                                    })
                                    .await;
                            }
                        }
                        Err(e) => {
                            warn!(
                                channel_id = channel_id_str,
                                "failed to unwrap channel key: {e}"
                            );
                        }
                    }
                }
                // If own entry not found, this is a relay node — no key stored, manifest still persisted below.
            }
            // Persist to NodeStore if present (relay nodes store manifest for FetchManifest serving).
            if let Some(store) = &state.node_store {
                if let Ok(Some(mut meta)) = store.get_community_meta(&community_pk) {
                    if !meta.channels.iter().any(|c| c.id == payload.manifest.id) {
                        meta.channels.push(payload.manifest.clone());
                    } else if let Some(ch) = meta
                        .channels
                        .iter_mut()
                        .find(|c| c.id == payload.manifest.id)
                    {
                        // Update the manifest (e.g., after key rotation — new encrypted_channel_key map).
                        *ch = payload.manifest.clone();
                    }
                    let _ = store.set_community_meta(&community_pk, &meta);
                }
            }
            // Subscribe to the new channel topic.
            let channel_topic = format!("/bitcord/channel/{channel_id_str}/1.0.0");
            let _ = state
                .swarm_cmd_tx
                .send(NetworkCommand::Subscribe(channel_topic))
                .await;

            debug!(
                channel_id = channel_id_str,
                "stored channel manifest from gossip broadcast"
            );
        }
        NetworkEvent::ManifestUpdate(new_signed) => {
            if !new_signed.verify() {
                warn!("received unverifiable ManifestUpdate on {topic}; discarding");
                return;
            }
            let community_id_str = new_signed.manifest.id.to_string();
            let new_version = new_signed.manifest.version;

            // Only process if this community is known and the manifest is newer.
            let (is_newer, old_channel_ids, old_seed_nodes) = {
                let communities = state.communities.read().await;
                match communities.get(&community_id_str) {
                    Some(current) => (
                        new_version > current.manifest.version,
                        current.manifest.channel_ids.clone(),
                        current.manifest.seed_nodes.clone(),
                    ),
                    None => return, // Not a member of this community.
                }
            };
            if !is_newer {
                return;
            }

            // If the admin deleted the community, remove it and notify the frontend.
            if new_signed.manifest.deleted {
                remove_community_local(state, &community_id_str).await;
                state
                    .broadcaster
                    .send(PushEvent::CommunityDeleted(CommunityEventData {
                        community_id: community_id_str,
                        version: new_version,
                        reason: String::new(),
                    }));
                return;
            }

            // Persist the updated manifest.
            {
                let mut communities = state.communities.write().await;
                communities.insert(community_id_str.clone(), new_signed.clone());
                save_table(
                    &state.data_dir.join("communities.json"),
                    &*communities,
                    state.encryption_key.as_ref(),
                );
            }

            // Update NodeStore if it exists.
            if let Some(store) = &state.node_store {
                if let Ok(Some(mut meta)) =
                    store.get_community_meta(&new_signed.manifest.public_key)
                {
                    meta.manifest = Some(new_signed.clone());
                    let _ = store.set_community_meta(&new_signed.manifest.public_key, &meta);
                }
            }

            // Subscribe to any new channel topics.
            let new_channel_ids: Vec<String> = new_signed
                .manifest
                .channel_ids
                .iter()
                .filter(|id| !old_channel_ids.contains(id))
                .map(|id| id.to_string())
                .collect();
            for ch_id in &new_channel_ids {
                let channel_topic = format!("/bitcord/channel/{ch_id}/1.0.0");
                let _ = state
                    .swarm_cmd_tx
                    .send(NetworkCommand::Subscribe(channel_topic))
                    .await;
            }

            // Unsubscribe from and remove any channels that were deleted.
            let removed_channel_ids: Vec<String> = old_channel_ids
                .iter()
                .filter(|id| !new_signed.manifest.channel_ids.contains(id))
                .map(|id| id.to_string())
                .collect();
            if !removed_channel_ids.is_empty() {
                for ch_id in &removed_channel_ids {
                    let channel_topic = format!("/bitcord/channel/{ch_id}/1.0.0");
                    let _ = state
                        .swarm_cmd_tx
                        .send(NetworkCommand::Unsubscribe(channel_topic))
                        .await;
                }
                {
                    let mut channels = state.channels.write().await;
                    for ch_id in &removed_channel_ids {
                        channels.remove(ch_id);
                    }
                    save_table(
                        &state.data_dir.join("channels.json"),
                        &*channels,
                        state.encryption_key.as_ref(),
                    );
                }
                for ch_id in &removed_channel_ids {
                    state
                        .broadcaster
                        .send(PushEvent::ChannelDeleted(ChannelEventData {
                            channel_id: ch_id.clone(),
                            community_id: community_id_str.clone(),
                            name: String::new(),
                        }));
                }
            }

            // Request a full manifest from the source peer (or any connected peer)
            // to obtain channel manifests + keys and trigger history sync.
            if let Some(src) = source {
                let _ = state
                    .swarm_cmd_tx
                    .send(NetworkCommand::FetchManifest {
                        peer_id: src,
                        community_id: community_id_str.clone(),
                        community_pk: new_signed.manifest.public_key,
                    })
                    .await;
            } else {
                // Queue for sync: add to pending manifest syncs.
                let mut pending = state.pending_manifest_syncs.lock().await;
                if !pending.contains(&community_id_str) {
                    pending.push(community_id_str.clone());
                }
            }

            // If the seed node address changed, dial the new one so this node
            // stays connected and can sync.  This handles the case where an
            // offline user comes back and discovers the community moved to a
            // different seed node.
            let new_seed = new_signed
                .manifest
                .seed_nodes
                .first()
                .cloned()
                .unwrap_or_default();
            let old_seed = old_seed_nodes.first().cloned().unwrap_or_default();
            if !new_seed.is_empty() && new_seed != old_seed {
                if let Ok(addr) = new_seed.parse::<crate::network::NodeAddr>() {
                    info!(
                        community_id = %community_id_str,
                        seed = %new_seed,
                        "seed node changed; dialing new seed"
                    );
                    let hosting_pw = state
                        .hosting_passwords
                        .read()
                        .await
                        .get(&community_id_str)
                        .cloned();
                    let fp = state
                        .seed_fingerprints
                        .read()
                        .await
                        .get(&new_seed)
                        .copied()
                        .unwrap_or([0u8; 32]);
                    let _ = state
                        .swarm_cmd_tx
                        .send(NetworkCommand::Dial {
                            addr,
                            is_seed: true,
                            join_community: Some((
                                new_signed.manifest.public_key,
                                community_id_str.clone(),
                            )),
                            join_community_password: hosting_pw,
                            cert_fingerprint: fp,
                        })
                        .await;
                }
            }

            state
                .broadcaster
                .send(PushEvent::CommunityManifestUpdated(CommunityEventData {
                    community_id: community_id_str,
                    version: new_version,
                    reason: String::new(),
                }));
        }
        NetworkEvent::MemberJoined(record) => {
            handle_member_joined(state, record).await;
        }
        NetworkEvent::MemberLeft(payload) => {
            handle_member_left(state, payload).await;
        }
        NetworkEvent::MemberRoleUpdated(payload) => {
            handle_member_role_updated(state, payload).await;
        }
        NetworkEvent::PresenceHeartbeat(hb) => {
            // Verify the Ed25519 signature before updating presence state.
            {
                let members = state.members.read().await;
                let member_pk = members
                    .values()
                    .flat_map(|list| list.values())
                    .find_map(|m| {
                        if m.user_id == hb.user_id {
                            Some(m.public_key)
                        } else {
                            None
                        }
                    });
                match member_pk {
                    Some(pk) => {
                        let sig_ok = VerifyingKey::from_bytes(&pk).is_ok_and(|vk| {
                            let sig_bytes: [u8; 64] =
                                hb.signature.as_slice().try_into().unwrap_or([0u8; 64]);
                            let sig = Signature::from_bytes(&sig_bytes);
                            let mut to_verify = Vec::new();
                            to_verify.extend_from_slice(&hb.user_id.0);
                            to_verify.extend_from_slice(hb.status.as_bytes());
                            to_verify.extend_from_slice(
                                &(hb.timestamp.timestamp_millis() as u64).to_le_bytes(),
                            );
                            vk.verify(&to_verify, &sig).is_ok()
                        });
                        if !sig_ok {
                            warn!(user_id = %hb.user_id, "presence_heartbeat: invalid signature; discarding");
                            return;
                        }
                    }
                    None => {
                        // Unknown member — silently ignore rather than panic.
                        return;
                    }
                }
            }
            let user_id_str = hb.user_id.to_string();
            let (status, status_str) = match hb.status.as_str() {
                "online" => (UserStatus::Online, "online"),
                "idle" => (UserStatus::Idle, "idle"),
                "do_not_disturb" => (UserStatus::DoNotDisturb, "do_not_disturb"),
                "invisible" => (UserStatus::Invisible, "invisible"),
                _ => (UserStatus::Offline, "offline"),
            };
            {
                let mut presence = state.presence.write().await;
                presence.insert(user_id_str.clone(), status);
            }
            state
                .broadcaster
                .send(PushEvent::PresenceChanged(PresenceChangedData {
                    user_id: user_id_str,
                    status: status_str.to_string(),
                    last_seen: hb.timestamp,
                }));
        }
        _ => {} // Other event variants not used on community topics.
    }
}

/// Public-to-sibling wrapper so `manifest_handler` can call `handle_member_joined`
/// without exposing it beyond this crate's event_processor module.
pub(super) async fn handle_member_joined_pub(state: &AppState, record: MembershipRecord) {
    handle_member_joined(state, record).await;
}

/// Store an incoming membership record and notify the frontend.
async fn handle_member_joined(state: &AppState, record: MembershipRecord) {
    // Verify the self-signature before accepting.
    let vk = match VerifyingKey::from_bytes(&record.public_key) {
        Ok(k) => k,
        Err(_) => {
            warn!(user_id = %record.user_id, "member_joined: invalid public key; discarding");
            return;
        }
    };
    // Ensure the claimed user_id is actually derived from the supplied public key.
    if record.user_id != UserId::from_verifying_key(&vk) {
        warn!(user_id = %record.user_id, "member_joined: user_id does not match public key; discarding");
        return;
    }
    let sig_bytes: [u8; 64] = match record.signature.as_slice().try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!(user_id = %record.user_id, "member_joined: invalid signature length; discarding");
            return;
        }
    };
    let sig = Signature::from_bytes(&sig_bytes);
    let mut to_verify = Vec::new();
    to_verify.extend_from_slice(&record.user_id.0);
    to_verify.extend_from_slice(&record.community_id.0.to_bytes());
    to_verify.extend_from_slice(record.display_name.as_bytes());
    to_verify.extend_from_slice(&(record.joined_at.timestamp_millis() as u64).to_le_bytes());
    to_verify.extend_from_slice(&postcard::to_allocvec(&record.roles).unwrap_or_default());
    if vk.verify(&to_verify, &sig).is_err() {
        warn!(user_id = %record.user_id, "member_joined: signature verification failed; discarding");
        return;
    }

    let community_id_str = record.community_id.to_string();
    let user_id_str = record.user_id.to_string();
    let display_name = record.display_name.clone();

    // Reject gossip for banned users.
    {
        let bans = state.bans.read().await;
        if bans
            .get(&community_id_str)
            .map(|list| list.contains(&user_id_str))
            .unwrap_or(false)
        {
            warn!(user_id = %user_id_str, "member_joined: user is banned; discarding");
            return;
        }
    }

    {
        let mut members = state.members.write().await;
        let list = members.entry(community_id_str.clone()).or_default();
        list.insert(record.user_id.to_string(), record.clone());
        save_table(
            &state.data_dir.join("members.json"),
            &*members,
            state.encryption_key.as_ref(),
        );
    }
    // Cache peer info so DMs survive community disbanding.
    {
        let mut dm_peers = state.dm_peers.write().await;
        dm_peers.insert(
            user_id_str.clone(),
            DmPeerInfo {
                display_name: display_name.clone(),
                x25519_public_key: record.x25519_public_key,
            },
        );
        save_table(
            &state.data_dir.join("dm_peers.json"),
            &*dm_peers,
            state.encryption_key.as_ref(),
        );
    }

    // Update NodeStore if it exists.
    if let Some(store) = &state.node_store {
        let communities = state.communities.read().await;
        if let Some(signed) = communities.get(&community_id_str) {
            let pk = signed.manifest.public_key;
            let mut meta = store
                .get_community_meta(&pk)
                .unwrap_or(None)
                .unwrap_or_else(|| crate::node::store::CommunityMeta {
                    cert: crate::crypto::certificate::HostingCert {
                        community_pk: pk,
                        node_pk: (0..state.public_key_hex.len())
                            .step_by(2)
                            .map(|i| {
                                u8::from_str_radix(&state.public_key_hex[i..i + 2], 16).unwrap_or(0)
                            })
                            .collect::<Vec<u8>>()
                            .try_into()
                            .unwrap_or([0u8; 32]),
                        expires_at: u64::MAX,
                        signature: ed25519_dalek::Signature::from_bytes(&[0u8; 64]),
                    },
                    manifest: Some(signed.clone()),
                    channels: Vec::new(),
                    channel_keys: std::collections::HashMap::new(),
                    members: std::collections::HashMap::new(),
                });

            meta.members
                .insert(record.user_id.to_string(), record.clone());
            let _ = store.set_community_meta(&pk, &meta);
            debug!(user_id = %user_id_str, community_id = %community_id_str, "synced member to NodeStore");
        }
    }

    // If this node is the community admin, wrap channel keys for the new member and
    // re-broadcast the updated channel manifests.  This ensures members who join
    // after channels were created can decrypt messages immediately, without relying
    // on the FetchManifest race-condition retry loop.
    {
        let own_pk = state.signing_key.verifying_key().to_bytes();
        let communities = state.communities.read().await;
        if let Some(signed) = communities.get(&community_id_str) {
            if signed.manifest.public_key == own_pk {
                let channel_ids: Vec<String> = signed
                    .manifest
                    .channel_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect();
                drop(communities);

                let channel_keys_snap = state.channel_keys.read().await.clone();
                let topic = format!("/bitcord/community/{community_id_str}/1.0.0");

                for ch_id_str in &channel_ids {
                    if let Some(&key_bytes) = channel_keys_snap.get(ch_id_str) {
                        let mut channels = state.channels.write().await;
                        if let Some(ch) = channels.get_mut(ch_id_str) {
                            if ch.encrypted_channel_key.contains_key(&record.user_id) {
                                continue; // already has a wrapped key for this member
                            }
                            let ck = ChannelKey::from_bytes(key_bytes);
                            if let Ok(wrapped) = ck.encrypt_for_member(&record.x25519_public_key) {
                                ch.encrypted_channel_key
                                    .insert(record.user_id.clone(), wrapped);
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
                                            topic: topic.clone(),
                                            data: encoded,
                                        })
                                        .await;
                                }
                                // Persist updated channel manifest so FetchManifest responses
                                // and future PeerConnected broadcasts also include the new key.
                                let channels_snap = channels.clone();
                                drop(channels);
                                save_table(
                                    &state.data_dir.join("channels.json"),
                                    &channels_snap,
                                    state.encryption_key.as_ref(),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    state
        .broadcaster
        .send(PushEvent::MemberJoined(MemberEventData {
            user_id: user_id_str,
            community_id: community_id_str,
            display_name,
        }));
}

/// Handle a `MemberLeft` gossip event: verify the signature, remove the member, notify frontend.
async fn handle_member_left(state: &AppState, payload: MemberLeftPayload) {
    let community_id_str = payload.community_id.to_string();
    let user_id_str = payload.user_id.to_string();

    // Build the signed message bytes: user_id (32) || community_id ULID bytes (16) || "leave".
    let mut msg = Vec::with_capacity(32 + 16 + 5);
    msg.extend_from_slice(&payload.user_id.0);
    msg.extend_from_slice(&payload.community_id.0.to_bytes());
    msg.extend_from_slice(b"leave");

    let sig_bytes: [u8; 64] = match payload.signature.as_slice().try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("member_left: invalid signature length for user {user_id_str}");
            return;
        }
    };
    let sig = Signature::from_bytes(&sig_bytes);

    // Accept if the departing member themselves signed (self-leave) OR
    // if an admin/moderator signed (kick/ban).
    let valid = {
        let members = state.members.read().await;
        members
            .get(&community_id_str)
            .map(|list| {
                list.values().any(|m| {
                    let is_self = m.user_id == payload.user_id;
                    let is_privileged = m
                        .roles
                        .iter()
                        .any(|r| matches!(r, Role::Admin | Role::Moderator));
                    if !is_self && !is_privileged {
                        return false;
                    }
                    VerifyingKey::from_bytes(&m.public_key)
                        .is_ok_and(|vk| vk.verify(&msg, &sig).is_ok())
                })
            })
            .unwrap_or(false)
    };

    if !valid {
        warn!(
            "member_left: unverifiable signature for user {user_id_str} in community {community_id_str}; discarding"
        );
        return;
    }

    // Remove from the in-memory + persisted members map.
    let removed = {
        let mut members = state.members.write().await;
        if let Some(list) = members.get_mut(&community_id_str) {
            list.remove(&payload.user_id.to_string())
        } else {
            None
        }
    };

    if let Some(removed) = removed {
        {
            let members = state.members.read().await;
            save_table(
                &state.data_dir.join("members.json"),
                &*members,
                state.encryption_key.as_ref(),
            );
        }

        // Update NodeStore if present.
        if let Some(store) = &state.node_store {
            let communities = state.communities.read().await;
            if let Some(signed) = communities.get(&community_id_str) {
                if let Ok(Some(mut meta)) = store.get_community_meta(&signed.manifest.public_key) {
                    meta.members.remove(&payload.user_id.to_string());
                    let _ = store.set_community_meta(&signed.manifest.public_key, &meta);
                }
            }
        }

        state
            .broadcaster
            .send(PushEvent::MemberLeft(MemberEventData {
                user_id: user_id_str,
                community_id: community_id_str,
                display_name: removed.display_name,
            }));
    }
}

/// Handle a `MemberRoleUpdated` gossip event: verify the admin's signature, apply the role
/// change, persist, and notify the frontend.
async fn handle_member_role_updated(state: &AppState, payload: MemberRoleUpdatedPayload) {
    let community_id_str = payload.community_id.to_string();
    let user_id_str = payload.user_id.to_string();

    // Build signed bytes: user_id (32) || community_id ULID bytes (16) || postcard(new_role) || "role_update".
    let role_bytes = postcard::to_allocvec(&payload.new_role).unwrap_or_default();
    let mut msg = Vec::with_capacity(32 + 16 + role_bytes.len() + 11);
    msg.extend_from_slice(&payload.user_id.0);
    msg.extend_from_slice(&payload.community_id.0.to_bytes());
    msg.extend_from_slice(&role_bytes);
    msg.extend_from_slice(b"role_update");

    let sig_bytes: [u8; 64] = match payload.signature.as_slice().try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("member_role_updated: invalid signature length for user {user_id_str}");
            return;
        }
    };
    let sig = Signature::from_bytes(&sig_bytes);

    // Verify that a community admin signed the message.
    let valid = {
        let members = state.members.read().await;
        members
            .get(&community_id_str)
            .map(|list| {
                list.values().any(|m| {
                    let is_admin = m.roles.iter().any(|r| matches!(r, Role::Admin));
                    if !is_admin {
                        return false;
                    }
                    VerifyingKey::from_bytes(&m.public_key)
                        .is_ok_and(|vk| vk.verify(&msg, &sig).is_ok())
                })
            })
            .unwrap_or(false)
    };

    if !valid {
        warn!(
            "member_role_updated: unverifiable admin signature for user {user_id_str} in community {community_id_str}; discarding"
        );
        return;
    }

    // Apply the role change.
    let updated = {
        let mut members = state.members.write().await;
        if let Some(list) = members.get_mut(&community_id_str) {
            if let Some(member) = list.get_mut(&user_id_str) {
                member.roles = vec![payload.new_role.clone()];
                true
            } else {
                false
            }
        } else {
            false
        }
    };

    if !updated {
        return;
    }

    {
        let members = state.members.read().await;
        save_table(
            &state.data_dir.join("members.json"),
            &*members,
            state.encryption_key.as_ref(),
        );
    }

    let role_str = match payload.new_role {
        Role::Admin => "admin",
        Role::Moderator => "moderator",
        Role::Member => "member",
    };
    state
        .broadcaster
        .send(PushEvent::MemberRoleUpdated(MemberRoleUpdatedData {
            user_id: user_id_str,
            community_id: community_id_str,
            new_role: role_str.to_string(),
        }));
}
