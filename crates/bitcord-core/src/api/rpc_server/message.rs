use std::sync::Arc;

use ed25519_dalek::Signer as _;
use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::debug;

use super::super::AppState;
use super::super::push_broadcaster::MemberRoleUpdatedData;
use super::super::{
    push_broadcaster,
    push_broadcaster::{PushEvent, ReactionInfo as PushReactionInfo, ReactionUpdatedData},
    save_table,
    types::{
        DeleteMessageParams, EditMessageParams, GetHistoryParams, KickBanParams, MarkReadParams,
        MemberInfo, MessageInfo, ReactionParams, RoleDto, SendMessageParams, SetStatusParams,
        UpdateRoleParams, UserStatus,
    },
};
use super::{forbidden, internal_err, invalid_params, not_found, require_seed_connected};
use crate::{
    crypto::channel_keys::ChannelKey,
    model::{
        channel::ChannelKind,
        membership::Role,
        message::{MessageContent, RawMessage},
        network_event::{
            DeleteMessagePayload, EditMessagePayload, MemberLeftPayload, MemberRoleUpdatedPayload,
            NetworkEvent, PresenceHeartbeatPayload,
        },
        types::{ChannelId, CommunityId, UserId},
    },
    network::NetworkCommand,
};

pub(super) fn register_message_methods(
    module: &mut RpcModule<Arc<AppState>>,
) -> anyhow::Result<()> {
    module.register_async_method("message_send", |params, ctx, _| async move {
        let p: SendMessageParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        if p.body.is_empty() {
            return Err(invalid_params("body must not be empty"));
        }
        require_seed_connected(&ctx, &p.community_id).await?;

        // Announcement channels: only admins and moderators may post.
        {
            let channels = ctx.channels.read().await;
            if let Some(ch) = channels.get(&p.channel_id) {
                if ch.kind == ChannelKind::Announcement {
                    let members = ctx.members.read().await;
                    let community_members = members
                        .get(&p.community_id)
                        .ok_or_else(|| not_found("community not found"))?;
                    let caller = community_members
                        .get(&ctx.peer_id)
                        .ok_or_else(|| forbidden("not a member of this community"))?;
                    if !caller
                        .roles
                        .iter()
                        .any(|r| matches!(r, Role::Admin | Role::Moderator))
                    {
                        return Err(forbidden(
                            "only admins and moderators can post in announcement channels",
                        ));
                    }
                }
            }
        }

        // Get the channel key.
        let key_bytes: [u8; 32] = {
            let keys = ctx.channel_keys.read().await;
            *keys
                .get(&p.channel_id)
                .ok_or_else(|| not_found("channel not found"))?
        };
        let channel_key = ChannelKey::from_bytes(key_bytes);

        // Serialize MessageContent to msgpack then encrypt.
        let reply_to = p.reply_to.and_then(|s| {
            ulid::Ulid::from_string(&s)
                .ok()
                .map(crate::model::types::MessageId)
        });
        let content = MessageContent::Text {
            body: p.body,
            attachments: vec![],
            reply_to,
            edited_at: None,
        };
        let plaintext = postcard::to_allocvec(&content)
            .map_err(|e| internal_err(format!("serialize message: {e}")))?;
        let (nonce, ciphertext) = channel_key
            .encrypt_message(&plaintext)
            .map_err(|e| internal_err(format!("encrypt message: {e}")))?;

        // Parse channel_id ULID.
        let channel_id = {
            let ulid = ulid::Ulid::from_string(&p.channel_id)
                .map_err(|_| invalid_params("invalid channel_id"))?;
            ChannelId(ulid)
        };

        let raw = RawMessage::create(
            channel_id,
            &ctx.signing_key,
            chrono::Utc::now(),
            ciphertext,
            nonce,
        );

        let msg_id = raw.id.to_string();
        let author_id = raw.author_id.to_string();
        let timestamp = raw.timestamp;

        // Append to the message log.
        {
            let mut log = ctx.message_log.lock().await;
            log.append(
                &p.channel_id,
                msg_id.clone(),
                author_id.clone(),
                timestamp.timestamp_millis(),
                raw.nonce,
                raw.ciphertext.clone(),
            );
        }

        // Persist to NodeStore if available.
        if let Some(store) = &ctx.node_store {
            let communities = ctx.communities.read().await;
            if let Some(comm) = communities.get(&p.community_id) {
                let pk = comm.manifest.public_key;
                let _ = store.append_message(
                    &pk,
                    &raw.channel_id.0,
                    raw.nonce,
                    raw.ciphertext.clone(),
                    msg_id.clone(),
                    author_id.clone(),
                    timestamp.timestamp_millis(),
                );
            }
        }

        // Publish to GossipSub so connected peers receive the message.
        match NetworkEvent::NewMessage(raw).encode() {
            Ok(encoded) => {
                let topic = format!("/bitcord/channel/{}/1.0.0", p.channel_id);
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic,
                        data: encoded,
                    })
                    .await;
            }
            Err(e) => {
                debug!("Failed to encode NetworkEvent for GossipSub: {e}");
            }
        }

        // Broadcast MessageNew push event.
        let author_name = {
            let members = ctx.members.read().await;
            members
                .get(&p.community_id)
                .and_then(|list| list.get(&author_id))
                .map(|m| m.display_name.clone())
        };

        ctx.broadcaster
            .send(PushEvent::MessageNew(push_broadcaster::MessageEventData {
                message_id: msg_id.clone(),
                channel_id: p.channel_id.clone(),
                community_id: p.community_id.clone(),
                author_id: author_id.clone(),
                author_name,
                timestamp,
                body: None,
            }));

        let (body, reply_to) = match content {
            MessageContent::Text { body, reply_to, .. } => (body, reply_to.map(|r| r.to_string())),
            _ => (String::new(), None),
        };
        let info = MessageInfo {
            id: msg_id,
            channel_id: p.channel_id,
            community_id: p.community_id,
            author_id,
            timestamp,
            body,
            reply_to,
            edited_at: None,
            deleted: false,
            reactions: vec![],
        };
        Ok::<MessageInfo, ErrorObjectOwned>(info)
    })?;

    module.register_async_method("message_edit", |params, ctx, _| async move {
        let p: EditMessageParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        if p.body.is_empty() {
            return Err(invalid_params("body must not be empty"));
        }
        require_seed_connected(&ctx, &p.community_id).await?;

        // Get the channel key.
        let key_bytes: [u8; 32] = {
            let keys = ctx.channel_keys.read().await;
            *keys
                .get(&p.channel_id)
                .ok_or_else(|| not_found("channel not found"))?
        };
        let channel_key = ChannelKey::from_bytes(key_bytes);

        let edited_at = chrono::Utc::now();

        // Verify authorship and recover original reply_to/attachments.
        let (reply_to, attachments) = {
            let log = ctx.message_log.lock().await;
            let entry = log
                .get_entry(&p.channel_id, &p.message_id)
                .ok_or_else(|| not_found("message not found"))?;
            if entry.author_id != ctx.peer_id {
                return Err(forbidden("only the author can edit a message"));
            }
            match channel_key.decrypt_message(&entry.nonce, &entry.ciphertext) {
                Ok(plain) => match MessageContent::decode(&plain) {
                    Some(MessageContent::Text {
                        reply_to,
                        attachments,
                        ..
                    }) => (reply_to, attachments),
                    _ => (None, vec![]),
                },
                Err(_) => (None, vec![]),
            }
        };

        // Re-encrypt with updated body and edited_at timestamp.
        let updated_content = MessageContent::Text {
            body: p.body.clone(),
            attachments,
            reply_to,
            edited_at: Some(edited_at),
        };
        let plaintext = postcard::to_allocvec(&updated_content)
            .map_err(|e| internal_err(format!("serialize message: {e}")))?;
        let (new_nonce, new_ciphertext) = channel_key
            .encrypt_message(&plaintext)
            .map_err(|e| internal_err(format!("encrypt message: {e}")))?;

        // Update the log entry in-place.
        {
            let mut log = ctx.message_log.lock().await;
            if !log.edit(
                &p.channel_id,
                &p.message_id,
                new_nonce,
                new_ciphertext.clone(),
            ) {
                return Err(not_found("message not found"));
            }
        }

        // Publish to GossipSub so connected peers receive the edit.
        {
            let message_id = ulid::Ulid::from_string(&p.message_id)
                .map_err(|_| invalid_params("invalid message_id"))?;
            let channel_id = ulid::Ulid::from_string(&p.channel_id)
                .map_err(|_| invalid_params("invalid channel_id"))?;
            let author_bytes: [u8; 32] = (0..ctx.peer_id.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&ctx.peer_id[i..i + 2], 16).unwrap_or(0))
                .collect::<Vec<u8>>()
                .try_into()
                .unwrap_or([0u8; 32]);

            // Sign: message_id || channel_id || new_ciphertext
            let mut sign_data = Vec::new();
            sign_data.extend_from_slice(&message_id.to_bytes());
            sign_data.extend_from_slice(&channel_id.to_bytes());
            sign_data.extend_from_slice(&new_ciphertext);
            let signature = ctx.signing_key.sign(&sign_data).to_bytes().to_vec();

            let payload = EditMessagePayload {
                message_id: crate::model::types::MessageId(message_id),
                channel_id: crate::model::types::ChannelId(channel_id),
                author_id: UserId(author_bytes),
                new_ciphertext,
                new_nonce,
                signature,
                timestamp: edited_at,
            };
            match NetworkEvent::EditMessage(payload).encode() {
                Ok(encoded) => {
                    let topic = format!("/bitcord/channel/{}/1.0.0", p.channel_id);
                    let _ = ctx
                        .swarm_cmd_tx
                        .send(NetworkCommand::Publish {
                            topic,
                            data: encoded,
                        })
                        .await;
                }
                Err(e) => {
                    debug!("Failed to encode EditMessage for GossipSub: {e}");
                }
            }
        }

        // Broadcast MessageEdited push event.
        let author_name = {
            let members = ctx.members.read().await;
            members
                .get(&p.community_id)
                .and_then(|list| list.get(&ctx.peer_id))
                .map(|m| m.display_name.clone())
        };

        ctx.broadcaster.send(PushEvent::MessageEdited(
            push_broadcaster::MessageEventData {
                message_id: p.message_id.clone(),
                channel_id: p.channel_id.clone(),
                community_id: p.community_id.clone(),
                author_id: ctx.peer_id.clone(),
                author_name,
                timestamp: edited_at,
                body: Some(p.body.clone()),
            },
        ));

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("message_delete", |params, ctx, _| async move {
        let p: DeleteMessageParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        require_seed_connected(&ctx, &p.community_id).await?;

        // Verify authorship before tombstoning.
        {
            let log = ctx.message_log.lock().await;
            let entry = log
                .get_entry(&p.channel_id, &p.message_id)
                .ok_or_else(|| not_found("message not found"))?;
            if entry.author_id != ctx.peer_id {
                return Err(forbidden("only the author can delete a message"));
            }
        }

        // Tombstone the entry.
        {
            let mut log = ctx.message_log.lock().await;
            if !log.tombstone(&p.channel_id, &p.message_id) {
                return Err(not_found("message not found"));
            }
        }

        // Publish to GossipSub so connected peers receive the deletion.
        {
            let message_id = ulid::Ulid::from_string(&p.message_id)
                .map_err(|_| invalid_params("invalid message_id"))?;
            let channel_id = ulid::Ulid::from_string(&p.channel_id)
                .map_err(|_| invalid_params("invalid channel_id"))?;
            let author_bytes: [u8; 32] = (0..ctx.peer_id.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&ctx.peer_id[i..i + 2], 16).unwrap_or(0))
                .collect::<Vec<u8>>()
                .try_into()
                .unwrap_or([0u8; 32]);

            // Sign: message_id || channel_id || "delete"
            let mut sign_data = Vec::new();
            sign_data.extend_from_slice(&message_id.to_bytes());
            sign_data.extend_from_slice(&channel_id.to_bytes());
            sign_data.extend_from_slice(b"delete");
            let signature = ctx.signing_key.sign(&sign_data).to_bytes().to_vec();

            let payload = DeleteMessagePayload {
                message_id: crate::model::types::MessageId(message_id),
                channel_id: crate::model::types::ChannelId(channel_id),
                author_id: UserId(author_bytes),
                signature,
                timestamp: chrono::Utc::now(),
            };
            match NetworkEvent::DeleteMessage(payload).encode() {
                Ok(encoded) => {
                    let topic = format!("/bitcord/channel/{}/1.0.0", p.channel_id);
                    let _ = ctx
                        .swarm_cmd_tx
                        .send(NetworkCommand::Publish {
                            topic,
                            data: encoded,
                        })
                        .await;
                }
                Err(e) => {
                    debug!("Failed to encode DeleteMessage for GossipSub: {e}");
                }
            }
        }

        // Broadcast MessageDeleted push event.
        ctx.broadcaster.send(PushEvent::MessageDeleted(
            push_broadcaster::MessageDeletedData {
                message_id: p.message_id.clone(),
                channel_id: p.channel_id.clone(),
                community_id: p.community_id.clone(),
            },
        ));

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("message_get_history", |params, ctx, _| async move {
        let p: GetHistoryParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        let limit = p.limit.unwrap_or(50).min(200) as usize;

        // Get the current and previous channel keys for decryption.
        let key_bytes: Option<[u8; 32]> = ctx.channel_keys.read().await.get(&p.channel_id).copied();
        let prev_key_bytes: Option<[u8; 32]> = ctx
            .previous_channel_keys
            .read()
            .await
            .get(&p.channel_id)
            .copied();

        // Validate channel exists.
        {
            let channels = ctx.channels.read().await;
            if !channels.contains_key(&p.channel_id) {
                return Err(not_found("channel not found"));
            }
        }

        let (entries, reactions_map) = {
            let log = ctx.message_log.lock().await;
            let since = log.len(&p.channel_id).saturating_sub(limit as u64);
            let entries = log.get_since(&p.channel_id, since).to_vec();
            let reactions_map: std::collections::HashMap<String, Vec<(String, Vec<String>)>> =
                entries
                    .iter()
                    .map(|e| (e.message_id.clone(), log.get_reactions(&e.message_id)))
                    .collect();
            (entries, reactions_map)
        };

        // Helper: try decrypting with current key, then fall back to previous
        // (pre-rotation) key so that old messages remain readable after rotation.
        let try_decrypt = |nonce: &[u8; 24], ciphertext: &[u8]| -> Option<Vec<u8>> {
            key_bytes
                .and_then(|kb| {
                    ChannelKey::from_bytes(kb)
                        .decrypt_message(nonce, ciphertext)
                        .ok()
                })
                .or_else(|| {
                    prev_key_bytes.and_then(|kb| {
                        ChannelKey::from_bytes(kb)
                            .decrypt_message(nonce, ciphertext)
                            .ok()
                    })
                })
        };

        let mut messages = Vec::with_capacity(entries.len());
        for entry in entries {
            // Skip reaction log entries — they are not user-visible messages.
            // Reaction state is tracked separately in the reactions cache.
            if !entry.deleted {
                if let Some(plain) = try_decrypt(&entry.nonce, &entry.ciphertext) {
                    if matches!(
                        MessageContent::decode(&plain),
                        Some(MessageContent::Reaction { .. })
                    ) {
                        continue;
                    }
                }
            }

            let (body, edited_at, reply_to) = if entry.deleted {
                (String::new(), None, None)
            } else if let Some(plain) = try_decrypt(&entry.nonce, &entry.ciphertext) {
                match MessageContent::decode(&plain) {
                    Some(MessageContent::Text {
                        body,
                        edited_at,
                        reply_to,
                        ..
                    }) => (body, edited_at, reply_to.map(|r| r.to_string())),
                    _ => (String::new(), None, None),
                }
            } else {
                (String::new(), None, None)
            };
            use chrono::TimeZone;
            let timestamp = chrono::Utc
                .timestamp_millis_opt(entry.timestamp_ms)
                .single()
                .unwrap_or_default();
            let reactions = reactions_map
                .get(&entry.message_id)
                .map(|v| {
                    v.iter()
                        .map(|(emoji, user_ids)| super::super::types::ReactionInfo {
                            emoji: emoji.clone(),
                            user_ids: user_ids.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            messages.push(MessageInfo {
                id: entry.message_id,
                channel_id: p.channel_id.clone(),
                community_id: p.community_id.clone(),
                author_id: entry.author_id,
                timestamp,
                body,
                reply_to,
                edited_at,
                deleted: entry.deleted,
                reactions,
            });
        }
        Ok::<Vec<MessageInfo>, ErrorObjectOwned>(messages)
    })?;

    module.register_async_method("reaction_add", |params, ctx, _| async move {
        let p: ReactionParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        require_seed_connected(&ctx, &p.community_id).await?;

        // Get the channel key for encryption.
        let key_bytes: [u8; 32] = {
            let keys = ctx.channel_keys.read().await;
            *keys
                .get(&p.channel_id)
                .ok_or_else(|| not_found("channel not found"))?
        };
        let channel_key = ChannelKey::from_bytes(key_bytes);

        // Verify the target message exists.
        {
            let log = ctx.message_log.lock().await;
            log.get_entry(&p.channel_id, &p.message_id)
                .ok_or_else(|| not_found("message not found"))?;
        }

        // Build and encrypt a Reaction log entry.
        let content = MessageContent::Reaction {
            target_message_id: p.message_id.clone(),
            emoji: p.emoji.clone(),
            is_add: true,
        };
        let plaintext = postcard::to_allocvec(&content)
            .map_err(|e| internal_err(format!("serialize reaction: {e}")))?;
        let (nonce, ciphertext) = channel_key
            .encrypt_message(&plaintext)
            .map_err(|e| internal_err(format!("encrypt reaction: {e}")))?;

        let channel_id_ulid = {
            let ulid = ulid::Ulid::from_string(&p.channel_id)
                .map_err(|_| invalid_params("invalid channel_id"))?;
            ChannelId(ulid)
        };
        let raw = RawMessage::create(
            channel_id_ulid,
            &ctx.signing_key,
            chrono::Utc::now(),
            ciphertext,
            nonce,
        );
        let entry_id = raw.id.to_string();
        let author_id = raw.author_id.to_string();
        let timestamp_ms = raw.timestamp.timestamp_millis();

        // Append to log and update the in-memory reactions cache.
        let raw_reactions = {
            let mut log = ctx.message_log.lock().await;
            log.append(
                &p.channel_id,
                entry_id.clone(),
                author_id.clone(),
                timestamp_ms,
                raw.nonce,
                raw.ciphertext.clone(),
            );
            log.react(&p.message_id, &p.emoji, &ctx.peer_id);
            log.get_reactions(&p.message_id)
        };

        // Persist to NodeStore.
        if let Some(store) = &ctx.node_store {
            let communities = ctx.communities.read().await;
            if let Some(comm) = communities.get(&p.community_id) {
                let pk = comm.manifest.public_key;
                let _ = store.append_message(
                    &pk,
                    &raw.channel_id.0,
                    raw.nonce,
                    raw.ciphertext.clone(),
                    entry_id,
                    author_id,
                    timestamp_ms,
                );
            }
        }

        // Broadcast via NewMessage so peers receive and persist the reaction entry.
        let channel_topic = format!("/bitcord/channel/{}/1.0.0", p.channel_id);
        if let Ok(encoded) = NetworkEvent::NewMessage(raw).encode() {
            let _ = ctx
                .swarm_cmd_tx
                .send(NetworkCommand::Publish {
                    topic: channel_topic,
                    data: encoded,
                })
                .await;
        }

        let reactions: Vec<PushReactionInfo> = raw_reactions
            .into_iter()
            .map(|(emoji, user_ids)| PushReactionInfo { emoji, user_ids })
            .collect();

        ctx.broadcaster
            .send(PushEvent::ReactionUpdated(ReactionUpdatedData {
                message_id: p.message_id.clone(),
                channel_id: p.channel_id.clone(),
                community_id: p.community_id.clone(),
                reactions,
            }));

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("reaction_remove", |params, ctx, _| async move {
        let p: ReactionParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        require_seed_connected(&ctx, &p.community_id).await?;

        // Get the channel key for encryption.
        let key_bytes: [u8; 32] = {
            let keys = ctx.channel_keys.read().await;
            *keys
                .get(&p.channel_id)
                .ok_or_else(|| not_found("channel not found"))?
        };
        let channel_key = ChannelKey::from_bytes(key_bytes);

        // Verify the target message exists.
        {
            let log = ctx.message_log.lock().await;
            log.get_entry(&p.channel_id, &p.message_id)
                .ok_or_else(|| not_found("message not found"))?;
        }

        // Build and encrypt a Reaction log entry.
        let content = MessageContent::Reaction {
            target_message_id: p.message_id.clone(),
            emoji: p.emoji.clone(),
            is_add: false,
        };
        let plaintext = postcard::to_allocvec(&content)
            .map_err(|e| internal_err(format!("serialize reaction: {e}")))?;
        let (nonce, ciphertext) = channel_key
            .encrypt_message(&plaintext)
            .map_err(|e| internal_err(format!("encrypt reaction: {e}")))?;

        let channel_id_ulid = {
            let ulid = ulid::Ulid::from_string(&p.channel_id)
                .map_err(|_| invalid_params("invalid channel_id"))?;
            ChannelId(ulid)
        };
        let raw = RawMessage::create(
            channel_id_ulid,
            &ctx.signing_key,
            chrono::Utc::now(),
            ciphertext,
            nonce,
        );
        let entry_id = raw.id.to_string();
        let author_id = raw.author_id.to_string();
        let timestamp_ms = raw.timestamp.timestamp_millis();

        // Append to log and update the in-memory reactions cache.
        let raw_reactions = {
            let mut log = ctx.message_log.lock().await;
            log.append(
                &p.channel_id,
                entry_id.clone(),
                author_id.clone(),
                timestamp_ms,
                raw.nonce,
                raw.ciphertext.clone(),
            );
            log.unreact(&p.message_id, &p.emoji, &ctx.peer_id);
            log.get_reactions(&p.message_id)
        };

        // Persist to NodeStore.
        if let Some(store) = &ctx.node_store {
            let communities = ctx.communities.read().await;
            if let Some(comm) = communities.get(&p.community_id) {
                let pk = comm.manifest.public_key;
                let _ = store.append_message(
                    &pk,
                    &raw.channel_id.0,
                    raw.nonce,
                    raw.ciphertext.clone(),
                    entry_id,
                    author_id,
                    timestamp_ms,
                );
            }
        }

        // Broadcast via NewMessage so peers receive and persist the reaction entry.
        let channel_topic = format!("/bitcord/channel/{}/1.0.0", p.channel_id);
        if let Ok(encoded) = NetworkEvent::NewMessage(raw).encode() {
            let _ = ctx
                .swarm_cmd_tx
                .send(NetworkCommand::Publish {
                    topic: channel_topic,
                    data: encoded,
                })
                .await;
        }

        let reactions: Vec<PushReactionInfo> = raw_reactions
            .into_iter()
            .map(|(emoji, user_ids)| PushReactionInfo { emoji, user_ids })
            .collect();

        ctx.broadcaster
            .send(PushEvent::ReactionUpdated(ReactionUpdatedData {
                message_id: p.message_id.clone(),
                channel_id: p.channel_id.clone(),
                community_id: p.community_id.clone(),
                reactions,
            }));

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("mark_read", |params, ctx, _| async move {
        let p: MarkReadParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;

        // Look up the message's sequence number in the in-memory log.
        let seq = {
            let log = ctx.message_log.lock().await;
            log.get_entry(&p.channel_id, &p.message_id).map(|e| e.seq)
        };

        if let Some(seq) = seq {
            let mut rs = ctx.read_state.write().await;
            // Only advance — never regress the read cursor.
            let current = rs.get(&p.channel_id).copied().unwrap_or(0);
            if seq > current {
                rs.insert(p.channel_id.clone(), seq);
                save_table(
                    &ctx.data_dir.join("read_state.json"),
                    &*rs,
                    ctx.encryption_key.as_ref(),
                );
            }
        }

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("presence_heartbeat", |params, ctx, _| async move {
        let p: SetStatusParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        let status_str = match p.status {
            UserStatus::Online => "online",
            UserStatus::Idle => "idle",
            UserStatus::DoNotDisturb => "do_not_disturb",
            UserStatus::Invisible => "invisible",
            UserStatus::Offline => "offline",
        }
        .to_string();

        // Update local presence map and emit PresenceChanged to the connected frontend.
        {
            let mut presence = ctx.presence.write().await;
            presence.insert(ctx.peer_id.clone(), p.status.clone());
        }
        ctx.broadcaster.send(PushEvent::PresenceChanged(
            push_broadcaster::PresenceChangedData {
                user_id: ctx.peer_id.clone(),
                status: status_str.clone(),
                last_seen: chrono::Utc::now(),
            },
        ));

        // Gossip a PresenceHeartbeat NetworkEvent on every community topic so peers
        // can update their presence maps and notify their own frontends.
        let community_ids: Vec<String> = ctx.communities.read().await.keys().cloned().collect();
        let user_id_bytes: [u8; 32] = (0..ctx.peer_id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&ctx.peer_id[i..i + 2], 16).unwrap_or(0))
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap_or([0u8; 32]);
        let timestamp = chrono::Utc::now();
        let ts_ms = timestamp.timestamp_millis() as u64;
        let mut to_sign = Vec::with_capacity(32 + status_str.len() + 8);
        to_sign.extend_from_slice(&user_id_bytes);
        to_sign.extend_from_slice(status_str.as_bytes());
        to_sign.extend_from_slice(&ts_ms.to_le_bytes());
        let signature = ctx.signing_key.sign(&to_sign).to_bytes().to_vec();
        let payload = PresenceHeartbeatPayload {
            user_id: UserId(user_id_bytes),
            status: status_str,
            timestamp,
            signature,
        };
        if let Ok(encoded) = NetworkEvent::PresenceHeartbeat(payload).encode() {
            for community_id in community_ids {
                let topic = format!("/bitcord/community/{community_id}/1.0.0");
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic,
                        data: encoded.clone(),
                    })
                    .await;
            }
        }

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // ── Members ───────────────────────────────────────────────────────────────

    module.register_async_method("member_list", |params, ctx, _| async move {
        let community_id: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
        let members = ctx.members.read().await;
        let list = members.get(&community_id);
        let presence = ctx.presence.read().await;
        let result: Vec<MemberInfo> = list
            .map(|m| m.values())
            .into_iter()
            .flatten()
            .map(|r| {
                let uid_str = r.user_id.to_string();
                let status = presence
                    .get(&uid_str)
                    .cloned()
                    .unwrap_or(UserStatus::Offline);
                MemberInfo {
                    user_id: uid_str,
                    display_name: r.display_name.clone(),
                    avatar_cid: r.avatar_cid.clone(),
                    roles: r
                        .roles
                        .iter()
                        .map(|role| match role {
                            Role::Admin => RoleDto::Admin,
                            Role::Moderator => RoleDto::Moderator,
                            Role::Member => RoleDto::Member,
                        })
                        .collect(),
                    joined_at: r.joined_at,
                    public_key_hex: r.public_key.iter().map(|b| format!("{b:02x}")).collect(),
                    status,
                }
            })
            .collect();
        Ok::<Vec<MemberInfo>, ErrorObjectOwned>(result)
    })?;

    module.register_async_method("member_kick", |params, ctx, _| async move {
        let p: KickBanParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;

        // Verify caller has moderator or admin role in this community.
        {
            let members = ctx.members.read().await;
            let community_members = members
                .get(&p.community_id)
                .ok_or_else(|| not_found("community not found"))?;
            let caller = community_members
                .get(&ctx.peer_id)
                .ok_or_else(|| forbidden("not a member of this community"))?;
            if !caller
                .roles
                .iter()
                .any(|r| matches!(r, Role::Admin | Role::Moderator))
            {
                return Err(forbidden(
                    "insufficient permissions: moderator or admin required",
                ));
            }
        }

        // Remove the target member from the list.
        let removed = {
            let mut members = ctx.members.write().await;
            let community_members = members.entry(p.community_id.clone()).or_default();
            community_members.remove(&p.user_id)
        };
        let removed = removed.ok_or_else(|| not_found("member not found"))?;

        // Persist updated member list.
        {
            let members = ctx.members.read().await;
            save_table(
                &ctx.data_dir.join("members.json"),
                &*members,
                ctx.encryption_key.as_ref(),
            );
        }

        // Publish MemberLeft gossip so peer nodes also remove the kicked member.
        let kicked_user_id_bytes: [u8; 32] = (0..p.user_id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&p.user_id[i..i + 2], 16).unwrap_or(0))
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap_or([0u8; 32]);
        if let Ok(community_ulid) = ulid::Ulid::from_string(&p.community_id) {
            let community_id_typed = CommunityId(community_ulid);
            let mut msg = Vec::with_capacity(32 + 16 + 5);
            msg.extend_from_slice(&kicked_user_id_bytes);
            msg.extend_from_slice(&community_ulid.to_bytes());
            msg.extend_from_slice(b"leave");
            let signature = ctx.signing_key.sign(&msg).to_bytes().to_vec();
            let payload = MemberLeftPayload {
                user_id: UserId(kicked_user_id_bytes),
                community_id: community_id_typed,
                timestamp: chrono::Utc::now(),
                signature,
            };
            let community_topic = format!("/bitcord/community/{}/1.0.0", p.community_id);
            if let Ok(encoded) = NetworkEvent::MemberLeft(payload).encode() {
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: community_topic,
                        data: encoded,
                    })
                    .await;
            }
        }

        // Notify the frontend.
        ctx.broadcaster
            .send(PushEvent::MemberLeft(push_broadcaster::MemberEventData {
                user_id: p.user_id.clone(),
                community_id: p.community_id.clone(),
                display_name: removed.display_name.clone(),
            }));

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("member_ban", |params, ctx, _| async move {
        let p: KickBanParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;

        // Verify caller has admin role.
        {
            let members = ctx.members.read().await;
            let community_members = members
                .get(&p.community_id)
                .ok_or_else(|| not_found("community not found"))?;
            let caller = community_members
                .get(&ctx.peer_id)
                .ok_or_else(|| forbidden("not a member of this community"))?;
            if !caller.roles.iter().any(|r| matches!(r, Role::Admin)) {
                return Err(forbidden("insufficient permissions: admin required"));
            }
        }

        // Remove the target member.
        let removed = {
            let mut members = ctx.members.write().await;
            let community_members = members.entry(p.community_id.clone()).or_default();
            community_members.remove(&p.user_id)
        };
        let removed = removed.ok_or_else(|| not_found("member not found"))?;

        // Persist updated member list.
        {
            let members = ctx.members.read().await;
            save_table(
                &ctx.data_dir.join("members.json"),
                &*members,
                ctx.encryption_key.as_ref(),
            );
        }

        // Record the ban so re-joins can be rejected.
        {
            let mut bans = ctx.bans.write().await;
            bans.entry(p.community_id.clone())
                .or_default()
                .push(p.user_id.clone());
            save_table(
                &ctx.data_dir.join("bans.json"),
                &*bans,
                ctx.encryption_key.as_ref(),
            );
        }

        // Publish MemberLeft gossip so peer nodes also remove the banned member.
        let banned_user_id_bytes: [u8; 32] = (0..p.user_id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&p.user_id[i..i + 2], 16).unwrap_or(0))
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap_or([0u8; 32]);
        if let Ok(community_ulid) = ulid::Ulid::from_string(&p.community_id) {
            let community_id_typed = CommunityId(community_ulid);
            let mut msg = Vec::with_capacity(32 + 16 + 5);
            msg.extend_from_slice(&banned_user_id_bytes);
            msg.extend_from_slice(&community_ulid.to_bytes());
            msg.extend_from_slice(b"leave");
            let signature = ctx.signing_key.sign(&msg).to_bytes().to_vec();
            let payload = MemberLeftPayload {
                user_id: UserId(banned_user_id_bytes),
                community_id: community_id_typed,
                timestamp: chrono::Utc::now(),
                signature,
            };
            let community_topic = format!("/bitcord/community/{}/1.0.0", p.community_id);
            if let Ok(encoded) = NetworkEvent::MemberLeft(payload).encode() {
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: community_topic,
                        data: encoded,
                    })
                    .await;
            }
        }

        // Notify the frontend.
        ctx.broadcaster
            .send(PushEvent::MemberLeft(push_broadcaster::MemberEventData {
                user_id: p.user_id.clone(),
                community_id: p.community_id.clone(),
                display_name: removed.display_name.clone(),
            }));

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("member_update_role", |params, ctx, _| async move {
        let p: UpdateRoleParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;

        // Verify caller has admin role.
        {
            let members = ctx.members.read().await;
            let community_members = members
                .get(&p.community_id)
                .ok_or_else(|| not_found("community not found"))?;
            let caller = community_members
                .get(&ctx.peer_id)
                .ok_or_else(|| forbidden("not a member of this community"))?;
            if !caller.roles.iter().any(|r| matches!(r, Role::Admin)) {
                return Err(forbidden("insufficient permissions: admin required"));
            }

            // Prevent demoting the community creator. Their public key is the
            // community's public key — they are the only node that can wrap
            // channel keys, so removing their admin role would leave the
            // community in a broken state.
            let target = community_members
                .get(&p.user_id)
                .ok_or_else(|| not_found("member not found"))?;
            let communities = ctx.communities.read().await;
            if let Some(signed) = communities.get(&p.community_id) {
                if target.public_key == signed.manifest.public_key {
                    return Err(forbidden("cannot change the role of the community creator"));
                }
            }
        }

        let new_role = match p.role {
            RoleDto::Admin => Role::Admin,
            RoleDto::Moderator => Role::Moderator,
            RoleDto::Member => Role::Member,
        };

        // Update the target member's role.
        let found = {
            let mut members = ctx.members.write().await;
            let community_members = members.entry(p.community_id.clone()).or_default();
            if let Some(member) = community_members.get_mut(&p.user_id) {
                member.roles = vec![new_role.clone()];
                true
            } else {
                false
            }
        };

        if !found {
            return Err(not_found("member not found"));
        }

        // Persist.
        {
            let members = ctx.members.read().await;
            save_table(
                &ctx.data_dir.join("members.json"),
                &*members,
                ctx.encryption_key.as_ref(),
            );
        }

        // Publish MemberRoleUpdated gossip so peer nodes apply the change.
        let target_user_id_bytes: [u8; 32] = (0..p.user_id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&p.user_id[i..i + 2], 16).unwrap_or(0))
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap_or([0u8; 32]);
        if let Ok(community_ulid) = ulid::Ulid::from_string(&p.community_id) {
            let community_id_typed = CommunityId(community_ulid);
            let role_bytes = postcard::to_allocvec(&new_role).unwrap_or_default();
            let mut msg = Vec::with_capacity(32 + 16 + role_bytes.len() + 11);
            msg.extend_from_slice(&target_user_id_bytes);
            msg.extend_from_slice(&community_ulid.to_bytes());
            msg.extend_from_slice(&role_bytes);
            msg.extend_from_slice(b"role_update");
            let signature = ctx.signing_key.sign(&msg).to_bytes().to_vec();
            let payload = MemberRoleUpdatedPayload {
                user_id: UserId(target_user_id_bytes),
                community_id: community_id_typed,
                new_role: new_role.clone(),
                timestamp: chrono::Utc::now(),
                signature,
            };
            let community_topic = format!("/bitcord/community/{}/1.0.0", p.community_id);
            if let Ok(encoded) = NetworkEvent::MemberRoleUpdated(payload).encode() {
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: community_topic,
                        data: encoded,
                    })
                    .await;
            }
        }

        // Notify the frontend.
        let role_str = match new_role {
            Role::Admin => "admin",
            Role::Moderator => "moderator",
            Role::Member => "member",
        };
        ctx.broadcaster
            .send(PushEvent::MemberRoleUpdated(MemberRoleUpdatedData {
                user_id: p.user_id.clone(),
                community_id: p.community_id.clone(),
                new_role: role_str.to_string(),
            }));

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    Ok(())
}
