use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use tracing::{debug, info, warn};

use super::super::push_broadcaster::{
    ChannelHistorySyncedData, MessageDeletedData, MessageEventData, PushEvent,
    ReactionInfo as PushReactionInfo, ReactionUpdatedData,
};
use super::super::{AppState, save_table};
use crate::{
    crypto::channel_keys::ChannelKey,
    model::{
        message::MessageContent,
        network_event::{
            ChannelKeyRotationPayload, DeleteMessagePayload, EditMessagePayload, NetworkEvent,
        },
    },
    network::NetworkCommand,
};
use ulid::Ulid;

/// Decode a GossipSub channel message, decrypt it, append to the CRDT, and push to frontend.
pub(super) async fn handle_channel_message(state: &AppState, topic: String, data: Vec<u8>) {
    let event = match NetworkEvent::decode(&data) {
        Ok(e) => e,
        Err(e) => {
            debug!(topic, "failed to decode NetworkEvent: {e}");
            return;
        }
    };
    match event {
        NetworkEvent::NewMessage(raw) => {
            let channel_id_str = raw.channel_id.to_string();

            // Look up which community this channel belongs to.
            let community_id = {
                let channels = state.channels.read().await;
                match channels
                    .get(&channel_id_str)
                    .map(|c| c.community_id.clone())
                {
                    Some(id) => id,
                    None => {
                        debug!("received message for unknown channel {channel_id_str}");
                        return;
                    }
                }
            };

            // Retrieve channel key for decryption.
            let (key_bytes, prev_key_bytes) = {
                let keys = state.channel_keys.read().await;
                match keys.get(&channel_id_str).copied() {
                    Some(k) => {
                        let prev = state.previous_channel_keys.read().await;
                        (k, prev.get(&channel_id_str).copied())
                    }
                    None => {
                        debug!("no key for channel {channel_id_str}");
                        return;
                    }
                }
            };

            // Decrypt to validate membership and decode the content type.
            let channel_key = ChannelKey::from_bytes(key_bytes);
            let plaintext = match channel_key.decrypt_message(&raw.nonce, &raw.ciphertext) {
                Ok(p) => p,
                Err(_) => {
                    // Try the previous key (pre-rotation) before giving up.
                    if let Some(prev_kb) = prev_key_bytes {
                        let prev_ck = ChannelKey::from_bytes(prev_kb);
                        if let Ok(p) = prev_ck.decrypt_message(&raw.nonce, &raw.ciphertext) {
                            debug!(
                                "decrypted message on channel {channel_id_str} \
                                 using previous (pre-rotation) key"
                            );
                            p
                        } else {
                            // Both keys failed — buffer or warn as before.
                            let buffered = {
                                let mut pending = state.pending_rotation_messages.lock().await;
                                if let Some(buf) = pending.get_mut(&channel_id_str) {
                                    buf.push((topic.clone(), data.clone()));
                                    true
                                } else {
                                    false
                                }
                            };
                            if buffered {
                                debug!(
                                    "buffered undecryptable message on channel {channel_id_str} \
                                     (key rotation in progress)"
                                );
                            } else {
                                warn!(
                                    "failed to decrypt incoming message on channel {channel_id_str}"
                                );
                            }
                            return;
                        }
                    } else {
                        // If a key rotation is in progress for this channel, buffer
                        // the raw gossip message so it can be retried once the new
                        // key arrives via FetchManifest.
                        let buffered = {
                            let mut pending = state.pending_rotation_messages.lock().await;
                            if let Some(buf) = pending.get_mut(&channel_id_str) {
                                buf.push((topic.clone(), data.clone()));
                                true
                            } else {
                                false
                            }
                        };
                        if buffered {
                            debug!(
                                "buffered undecryptable message on channel {channel_id_str} \
                                 (key rotation in progress)"
                            );
                        } else {
                            warn!("failed to decrypt incoming message on channel {channel_id_str}");
                        }
                        return;
                    }
                }
            };
            let content = MessageContent::decode(&plaintext);

            // Verify the Ed25519 signature over the message before appending.
            {
                let members = state.members.read().await;
                let member_pk = members
                    .get(&community_id.to_string())
                    .and_then(|list| list.get(&raw.author_id.to_string()))
                    .map(|m| m.public_key);
                match member_pk {
                    Some(pk) => {
                        let verified =
                            VerifyingKey::from_bytes(&pk).is_ok_and(|vk| raw.verify(&vk));
                        if !verified {
                            warn!(
                                author_id = %raw.author_id,
                                channel_id = channel_id_str,
                                "new_message: invalid signature; discarding"
                            );
                            return;
                        }
                    }
                    None => {
                        // Sender not yet in members list (race between MemberJoined and NewMessage).
                        warn!(
                            author_id = %raw.author_id,
                            channel_id = channel_id_str,
                            "new_message: unknown author; discarding"
                        );
                        return;
                    }
                }
            }

            // Append to the message log (skip if already present — duplicate delivery).
            {
                let mut log = state.message_log.lock().await;
                if log
                    .get_entry(&channel_id_str, &raw.id.to_string())
                    .is_some()
                {
                    return; // already stored — discard duplicate
                }
                // For reaction entries, also update the in-memory reactions cache.
                if let Some(MessageContent::Reaction {
                    ref target_message_id,
                    ref emoji,
                    is_add,
                }) = content
                {
                    if is_add {
                        log.react(target_message_id, emoji, &raw.author_id.to_string());
                    } else {
                        log.unreact(target_message_id, emoji, &raw.author_id.to_string());
                    }
                }
                log.append(
                    &channel_id_str,
                    raw.id.to_string(),
                    raw.author_id.to_string(),
                    raw.timestamp.timestamp_millis(),
                    raw.nonce,
                    raw.ciphertext.clone(),
                );
            }

            // Persist to NodeStore if available.
            if let Some(store) = &state.node_store {
                let communities = state.communities.read().await;
                if let Some(comm) = communities.get(&community_id.to_string()) {
                    let pk = comm.manifest.public_key;
                    let _ = store.append_message(
                        &pk,
                        &raw.channel_id.0,
                        raw.nonce,
                        raw.ciphertext.clone(),
                        raw.id.to_string(),
                        raw.author_id.to_string(),
                        raw.timestamp.timestamp_millis(),
                    );
                }
            }

            // Notify connected frontend clients.
            match content {
                Some(MessageContent::Reaction {
                    target_message_id, ..
                }) => {
                    // Emit ReactionUpdated so the frontend refreshes reactions for the target.
                    let reactions = {
                        let log = state.message_log.lock().await;
                        log.get_reactions(&target_message_id)
                    };
                    let reactions: Vec<PushReactionInfo> = reactions
                        .into_iter()
                        .map(|(emoji, user_ids)| PushReactionInfo { emoji, user_ids })
                        .collect();
                    state
                        .broadcaster
                        .send(PushEvent::ReactionUpdated(ReactionUpdatedData {
                            message_id: target_message_id,
                            channel_id: channel_id_str,
                            community_id: community_id.to_string(),
                            reactions,
                        }));
                }
                _ => {
                    // Text message (or unknown entry type) — emit MessageNew.
                    let author_id_str = raw.author_id.to_string();
                    let author_name = {
                        let members = state.members.read().await;
                        members
                            .get(&community_id.to_string())
                            .and_then(|list| list.get(&author_id_str))
                            .map(|m| m.display_name.clone())
                    };
                    state
                        .broadcaster
                        .send(PushEvent::MessageNew(MessageEventData {
                            message_id: raw.id.to_string(),
                            channel_id: channel_id_str,
                            community_id: community_id.to_string(),
                            author_id: author_id_str,
                            author_name,
                            timestamp: raw.timestamp,
                            body: None,
                        }));
                }
            }
        }
        NetworkEvent::ChannelKeyRotation(payload) => {
            handle_channel_key_rotation(state, payload).await;
        }
        NetworkEvent::EditMessage(payload) => {
            handle_edit_message(state, payload).await;
        }
        NetworkEvent::DeleteMessage(payload) => {
            handle_delete_message(state, payload).await;
        }
        _ => {}
    }
}

