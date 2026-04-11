use std::sync::Arc;

use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::{debug, warn};

use super::super::AppState;
use super::super::DmPeerInfo;
use super::super::{
    push_broadcaster,
    push_broadcaster::PushEvent,
    save_table,
    types::{
        DiscardDmParams, DmMessageInfo, GetDmHistoryParams, SendDmParams,
        SetPreferredMailboxCommunityParams,
    },
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
        // and route via the NetworkHandle (direct or via mailbox).
        {
            let mut recipient_x25519_pk: Option<[u8; 32]> = {
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
                found
            };
            // Fall back to dm_peers cache (post-disbandment delivery).
            if recipient_x25519_pk.is_none() {
                let dm_peers = ctx.dm_peers.read().await;
                if let Some(info) = dm_peers.get(&p.peer_id) {
                    recipient_x25519_pk = Some(info.x25519_public_key);
                }
            }
            // DHT peer info lookup: always resolve the peer's direct address for
            // online delivery, and also fill in x25519_pk if it wasn't found locally.
            let mut peer_node_addr: Option<crate::network::NodeAddr> = None;
            if let Some(dht) = &ctx.dht {
                if let Some(pid_bytes) = super::parse_fingerprint_hex(&p.peer_id) {
                    match dht.find_peer_info(pid_bytes).await {
                        Ok(Some(info)) => {
                            debug!("dm_send: found peer info via DHT for {}", p.peer_id);
                            peer_node_addr = Some(info.addr.clone());
                            if recipient_x25519_pk.is_none() {
                                recipient_x25519_pk = Some(info.x25519_pk);
                                // Cache in dm_peers with the peer's real display name.
                                let name = if info.display_name.is_empty() {
                                    p.peer_id[..12].to_string() + "…"
                                } else {
                                    info.display_name.clone()
                                };
                                let mut dm_peers = ctx.dm_peers.write().await;
                                dm_peers.entry(p.peer_id.clone()).or_insert(DmPeerInfo {
                                    display_name: name,
                                    x25519_public_key: info.x25519_pk,
                                });
                            }
                        }
                        Ok(None) => {
                            debug!("dm_send: peer {} not found in DHT peer info", p.peer_id);
                        }
                        Err(e) => {
                            debug!("dm_send: DHT peer info lookup failed: {e}");
                        }
                    }
                }
            }

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
                        // Resolve mailbox address via DHT before sending.
                        let mailbox_addr = if let Some(dht) = &ctx.dht {
                            dht.find_mailbox_peers(x25519_pk)
                                .await
                                .ok()
                                .and_then(|v| v.into_iter().next())
                        } else {
                            None
                        };
                        if ctx
                            .swarm_cmd_tx
                            .send(NetworkCommand::SendDm {
                                peer_id: p.peer_id.clone(),
                                message_id: msg.id.clone(),
                                recipient_x25519_pk: x25519_pk,
                                envelope,
                                mailbox_addr,
                                peer_node_addr,
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
                    "dm_send: recipient {} not found in members, dm_peers, or DHT",
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

            // Announce our mailbox preference to the DHT.
            let our_x25519_pk = NodeIdentity::from_signing_key_bytes(&ctx.signing_key.to_bytes())
                .x25519_public_key_bytes();
            if let Some(dht) = &ctx.dht {
                let dht = dht.clone();
                // Update the DHT self-address hint from the configured mailbox node addr.
                dht.update_self_addr(addr);
                tokio::spawn(async move { dht.register_mailbox(our_x25519_pk).await });
            }

            Ok::<String, ErrorObjectOwned>(addr_str)
        },
    )?;

    // Removes a specific message from the local DM store.
    // Called by the client after a dm_send_failed event so the message is gone on next load.
    module.register_async_method("dm_discard", |params, ctx, _| async move {
        let p: DiscardDmParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        let mut dms = ctx.dms.write().await;
        if let Some(msgs) = dms.get_mut(&p.peer_id) {
            msgs.retain(|m| m.id != p.message_id);
            save_table(
                &ctx.data_dir.join("dms.json"),
                &*dms,
                ctx.encryption_key.as_ref(),
            );
        }
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // Returns the cached display name for a peer from dm_peers, or null if unknown.
    module.register_async_method("dm_peer_name", |params, ctx, _| async move {
        let peer_id: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
        let dm_peers = ctx.dm_peers.read().await;
        let name = dm_peers.get(&peer_id).map(|info| info.display_name.clone());
        Ok::<Option<String>, ErrorObjectOwned>(name)
    })?;

    Ok(())
}
