use std::sync::Arc;

use ed25519_dalek::Signer as _;
use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::debug;

use super::super::AppState;
use super::super::{
    push_broadcaster,
    push_broadcaster::PushEvent,
    save_table,
    types::{
        ChannelInfo, ChannelKindDto, CreateChannelParams, DeleteChannelParams, RotateKeyParams,
    },
};
use super::{
    channel_manifest_to_info, internal_err, invalid_params, not_found, require_seed_connected,
};
use crate::{
    crypto::channel_keys::ChannelKey,
    model::{
        channel::{ChannelKind, ChannelManifest},
        network_event::{ChannelKeyRotationPayload, ChannelManifestBroadcastPayload, NetworkEvent},
        types::ChannelId,
    },
    network::NetworkCommand,
};

pub(super) fn register_channel_methods(
    module: &mut RpcModule<Arc<AppState>>,
) -> anyhow::Result<()> {
    module.register_async_method("channel_list", |params, ctx, _| async move {
        let community_id: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
        let channels = ctx.channels.read().await;
        let list: Vec<ChannelInfo> = channels
            .values()
            .filter(|c| c.community_id.to_string() == community_id)
            .map(channel_manifest_to_info)
            .collect();
        Ok::<Vec<ChannelInfo>, ErrorObjectOwned>(list)
    })?;

    module.register_async_method("channel_get", |params, ctx, _| async move {
        let channel_id: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
        let channels = ctx.channels.read().await;
        channels
            .get(&channel_id)
            .map(channel_manifest_to_info)
            .ok_or_else(|| not_found("channel not found"))
    })?;

    module.register_async_method("channel_create", |params, ctx, _| async move {
        let p: CreateChannelParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        if p.name.is_empty() || p.name.len() > 100 {
            return Err(invalid_params("name must be 1–100 characters"));
        }
        // Verify the community exists.
        {
            let communities = ctx.communities.read().await;
            if !communities.contains_key(&p.community_id) {
                return Err(not_found("community not found"));
            }
        }
        let community_id: crate::model::types::CommunityId = {
            // Parse the community ULID via its string representation.
            let ulid = ulid::Ulid::from_string(&p.community_id)
                .map_err(|_| invalid_params("invalid community_id"))?;
            crate::model::types::CommunityId(ulid)
        };
        let kind = match p.kind {
            ChannelKindDto::Text => ChannelKind::Text,
            ChannelKindDto::Announcement => ChannelKind::Announcement,
            ChannelKindDto::Voice => ChannelKind::Voice,
        };
        // Generate the channel key first so we can wrap it per-member.
        let channel_key = ChannelKey::generate();
        let key_bytes = *channel_key.as_bytes();
        // Wrap the channel key for every community member using ECIES.
        let encrypted_channel_key = {
            let members = ctx.members.read().await;
            let mut map = std::collections::HashMap::new();
            if let Some(list) = members.get(&p.community_id) {
                for m in list.values() {
                    if let Ok(wrapped) = channel_key.encrypt_for_member(&m.x25519_public_key) {
                        map.insert(m.user_id.clone(), wrapped);
                    }
                }
            }
            map
        };
        let manifest = ChannelManifest {
            id: ChannelId::new(),
            community_id,
            name: p.name,
            kind,
            encrypted_channel_key,
            created_at: chrono::Utc::now(),
            version: 1,
        };
        let channel_id = manifest.id.to_string();
        let info = channel_manifest_to_info(&manifest);
        {
            let mut channels = ctx.channels.write().await;
            channels.insert(channel_id.clone(), manifest.clone());
            save_table(
                &ctx.data_dir.join("channels.json"),
                &*channels,
                ctx.encryption_key.as_ref(),
            );
        }
        {
            let mut keys = ctx.channel_keys.write().await;
            keys.insert(channel_id.clone(), key_bytes);
            save_table(
                &ctx.data_dir.join("channel_keys.json"),
                &*keys,
                ctx.encryption_key.as_ref(),
            );
        }
        // Update the community manifest to include the new channel_id and re-persist.
        let (community_version, updated_manifest) = {
            let mut communities = ctx.communities.write().await;
            if let Some(signed) = communities.get_mut(&p.community_id) {
                let channel_ulid = ulid::Ulid::from_string(&channel_id)
                    .map(crate::model::types::ChannelId)
                    .map_err(|_| internal_err("invalid channel_id generated"))?;
                let mut updated = signed.manifest.clone();
                updated.channel_ids.push(channel_ulid);
                updated.version += 1;
                *signed = updated.sign(&ctx.signing_key);
                let version = signed.manifest.version;
                let cloned = signed.clone(); // end mutable borrow before save_table
                save_table(
                    &ctx.data_dir.join("communities.json"),
                    &*communities,
                    ctx.encryption_key.as_ref(),
                );

                // Sync the new channel to persistent NodeStore if we are a hosting node.
                if let Some(store) = &ctx.node_store {
                    if let Ok(Some(mut meta)) =
                        store.get_community_meta(&cloned.manifest.public_key)
                    {
                        meta.manifest = Some(cloned.clone());
                        meta.channels.push(manifest.clone());
                        meta.channel_keys
                            .insert(channel_id.clone(), key_bytes.to_vec());
                        let _ = store.set_community_meta(&cloned.manifest.public_key, &meta);
                    }
                }

                (version, cloned)
            } else {
                return Err(not_found("community not found"));
            }
        };
        // Broadcast the updated manifest to peers via the community GossipSub topic so they
        // learn about the new channel without needing a full manifest re-fetch.
        let community_topic = format!("/bitcord/community/{}/1.0.0", p.community_id);
        match NetworkEvent::ManifestUpdate(updated_manifest).encode() {
            Ok(encoded) => {
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: community_topic.clone(),
                        data: encoded,
                    })
                    .await;
            }
            Err(e) => debug!("channel_create: failed to encode ManifestUpdate: {e}"),
        }
        // Broadcast the channel manifest + key so relay nodes (which may only have
        // inbound connections from us) can store and forward it without a FetchManifest.
        match NetworkEvent::ChannelManifestBroadcast(ChannelManifestBroadcastPayload {
            manifest: manifest.clone(),
        })
        .encode()
        {
            Ok(encoded) => {
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: community_topic,
                        data: encoded,
                    })
                    .await;
            }
            Err(e) => debug!("channel_create: failed to encode ChannelManifestBroadcast: {e}"),
        }
        ctx.broadcaster.send(PushEvent::CommunityManifestUpdated(
            push_broadcaster::CommunityEventData {
                community_id: p.community_id.clone(),
                version: community_version,
                reason: String::new(),
            },
        ));
        // Subscribe to the new channel's GossipSub topic so we receive messages from peers.
        let channel_topic = format!("/bitcord/channel/{channel_id}/1.0.0");
        let _ = ctx
            .swarm_cmd_tx
            .send(NetworkCommand::Subscribe(channel_topic))
            .await;
        ctx.broadcaster.send(PushEvent::ChannelCreated(
            push_broadcaster::ChannelEventData {
                channel_id: channel_id.clone(),
                community_id: p.community_id,
                name: info.name.clone(),
            },
        ));
        Ok::<ChannelInfo, ErrorObjectOwned>(info)
    })?;

    module.register_async_method("channel_delete", |params, ctx, _| async move {
        let p: DeleteChannelParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;

        require_seed_connected(&ctx, &p.community_id).await?;

        // Look up the channel name (needed for the push event).
        let channel_name = {
            let channels = ctx.channels.read().await;
            channels
                .get(&p.channel_id)
                .map(|c| c.name.clone())
                .ok_or_else(|| not_found("channel not found"))?
        };

        // Remove the channel and its key from state.
        {
            let mut channels = ctx.channels.write().await;
            channels.remove(&p.channel_id);
            save_table(
                &ctx.data_dir.join("channels.json"),
                &*channels,
                ctx.encryption_key.as_ref(),
            );
        }
        {
            let mut keys = ctx.channel_keys.write().await;
            keys.remove(&p.channel_id);
            save_table(
                &ctx.data_dir.join("channel_keys.json"),
                &*keys,
                ctx.encryption_key.as_ref(),
            );
        }

        // Remove the channel ID from the community manifest and re-sign.
        let updated_manifest = {
            let mut communities = ctx.communities.write().await;
            if let Some(signed) = communities.get_mut(&p.community_id) {
                let channel_ulid = ulid::Ulid::from_string(&p.channel_id)
                    .map(crate::model::types::ChannelId)
                    .map_err(|_| invalid_params("invalid channel_id"))?;
                let mut updated = signed.manifest.clone();
                updated.channel_ids.retain(|id| *id != channel_ulid);
                updated.version += 1;
                let new_signed = updated.sign(&ctx.signing_key);
                *signed = new_signed.clone();
                save_table(
                    &ctx.data_dir.join("communities.json"),
                    &*communities,
                    ctx.encryption_key.as_ref(),
                );
                new_signed
            } else {
                return Err(not_found("community not found"));
            }
        };

        // Remove the deleted channel from the persistent NodeStore so it is not
        // re-served to reconnecting peers via FetchManifest after a restart.
        if let Some(store) = &ctx.node_store {
            if let Ok(Some(mut meta)) =
                store.get_community_meta(&updated_manifest.manifest.public_key)
            {
                meta.channels.retain(|c| c.id.to_string() != p.channel_id);
                meta.channel_keys.remove(&p.channel_id);
                let _ = store.set_community_meta(&updated_manifest.manifest.public_key, &meta);
            }
        }

        // Unsubscribe from the channel's GossipSub topic.
        let channel_topic = format!("/bitcord/channel/{}/1.0.0", p.channel_id);
        let _ = ctx
            .swarm_cmd_tx
            .send(NetworkCommand::Unsubscribe(channel_topic))
            .await;

        // Broadcast the updated manifest to peers so they learn about the deletion.
        let community_topic = format!("/bitcord/community/{}/1.0.0", p.community_id);
        match NetworkEvent::ManifestUpdate(updated_manifest).encode() {
            Ok(encoded) => {
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: community_topic,
                        data: encoded,
                    })
                    .await;
            }
            Err(e) => debug!("channel_delete: failed to encode ManifestUpdate: {e}"),
        }

        // Notify all connected frontends.
        ctx.broadcaster.send(PushEvent::ChannelDeleted(
            push_broadcaster::ChannelEventData {
                channel_id: p.channel_id,
                community_id: p.community_id,
                name: channel_name,
            },
        ));

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("channel_rotate_key", |params, ctx, _| async move {
        let p: RotateKeyParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;

        require_seed_connected(&ctx, &p.community_id).await?;

        // Verify the channel exists.
        {
            let channels = ctx.channels.read().await;
            if !channels.contains_key(&p.channel_id) {
                return Err(not_found("channel not found"));
            }
        }

        // Generate a fresh channel key.
        let new_key = ChannelKey::generate();
        let new_key_bytes = *new_key.as_bytes();

        // Persist the new key, retaining the old key for fallback decryption.
        {
            let mut keys = ctx.channel_keys.write().await;
            if let Some(&old) = keys.get(&p.channel_id) {
                if old != new_key_bytes {
                    let mut prev = ctx.previous_channel_keys.write().await;
                    prev.insert(p.channel_id.clone(), old);
                    save_table(
                        &ctx.data_dir.join("previous_channel_keys.json"),
                        &*prev,
                        ctx.encryption_key.as_ref(),
                    );
                }
            }
            keys.insert(p.channel_id.clone(), new_key_bytes);
            save_table(
                &ctx.data_dir.join("channel_keys.json"),
                &*keys,
                ctx.encryption_key.as_ref(),
            );
        }

        // Re-wrap the new key for every community member using ECIES.
        let new_encrypted_keys = {
            let members = ctx.members.read().await;
            let mut map = std::collections::HashMap::new();
            if let Some(list) = members.get(&p.community_id) {
                for m in list.values() {
                    if let Ok(wrapped) = new_key.encrypt_for_member(&m.x25519_public_key) {
                        map.insert(m.user_id.clone(), wrapped);
                    }
                }
            }
            map
        };
        // Update the channel manifest: bump version, populate per-member key wraps.
        let updated_manifest = {
            let mut channels = ctx.channels.write().await;
            let manifest = channels
                .get_mut(&p.channel_id)
                .ok_or_else(|| not_found("channel not found"))?;
            manifest.version += 1;
            manifest.encrypted_channel_key = new_encrypted_keys;
            let cloned = manifest.clone();
            save_table(
                &ctx.data_dir.join("channels.json"),
                &*channels,
                ctx.encryption_key.as_ref(),
            );
            cloned
        };

        // Sync the new key and updated channel manifest to NodeStore so the QUIC
        // server wraps the correct key when peers do FetchManifest after rotation.
        if let Some(store) = &ctx.node_store {
            let community_pk = {
                let communities = ctx.communities.read().await;
                communities
                    .get(&p.community_id)
                    .map(|c| c.manifest.public_key)
            };
            if let Some(pk) = community_pk {
                if let Ok(Some(mut meta)) = store.get_community_meta(&pk) {
                    meta.channel_keys
                        .insert(p.channel_id.clone(), new_key_bytes.to_vec());
                    if let Some(ch) = meta
                        .channels
                        .iter_mut()
                        .find(|c| c.id.to_string() == p.channel_id)
                    {
                        *ch = updated_manifest.clone();
                    }
                    let _ = store.set_community_meta(&pk, &meta);
                }
            }
        }

        // Sign and broadcast the rotation event on the channel's gossip topic.
        let encoded_manifest = postcard::to_allocvec(&updated_manifest)
            .map_err(|e| internal_err(format!("failed to encode manifest: {e}")))?;
        let signature: Vec<u8> = ctx.signing_key.sign(&encoded_manifest).to_bytes().to_vec();
        let rotation_event = NetworkEvent::ChannelKeyRotation(ChannelKeyRotationPayload {
            new_manifest: updated_manifest.clone(),
            signature,
        });
        match rotation_event.encode() {
            Ok(encoded) => {
                let channel_topic = format!("/bitcord/channel/{}/1.0.0", p.channel_id);
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: channel_topic,
                        data: encoded,
                    })
                    .await;
            }
            Err(e) => debug!("channel_rotate_key: failed to encode ChannelKeyRotation: {e}"),
        }

        // Push the new key to relay/seed nodes via ChannelManifestBroadcast on the
        // community topic.  Relay nodes update their NodeStore channel_keys from this
        // event and use it to wrap the key for members on subsequent FetchManifest
        // requests.  Without this, relays keep the old key and members that route
        // FetchManifest through a relay can never obtain the new key, causing the
        // admin's post-rotation messages to remain buffered and undeliverable.
        match NetworkEvent::ChannelManifestBroadcast(ChannelManifestBroadcastPayload {
            manifest: updated_manifest,
        })
        .encode()
        {
            Ok(encoded) => {
                let community_topic = format!("/bitcord/community/{}/1.0.0", p.community_id);
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: community_topic,
                        data: encoded,
                    })
                    .await;
            }
            Err(e) => {
                debug!("channel_rotate_key: failed to encode ChannelManifestBroadcast: {e}")
            }
        }

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    Ok(())
}