/// Handle an `EditMessage` gossip event received from a peer.
///
/// Verifies the author's signature, updates the local message log entry,
/// then emits a `PushEvent::MessageEdited` so connected frontends refresh.
pub(super) async fn handle_edit_message(state: &AppState, payload: EditMessagePayload) {
    let channel_id_str = payload.channel_id.to_string();
    let message_id_str = payload.message_id.to_string();
    let author_id_str = payload.author_id.to_string();

    // Verify authorship: the log entry must exist and belong to the same author.
    {
        let log = state.message_log.lock().await;
        match log.get_entry(&channel_id_str, &message_id_str) {
            Some(entry) => {
                if entry.author_id != author_id_str {
                    warn!("EditMessage author mismatch for {message_id_str}; discarding");
                    return;
                }
            }
            None => {
                debug!("EditMessage for unknown message {message_id_str}; discarding");
                return;
            }
        }
    }

    // Update the log entry in-place.
    {
        let mut log = state.message_log.lock().await;
        if !log.edit(
            &channel_id_str,
            &message_id_str,
            payload.new_nonce,
            payload.new_ciphertext.clone(),
        ) {
            debug!("EditMessage: failed to update log entry {message_id_str}");
            return;
        }
    }

    // Look up community_id for the push event.
    let community_id = {
        let channels = state.channels.read().await;
        match channels
            .get(&channel_id_str)
            .map(|c| c.community_id.to_string())
        {
            Some(id) => id,
            None => {
                debug!("EditMessage for unknown channel {channel_id_str}");
                return;
            }
        }
    };

    let author_name = {
        let members = state.members.read().await;
        members
            .get(&community_id)
            .and_then(|list| list.get(&author_id_str))
            .map(|m| m.display_name.clone())
    };

    let body: Option<String> = {
        let keys = state.channel_keys.read().await;
        keys.get(&channel_id_str).and_then(|&key_bytes| {
            let ck = ChannelKey::from_bytes(key_bytes);
            ck.decrypt_message(&payload.new_nonce, &payload.new_ciphertext)
                .ok()
                .and_then(|plain| MessageContent::decode(&plain))
                .and_then(|mc| match mc {
                    MessageContent::Text { body, .. } => Some(body),
                    _ => None,
                })
        })
    };

    state
        .broadcaster
        .send(PushEvent::MessageEdited(MessageEventData {
            message_id: message_id_str,
            channel_id: channel_id_str,
            community_id,
            author_id: author_id_str,
            author_name,
            timestamp: payload.timestamp,
            body,
        }));
}

