use std::sync::Arc;

use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::{debug, warn};

use super::super::AppState;
use super::super::{
    push_broadcaster,
    push_broadcaster::PushEvent,
    save_table,
    types::{DmMessageInfo, GetDmHistoryParams, SendDmParams, SetPreferredMailboxCommunityParams},
};
use super::{internal_err, invalid_params, not_found};
use crate::{crypto::dm::DmEnvelope, identity::NodeIdentity, network::NetworkCommand};

pub(super) fn register_dm_methods(module: &mut RpcModule<Arc<AppState>>) -> anyhow::Result<()> {
    module.register_async_method("dm_send", |params, ctx, _| async move {
        let p: SendDmParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        if p.body.is_empty() {
            return Err(invalid_params("body must not be empty"));
        }
        let now = chrono::Utc::now();
        let msg = DmMessageInfo {
            id: ulid::Ulid::new().to_string(),
            peer_id: p.peer_id.clone(),
            author_id: ctx.peer_id.clone(),
            timestamp: now,
            body: p.body.clone(),
            reply_to: p.reply_to.clone(),
            edited_at: None,
        };
        {
            let mut dms = ctx.dms.write().await;
            dms.entry(p.peer_id.clone()).or_default().push(msg.clone());
            save_table(
                &ctx.data_dir.join("dms.json"),
                &*dms,
                ctx.encryption_key.as_ref(),
            );
        }
        // Best-effort P2P delivery: look up recipient's X25519 key, seal a DmEnvelope,
        // and route via the NetworkHandle (direct or via seed relay).
        {
            let recipient_x25519_pk: Option<[u8; 32]> = {
                // First search community member records (most up-to-date source).
                let members = ctx.members.read().await;
                let mut found = None;
                'outer: for list in members.values() {
                    for m in list.values() {
                        if m.user_id.to_string() == p.peer_id {
                            found = Some(m.x25519_public_key);
                            break 'outer;
                        }
                    }
                }
                // Fall back to the dm_peers cache so delivery still works after a
                // shared community has been disbanded.
                if found.is_none() {
                    let dm_peers = ctx.dm_peers.read().await;
                    if let Some(info) = dm_peers.get(&p.peer_id) {
                        found = Some(info.x25519_public_key);
                    }
                }
                found
            };
            if let Some(x25519_pk) = recipient_x25519_pk {
                let sender_sk = NodeIdentity::from_signing_key_bytes(&ctx.signing_key.to_bytes())
                    .x25519_secret();
                let recipient_pk = x25519_dalek::PublicKey::from(x25519_pk);
                let payload = crate::crypto::dm::DmPayload {
                    body: p.body.clone(),
                    reply_to: p.reply_to.clone(),
                    id: msg.id.clone(),
                };
                let payload_bytes =
                    postcard::to_allocvec(&payload).unwrap_or_else(|_| p.body.as_bytes().to_vec());
                match DmEnvelope::seal(&sender_sk, &recipient_pk, &payload_bytes) {
                    Ok(envelope) => {
                        if ctx
                            .swarm_cmd_tx
                            .send(NetworkCommand::SendDm {
                                peer_id: p.peer_id.clone(),
                                recipient_x25519_pk: x25519_pk,
                                envelope,
                            })
                            .await
                            .is_err()
                        {
                            debug!("dm_send: network command channel closed");
                        }
                    }
                    Err(e) => debug!("dm_send: failed to seal DM envelope: {e}"),
                }
            } else {
                debug!(
                    "dm_send: recipient {} not found in any member list",
                    p.peer_id
                );
            }
        }
        ctx.broadcaster
            .send(PushEvent::DmNew(push_broadcaster::DmEventData {
                message: msg.clone(),
            }));
        Ok::<DmMessageInfo, ErrorObjectOwned>(msg)
    })?;

    module.register_async_method("dm_get_history", |params, ctx, _| async move {
        let p: GetDmHistoryParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        let limit = p.limit.unwrap_or(50).min(200) as usize;
        let dms = ctx.dms.read().await;
        let all = dms.get(&p.peer_id).cloned().unwrap_or_default();
        let result: Vec<DmMessageInfo> = if let Some(before_id) = p.before {
            let pos = all
                .iter()
                .position(|m| m.id == before_id)
                .unwrap_or(all.len());
            let start = pos.saturating_sub(limit);
            all[start..pos].to_vec()
        } else {
            let start = all.len().saturating_sub(limit);
            all[start..].to_vec()
        };
        Ok::<Vec<DmMessageInfo>, ErrorObjectOwned>(result)
    })?;

    module.register_async_method("dm_clear_preferred_mailbox", |_params, ctx, _| async move {
        let mut cfg = ctx.config.write().await;
        cfg.preferred_mailbox_node = None;
        if let Err(e) = cfg.save(&ctx.config_path) {
            warn!("failed to persist cleared preferred_mailbox_node: {e:#}");
        }
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method(
        "dm_set_preferred_mailbox_community",
        |params, ctx, _| async move {
            use crate::{identity::NodeIdentity, network::node_addr::NodeAddr};
            let p: SetPreferredMailboxCommunityParams =
                params.parse().map_err(|e| invalid_params(e.to_string()))?;

            // Resolve the community's first seed node address.
            let addr_str = {
                let communities = ctx.communities.read().await;
                let signed = communities
                    .get(&p.community_id)
                    .ok_or_else(|| not_found("community not found"))?;
                signed
                    .manifest
                    .seed_nodes
                    .first()
                    .cloned()
                    .ok_or_else(|| internal_err("community has no seed nodes configured"))?
            };

            // Validate the address parses as a NodeAddr before persisting.
            let addr: NodeAddr = addr_str
                .parse()
                .map_err(|e: anyhow::Error| invalid_params(e.to_string()))?;

            // Persist to config.
            {
                let mut cfg = ctx.config.write().await;
                cfg.preferred_mailbox_node = Some(addr_str.clone());
                if let Err(e) = cfg.save(&ctx.config_path) {
                    warn!("failed to persist preferred_mailbox_node: {e:#}");
                }
            }

            // Derive our X25519 public key and announce the preference to the DHT.
            let our_x25519_pk = NodeIdentity::from_signing_key_bytes(&ctx.signing_key.to_bytes())
                .x25519_public_key_bytes();
            let _ = ctx
                .swarm_cmd_tx
                .send(crate::network::NetworkCommand::AnnouncePreferredMailbox {
                    user_pk: our_x25519_pk,
                    addr,
                })
                .await;

            Ok::<String, ErrorObjectOwned>(addr_str)
        },
    )?;

    Ok(())
}
