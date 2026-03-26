use std::sync::Arc;

use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::debug;

use super::super::AppState;
use super::super::push_broadcaster::PushEvent;
use super::super::{
    push_broadcaster, save_table,
    types::{ChangePassphraseParams, IdentityInfo, SetDisplayNameParams, SetStatusParams},
};
use super::{internal_err, invalid_params};
use crate::{
    identity::{NodeIdentity, keystore::KeyStore},
    model::{membership::MembershipRecord, network_event::NetworkEvent},
    network::NetworkCommand,
};

pub(super) fn register_identity_methods(
    module: &mut RpcModule<Arc<AppState>>,
) -> anyhow::Result<()> {
    module.register_async_method("identity_get", |_params, ctx, _| async move {
        let id_state = ctx.identity_state.read().await;
        let public_addr = ctx.public_addr.read().await.clone();
        let info = IdentityInfo {
            peer_id: ctx.peer_id.clone(),
            display_name: id_state.display_name.clone(),
            status: id_state.status.clone(),
            public_key_hex: ctx.public_key_hex.clone(),
            public_addr,
            tls_fingerprint_hex: ctx.local_tls_fingerprint_hex.clone(),
        };
        Ok::<IdentityInfo, ErrorObjectOwned>(info)
    })?;

    module.register_async_method("identity_set_display_name", |params, ctx, _| async move {
        let p: SetDisplayNameParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        if p.display_name.is_empty() || p.display_name.len() > 64 {
            return Err(invalid_params("display_name must be 1–64 characters"));
        }
        ctx.identity_state.write().await.display_name = Some(p.display_name.clone());
        {
            let mut cfg = ctx.config.write().await;
            cfg.display_name = Some(p.display_name.clone());
            if let Err(e) = cfg.save(&ctx.config_path) {
                debug!("Failed to persist display_name to config: {e}");
            }
        }
        // Update display name in all community member records.
        let updated_records: Vec<(String, MembershipRecord)> = {
            let mut members = ctx.members.write().await;
            let mut records = Vec::new();
            for (community_id, list) in members.iter_mut() {
                if let Some(rec) = list.get_mut(&ctx.peer_id) {
                    rec.display_name = p.display_name.clone();
                    records.push((community_id.clone(), rec.clone()));
                }
            }
            save_table(
                &ctx.data_dir.join("members.json"),
                &*members,
                ctx.encryption_key.as_ref(),
            );
            records
        };
        // Notify the local frontend and gossip the updated name to all peers.
        for (community_id, record) in updated_records {
            ctx.broadcaster
                .send(PushEvent::MemberJoined(push_broadcaster::MemberEventData {
                    user_id: ctx.peer_id.clone(),
                    community_id: community_id.clone(),
                    display_name: record.display_name.clone(),
                }));
            let topic = format!("/bitcord/community/{community_id}/1.0.0");
            if let Ok(encoded) = NetworkEvent::MemberJoined(record).encode() {
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic,
                        data: encoded,
                    })
                    .await;
            }
        }
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("identity_set_status", |params, ctx, _| async move {
        let p: SetStatusParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        let status = p.status.clone();
        ctx.identity_state.write().await.status = p.status;
        ctx.broadcaster.send(PushEvent::PresenceChanged(
            push_broadcaster::PresenceChangedData {
                user_id: ctx.peer_id.clone(),
                status: format!("{:?}", status).to_lowercase(),
                last_seen: chrono::Utc::now(),
            },
        ));
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("identity_change_passphrase", |params, ctx, _| async move {
        let p: ChangePassphraseParams =
            params.parse().map_err(|e| invalid_params(e.to_string()))?;
        if p.new_passphrase.len() < 8 {
            return Err(invalid_params(
                "new passphrase must be at least 8 characters",
            ));
        }
        let identity_path = ctx.config.read().await.identity_path.clone();
        // Verify old passphrase by attempting to decrypt the keystore.
        KeyStore::load(&identity_path, &p.old_passphrase)
            .map_err(|_| invalid_params("incorrect current passphrase"))?;
        // Re-encrypt the keystore under the new passphrase.
        let key_bytes = ctx.signing_key.to_bytes();
        let identity = NodeIdentity::from_signing_key_bytes(&key_bytes);
        KeyStore::save(&identity_path, &identity, &p.new_passphrase)
            .map_err(|e| internal_err(format!("failed to save keystore: {e}")))?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    Ok(())
}