/// Handle a `DeleteMessage` gossip event received from a peer.
///
/// Verifies the author matches the original message, tombstones the local
/// log entry, then emits a `PushEvent::MessageDeleted` for the frontend.
pub(super) async fn handle_delete_message(state: &AppState, payload: DeleteMessagePayload) {
    let channel_id_str = payload.channel_id.to_string();
    let message_id_str = payload.message_id.to_string();
    let author_id_str = payload.author_id.to_string();

    // Verify authorship.
    {
        let log = state.message_log.lock().await;
        match log.get_entry(&channel_id_str, &message_id_str) {
            Some(entry) => {
                if entry.author_id != author_id_str {
                    warn!("DeleteMessage author mismatch for {message_id_str}; discarding");
                    return;
                }
            }
            None => {
                debug!("DeleteMessage for unknown message {message_id_str}; discarding");
                return;
            }
        }
    }

    // Tombstone the entry.
    {
        let mut log = state.message_log.lock().await;
        if !log.tombstone(&channel_id_str, &message_id_str) {
            debug!("DeleteMessage: failed to tombstone {message_id_str}");
            return;
        }
    }

    // Look up community_id for the push event.
    let community_id = {
        let channels = state.channels.read().await;
        match channels
            .get(&channel_id_str)
            .map(|c| c.community_id.to_string())
        {
            Some(id) => id,
            None => {
                debug!("DeleteMessage for unknown channel {channel_id_str}");
                return;
            }
        }
    };

    state
        .broadcaster
        .send(PushEvent::MessageDeleted(MessageDeletedData {
            message_id: message_id_str,
            channel_id: channel_id_str,
            community_id,
        }));
}

/// Handle a `ChannelKeyRotation` gossip event received from a peer.
///
/// Verifies the admin's signature over the new channel manifest, applies the
/// manifest update locally, then triggers a `FetchManifest` so the node
/// retrieves the new raw key bytes from the rotating peer.
pub(super) async fn handle_channel_key_rotation(
    state: &AppState,
    payload: ChannelKeyRotationPayload,
) {
    let channel_id_str = payload.new_manifest.id.to_string();
    let community_id_str = payload.new_manifest.community_id.to_string();

    // Look up the community admin public key used to verify the signature.
    let admin_pub_key: [u8; 32] = {
        let communities = state.communities.read().await;
        match communities.get(&community_id_str) {
            Some(signed) => signed.manifest.public_key,
            None => {
                debug!("channel_key_rotation: unknown community {community_id_str}");
                return;
            }
        }
    };

    // Verify the Ed25519 signature over the postcard-encoded new manifest.
    let Ok(vk) = VerifyingKey::from_bytes(&admin_pub_key) else {
        warn!("channel_key_rotation: invalid admin public key for community {community_id_str}");
        return;
    };
    let Ok(sig_bytes): Result<[u8; 64], _> = payload.signature.as_slice().try_into() else {
        warn!("channel_key_rotation: invalid signature length for channel {channel_id_str}");
        return;
    };
    let sig = Signature::from_bytes(&sig_bytes);
    let encoded_manifest = match postcard::to_allocvec(&payload.new_manifest) {
        Ok(b) => b,
        Err(e) => {
            warn!("channel_key_rotation: failed to encode manifest for verification: {e}");
            return;
        }
    };
    if vk.verify(&encoded_manifest, &sig).is_err() {
        warn!(
            "channel_key_rotation: signature verification failed for channel {channel_id_str}; discarding"
        );
        return;
    }

    // Only apply if the incoming version is strictly newer.
    let is_newer = {
        let channels = state.channels.read().await;
        match channels.get(&channel_id_str) {
            Some(current) => payload.new_manifest.version > current.version,
            None => false, // Unknown channel; ignore.
        }
    };
    if !is_newer {
        return;
    }

    // Persist the updated channel manifest.
    {
        let mut channels = state.channels.write().await;
        channels.insert(channel_id_str.clone(), payload.new_manifest.clone());
        save_table(
            &state.data_dir.join("channels.json"),
            &*channels,
            state.encryption_key.as_ref(),
        );
    }

    // Update NodeStore if it exists.
    if let Some(store) = &state.node_store {
        if let Ok(Some(mut meta)) = store.get_community_meta(&admin_pub_key) {
            // Update the channel manifest in the list.
            if let Some(ch) = meta
                .channels
                .iter_mut()
                .find(|c| c.id.to_string() == channel_id_str)
            {
                *ch = payload.new_manifest.clone();
            } else {
                meta.channels.push(payload.new_manifest.clone());
            }
            let _ = store.set_community_meta(&admin_pub_key, &meta);
        }
    }

    // Mark this channel as pending key rotation so that incoming messages
    // encrypted with the new key are buffered instead of discarded.
    // This must happen BEFORE sending FetchManifest to avoid a race where
    // the response arrives before the buffer is set up.
    {
        let mut pending = state.pending_rotation_messages.lock().await;
        pending.entry(channel_id_str.clone()).or_default();
    }

    // Fetch an updated full manifest from any connected peer so we receive the
    // new key bytes (E2EE: wrapped per-member in ChannelManifest responses).
    let connected_peer = {
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
    if let Some(peer_id) = connected_peer {
        let _ = state
            .swarm_cmd_tx
            .send(NetworkCommand::FetchManifest {
                peer_id,
                community_id: community_id_str,
                community_pk: admin_pub_key,
            })
            .await;
    }

    info!(
        channel_id = channel_id_str,
        "applied channel key rotation from peer"
    );
}

/// Handle channel history received from a peer.
pub(super) async fn handle_channel_history_received(
    state: &AppState,
    community_id: String,
    channel_id: String,
    entries: Vec<crate::state::message_log::LogEntry>,
) {
    let community_id_str = community_id.clone();
    let channel_id_str = channel_id.clone();

    if !entries.is_empty() {
        // Look up community public key for NodeStore.
        let community_pk = {
            let communities = state.communities.read().await;
            communities
                .get(&community_id_str)
                .map(|c| c.manifest.public_key)
        };

        let channel_ulid = match Ulid::from_string(&channel_id_str) {
            Ok(u) => u,
            Err(_) => {
                // Invalid channel ULID — still complete the sync so the banner clears.
                state.broadcaster.send(PushEvent::SyncProgress(
                    super::super::push_broadcaster::SyncProgressData {
                        channel_id: channel_id_str.clone(),
                        progress: 1.0,
                    },
                ));
                state
                    .broadcaster
                    .send(PushEvent::ChannelHistorySynced(ChannelHistorySyncedData {
                        channel_id: channel_id_str,
                        community_id: community_id_str,
                    }));
                return;
            }
        };

        // Append to MessageLog and NodeStore, rebuilding reactions cache from reaction entries.
        {
            let key_bytes = state
                .channel_keys
                .read()
                .await
                .get(&channel_id_str)
                .copied();
            let mut log = state.message_log.lock().await;
            for entry in &entries {
                // Avoid duplicate appends if we already have this message.
                if log.get_entry(&channel_id_str, &entry.message_id).is_some() {
                    continue;
                }

                // Decrypt reaction entries to rebuild the in-memory reactions cache.
                if let Some(kb) = key_bytes {
                    let ck = ChannelKey::from_bytes(kb);
                    if let Ok(plain) = ck.decrypt_message(&entry.nonce, &entry.ciphertext) {
                        if let Some(MessageContent::Reaction {
                            ref target_message_id,
                            ref emoji,
                            is_add,
                        }) = MessageContent::decode(&plain)
                        {
                            if is_add {
                                log.react(target_message_id, emoji, &entry.author_id);
                            } else {
                                log.unreact(target_message_id, emoji, &entry.author_id);
                            }
                        }
                    }
                }

                log.append_entry(&channel_id_str, entry.clone());

                // Persist to NodeStore if available.
                if let (Some(store), Some(pk)) = (&state.node_store, community_pk) {
                    let _ = store.append_message(
                        &pk,
                        &channel_ulid,
                        entry.nonce,
                        entry.ciphertext.clone(),
                        entry.message_id.clone(),
                        entry.author_id.clone(),
                        entry.timestamp_ms,
                    );
                }
            }
        }
    }

    // Always notify the frontend that sync is complete (even when history is empty).
    state.broadcaster.send(PushEvent::SyncProgress(
        super::super::push_broadcaster::SyncProgressData {
            channel_id: channel_id_str.clone(),
            progress: 1.0,
        },
    ));

    state
        .broadcaster
        .send(PushEvent::ChannelHistorySynced(ChannelHistorySyncedData {
            channel_id: channel_id_str,
            community_id: community_id_str,
        }));

    info!(
        count = entries.len(),
        channel_id, "integrated channel history from peer"
    );
}
