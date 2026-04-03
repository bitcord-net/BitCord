use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signer as _, Verifier as _};
use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::{debug, warn};

use super::super::AppState;
use super::super::{
    push_broadcaster,
    push_broadcaster::PushEvent,
    remove_community_local, save_table,
    types::{CreateCommunityParams, JoinCommunityParams, UpdateManifestParams},
};
use super::{
    bytes_to_hex, forbidden, internal_err, invalid_params, not_found, parse_fingerprint_hex,
    require_seed_connected, we_are_seed,
};
use crate::{
    identity::NodeIdentity,
    model::{
        community::{CommunityManifest, SignedManifest},
        membership::{MembershipRecord, Role},
        network_event::NetworkEvent,
        types::{CommunityId, UserId},
    },
    network::NetworkCommand,
};

pub(super) fn register_community_methods(
    module: &mut RpcModule<Arc<AppState>>,
) -> anyhow::Result<()> {
    module.register_async_method("community_create", |params, ctx, _| async move {
        let p: CreateCommunityParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;

        // When the node is GossipClient it cannot act as a seed, so require
        // at least one external seed node so peers can reach the community.
        if ctx.config.read().await.node_mode == crate::config::NodeMode::GossipClient
            && p.seed_nodes.is_empty()
        {
            return Err(forbidden(
                "GossipClient mode — add at least one external seed node address",
            ));
        }

        if p.name.is_empty() || p.name.len() > 100 {
            return Err(invalid_params("name must be 1–100 characters"));
        }

        // Decode the local verifying key bytes from the hex string in AppState.
        let pk_bytes: [u8; 32] = (|| {
            let v = (0..ctx.public_key_hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&ctx.public_key_hex[i..i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .ok()?;
            v.try_into().ok()
        })()
        .ok_or_else(|| internal_err("invalid public_key_hex in state"))?;

        // Decode the peer_id hex (SHA-256 of verifying key) into a UserId.
        let admin_bytes: [u8; 32] = (|| {
            let v = (0..ctx.peer_id.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&ctx.peer_id[i..i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .ok()?;
            v.try_into().ok()
        })()
        .ok_or_else(|| internal_err("invalid peer_id in state"))?;

        let seed_nodes = p.seed_nodes;

        // Parse and persist the seed node fingerprint when external seeds are used.
        let seed_fp: [u8; 32] = if !seed_nodes.is_empty() {
            let fp = p
                .seed_fingerprint_hex
                .as_deref()
                .and_then(parse_fingerprint_hex)
                .unwrap_or([0u8; 32]);
            if fp == [0u8; 32] {
                return Err(invalid_params(
                    "seed_fingerprint_hex is required when seed_nodes is non-empty \
                     (64-char hex SHA-256 of the seed node's TLS certificate)",
                ));
            }
            // Persist the fingerprint for all seed addresses.
            {
                let mut fps = ctx.seed_fingerprints.write().await;
                for addr_str in &seed_nodes {
                    fps.insert(addr_str.clone(), fp);
                }
                save_table(
                    &ctx.data_dir.join("seed_fingerprints.json"),
                    &*fps,
                    ctx.encryption_key.as_ref(),
                );
            }
            fp
        } else {
            [0u8; 32]
        };

        let manifest = CommunityManifest {
            id: CommunityId::new(),
            name: p.name,
            description: p.description,
            public_key: pk_bytes,
            created_at: chrono::Utc::now(),
            admin_ids: vec![UserId(admin_bytes)],
            channel_ids: vec![],
            seed_nodes,
            version: 1,
            deleted: false,
        };

        let signed = manifest.sign(&ctx.signing_key);
        let m = &signed.manifest;
        let info = super::super::types::CommunityInfo {
            id: m.id.to_string(),
            name: m.name.clone(),
            description: m.description.clone(),
            public_key_hex: ctx.public_key_hex.clone(),
            admin_ids: m.admin_ids.iter().map(|u| u.to_string()).collect(),
            channel_ids: vec![],
            seed_nodes: m.seed_nodes.clone(),
            version: m.version,
            created_at: m.created_at,
            reachable: true,
            seeded: !m.seed_nodes.is_empty(),
        };
        let community_id = m.id.to_string();
        let community_id_typed = m.id.clone(); // capture before signed is moved
        let version = m.version;
        let channel_ids_for_sub: Vec<String> =
            m.channel_ids.iter().map(|c| c.to_string()).collect();

        // Add ourselves to the members store.
        let display_name_self = ctx
            .identity_state
            .read()
            .await
            .display_name
            .clone()
            .unwrap_or_else(|| community_id[..8.min(community_id.len())].to_string());
        let self_x25519_pk = {
            let key_bytes = ctx.signing_key.to_bytes();
            NodeIdentity::from_signing_key_bytes(&key_bytes).x25519_public_key_bytes()
        };
        let member_joined_at = chrono::Utc::now();
        let member_roles = vec![Role::Admin];
        let member_display_name = display_name_self;
        let member_sig = {
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(&admin_bytes);
            to_sign.extend_from_slice(&community_id_typed.0.to_bytes());
            to_sign.extend_from_slice(member_display_name.as_bytes());
            to_sign.extend_from_slice(&(member_joined_at.timestamp_millis() as u64).to_le_bytes());
            to_sign.extend_from_slice(&postcard::to_allocvec(&member_roles).unwrap_or_default());
            ctx.signing_key.sign(&to_sign).to_bytes().to_vec()
        };
        let self_member = MembershipRecord {
            user_id: UserId(admin_bytes),
            community_id: community_id_typed.clone(),
            display_name: member_display_name,
            avatar_cid: None,
            joined_at: member_joined_at,
            roles: member_roles,
            public_key: pk_bytes,
            x25519_public_key: self_x25519_pk,
            signature: member_sig,
        };

        {
            let mut communities = ctx.communities.write().await;
            communities.insert(community_id.clone(), signed.clone());
            save_table(
                &ctx.data_dir.join("communities.json"),
                &*communities,
                ctx.encryption_key.as_ref(),
            );
        }

        // Persist the hosting password (if any) so reconnects after restart can
        // re-authenticate with password-protected seed nodes.
        if let Some(ref pw) = p.hosting_password {
            let mut passwords = ctx.hosting_passwords.write().await;
            passwords.insert(community_id.clone(), pw.clone());
            save_table(
                &ctx.data_dir.join("hosting_passwords.json"),
                &*passwords,
                ctx.encryption_key.as_ref(),
            );
        }

        // Sync to persistent NodeStore if we are a hosting node.
        if let Some(store) = &ctx.node_store {
            let node_pk: [u8; 32] = (0..ctx.public_key_hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&ctx.public_key_hex[i..i + 2], 16).unwrap_or(0))
                .collect::<Vec<u8>>()
                .try_into()
                .unwrap_or([0u8; 32]);
            let cert =
                crate::crypto::certificate::HostingCert::new(&ctx.signing_key, node_pk, u64::MAX);
            let meta = crate::node::store::CommunityMeta {
                cert,
                manifest: Some(signed.clone()),
                channels: Vec::new(),
                channel_keys: std::collections::HashMap::new(),
                members: std::iter::once((self_member.user_id.to_string(), self_member.clone()))
                    .collect(),
            };
            let _ = store.set_community_meta(&signed.manifest.public_key, &meta);
        }

        // Subscribe to all existing channel GossipSub topics.
        for ch_id in channel_ids_for_sub {
            let topic = format!("/bitcord/channel/{ch_id}/1.0.0");
            let _ = ctx
                .swarm_cmd_tx
                .send(NetworkCommand::Subscribe(topic))
                .await;
        }
        // Subscribe to the community topic for manifest/member updates from peers.
        let community_topic = format!("/bitcord/community/{community_id}/1.0.0");
        let _ = ctx
            .swarm_cmd_tx
            .send(NetworkCommand::Subscribe(community_topic))
            .await;

        // Dial known seed nodes for this community, skipping our own addresses.
        let own_quic_port = ctx.config.read().await.quic_port;
        let own_ips: std::collections::HashSet<String> = {
            let mut ips: std::collections::HashSet<String> = if_addrs::get_if_addrs()
                .unwrap_or_default()
                .into_iter()
                .map(|i| i.ip().to_string())
                .collect();
            ips.insert("127.0.0.1".to_string());
            ips.insert("::1".to_string());
            ips
        };
        for addr_str in &signed.manifest.seed_nodes {
            if let Ok(addr) = addr_str.parse::<crate::network::NodeAddr>() {
                if own_ips.contains(&addr.ip.to_string()) && addr.port == own_quic_port {
                    continue;
                }
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Dial {
                        addr,
                        is_seed: true,
                        join_community: Some((pk_bytes, community_id.clone())),
                        join_community_password: p.hosting_password.clone(),
                        cert_fingerprint: seed_fp,
                    })
                    .await;
            }
        }

        // Announce this node's presence in the new community to the DHT so
        // peers can discover us.  Seedless communities rely entirely on DHT
        // peer discovery since there is no seed node to act as a hub.
        if let Some(dht) = &ctx.dht {
            let dht = dht.clone();
            tokio::spawn(async move { dht.register_community_peer(pk_bytes).await });
        }

        {
            let mut members = ctx.members.write().await;
            let list = members.entry(community_id.clone()).or_default();
            list.insert(self_member.user_id.to_string(), self_member.clone());
            save_table(
                &ctx.data_dir.join("members.json"),
                &*members,
                ctx.encryption_key.as_ref(),
            );
        }
        // Publish MemberJoined on the community topic so seed/relay nodes
        // (and any peers already subscribed) learn about the creator
        // immediately — not only after a reconnect.
        let community_topic_pub = format!("/bitcord/community/{community_id}/1.0.0");
        if let Ok(encoded) = NetworkEvent::MemberJoined(self_member).encode() {
            let _ = ctx
                .swarm_cmd_tx
                .send(NetworkCommand::Publish {
                    topic: community_topic_pub,
                    data: encoded,
                })
                .await;
        }
        ctx.broadcaster.send(PushEvent::CommunityManifestUpdated(
            push_broadcaster::CommunityEventData {
                community_id,
                version,
                reason: String::new(),
            },
        ));
        Ok::<super::super::types::CommunityInfo, ErrorObjectOwned>(info)
    })?;

    module.register_async_method("community_join", |params, ctx, _| async move {
        let p: JoinCommunityParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;

        // Decode base64url invite payload.
        let decoded = general_purpose::URL_SAFE_NO_PAD
            .decode(&p.invite)
            .map_err(|_| invalid_params("invalid invite: bad base64url encoding"))?;

        #[derive(serde::Deserialize)]
        struct InvitePayload {
            community_id: String,
            name: String,
            #[serde(default)]
            description: String,
            #[serde(default)]
            seed_nodes: Vec<String>,
            #[serde(default)]
            public_key_hex: String,
            /// Ed25519 signature (128-char hex) over the canonical invite fields.
            /// Present only in admin-generated invites; absent in legacy/member invites.
            #[serde(default)]
            sig_hex: String,
            /// SHA-256 TLS certificate fingerprint of the seed node (64-char hex).
            /// Used for certificate pinning when connecting to seed nodes.
            #[serde(default)]
            cert_fingerprint_hex: String,
        }

        let invite: InvitePayload = serde_json::from_slice(&decoded)
            .map_err(|_| invalid_params("invalid invite: bad JSON payload"))?;

        if invite.name.is_empty() {
            return Err(invalid_params("invalid invite: missing community name"));
        }

        // Parse community ID (ULID string).
        let community_ulid = ulid::Ulid::from_string(&invite.community_id)
            .map_err(|_| invalid_params("invalid invite: bad community_id"))?;
        let community_id = CommunityId(community_ulid);
        let community_id_str = community_id.to_string();

        // Reject duplicate joins.
        {
            let communities = ctx.communities.read().await;
            if communities.contains_key(&community_id_str) {
                return Err(invalid_params("already a member of this community"));
            }
        }

        // Reject banned users.
        {
            let bans = ctx.bans.read().await;
            if bans
                .get(&community_id_str)
                .map(|list| list.iter().any(|id| *id == ctx.peer_id))
                .unwrap_or(false)
            {
                return Err(forbidden("you have been banned from this community"));
            }
        }

        // Recover public key bytes from hex if provided; fall back to zeroed placeholder.
        let pk_bytes: [u8; 32] = if invite.public_key_hex.len() == 64 {
            (0..64)
                .step_by(2)
                .map(|i| u8::from_str_radix(&invite.public_key_hex[i..i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .ok()
                .and_then(|v| v.try_into().ok())
                .unwrap_or([0u8; 32])
        } else {
            [0u8; 32]
        };

        // Verify the admin's Ed25519 signature on the invite if one is present.
        // Invites generated by community_generate_invite carry a signature; legacy
        // member-shared invites do not.  We only reject on a *bad* signature — a
        // missing one is accepted for backwards compatibility, though the P2P manifest
        // sync will still verify the real admin signature before storing any data.
        if !invite.sig_hex.is_empty() && pk_bytes != [0u8; 32] {
            #[derive(serde::Serialize)]
            struct InviteSigningPayload<'a> {
                community_id: &'a str,
                name: &'a str,
                description: &'a str,
                public_key_hex: &'a str,
                seed_nodes: &'a [String],
            }
            let signing_payload = InviteSigningPayload {
                community_id: &invite.community_id,
                name: &invite.name,
                description: &invite.description,
                public_key_hex: &invite.public_key_hex,
                seed_nodes: &invite.seed_nodes,
            };
            let payload_bytes = serde_json::to_vec(&signing_payload).unwrap_or_default();

            let sig_valid = (|| -> Option<bool> {
                if invite.sig_hex.len() != 128 {
                    return Some(false);
                }
                let sig_bytes: [u8; 64] = (0..128)
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&invite.sig_hex[i..i + 2], 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .ok()?
                    .try_into()
                    .ok()?;
                let vk = ed25519_dalek::VerifyingKey::from_bytes(&pk_bytes).ok()?;
                let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
                Some(vk.verify(&payload_bytes, &sig).is_ok())
            })()
            .unwrap_or(false);

            if !sig_valid {
                return Err(invalid_params(
                    "invalid invite: admin signature verification failed",
                ));
            }
        }

        let manifest = CommunityManifest {
            id: community_id,
            name: invite.name.clone(),
            description: invite.description.clone(),
            public_key: pk_bytes,
            created_at: chrono::Utc::now(),
            admin_ids: vec![],
            channel_ids: vec![],
            seed_nodes: invite.seed_nodes.clone(),
            version: 0,
            deleted: false,
        };

        // Store a placeholder with a zeroed signature.  The real admin-signed
        // manifest will replace this after the first successful P2P sync
        // (handle_manifest_received verifies the admin signature before storing).
        let signed = SignedManifest {
            manifest,
            signature: vec![0u8; 64],
        };
        let m = &signed.manifest;
        let info = super::super::types::CommunityInfo {
            id: m.id.to_string(),
            name: m.name.clone(),
            description: m.description.clone(),
            public_key_hex: invite.public_key_hex.clone(),
            admin_ids: vec![],
            channel_ids: vec![],
            seed_nodes: m.seed_nodes.clone(),
            version: m.version,
            created_at: m.created_at,
            reachable: invite.seed_nodes.is_empty(),
            seeded: !invite.seed_nodes.is_empty(),
        };
        let version = m.version;
        let channel_ids_for_sub: Vec<String> =
            m.channel_ids.iter().map(|c| c.to_string()).collect();
        {
            let mut communities = ctx.communities.write().await;
            communities.insert(community_id_str.clone(), signed);
            save_table(
                &ctx.data_dir.join("communities.json"),
                &*communities,
                ctx.encryption_key.as_ref(),
            );
        }

        // Persist the hosting password (if any) so reconnects after restart can
        // re-authenticate with password-protected seed nodes.
        if let Some(ref pw) = p.hosting_password {
            let mut passwords = ctx.hosting_passwords.write().await;
            passwords.insert(community_id_str.clone(), pw.clone());
            save_table(
                &ctx.data_dir.join("hosting_passwords.json"),
                &*passwords,
                ctx.encryption_key.as_ref(),
            );
        }
        // Subscribe to any channel topics we already know about.
        for ch_id in channel_ids_for_sub {
            let topic = format!("/bitcord/channel/{ch_id}/1.0.0");
            let _ = ctx
                .swarm_cmd_tx
                .send(NetworkCommand::Subscribe(topic))
                .await;
        }
        // Subscribe to the community topic for manifest/member updates from peers.
        let community_topic = format!("/bitcord/community/{community_id_str}/1.0.0");
        let _ = ctx
            .swarm_cmd_tx
            .send(NetworkCommand::Subscribe(community_topic))
            .await;
        // Add ourselves to the local members store (display name may update after manifest sync).
        let self_id_bytes: [u8; 32] = (0..ctx.peer_id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&ctx.peer_id[i..i + 2], 16).unwrap_or(0))
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap_or([0u8; 32]);
        let self_pk_bytes: [u8; 32] = (0..ctx.public_key_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&ctx.public_key_hex[i..i + 2], 16).unwrap_or(0))
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap_or([0u8; 32]);
        let display_name_join = ctx
            .identity_state
            .read()
            .await
            .display_name
            .clone()
            .unwrap_or_else(|| community_id_str[..8.min(community_id_str.len())].to_string());
        let join_x25519_pk = {
            let key_bytes = ctx.signing_key.to_bytes();
            NodeIdentity::from_signing_key_bytes(&key_bytes).x25519_public_key_bytes()
        };
        let join_community_id = ulid::Ulid::from_string(&community_id_str)
            .map(CommunityId)
            .unwrap_or_else(|_| CommunityId::new());
        let join_joined_at = chrono::Utc::now();
        let join_roles = vec![Role::Member];
        let join_sig = {
            let mut to_sign = Vec::new();
            to_sign.extend_from_slice(&self_id_bytes);
            to_sign.extend_from_slice(&join_community_id.0.to_bytes());
            to_sign.extend_from_slice(display_name_join.as_bytes());
            to_sign.extend_from_slice(&(join_joined_at.timestamp_millis() as u64).to_le_bytes());
            to_sign.extend_from_slice(&postcard::to_allocvec(&join_roles).unwrap_or_default());
            ctx.signing_key.sign(&to_sign).to_bytes().to_vec()
        };
        let self_member_join = MembershipRecord {
            user_id: UserId(self_id_bytes),
            community_id: join_community_id,
            display_name: display_name_join,
            avatar_cid: None,
            joined_at: join_joined_at,
            roles: join_roles,
            public_key: self_pk_bytes,
            x25519_public_key: join_x25519_pk,
            signature: join_sig,
        };
        {
            let mut members = ctx.members.write().await;
            let list = members.entry(community_id_str.clone()).or_default();
            list.insert(
                self_member_join.user_id.to_string(),
                self_member_join.clone(),
            );
            save_table(
                &ctx.data_dir.join("members.json"),
                &*members,
                ctx.encryption_key.as_ref(),
            );
        }
        ctx.broadcaster.send(PushEvent::CommunityManifestUpdated(
            push_broadcaster::CommunityEventData {
                community_id: community_id_str.clone(),
                version,
                reason: String::new(),
            },
        ));

        // Publish MemberJoined on the community topic so that the admin and
        // other peers learn about the new member immediately.  Without this,
        // the admin never wraps channel keys for the joiner, and the joiner
        // stays invisible in the member list until the next full sync.
        let community_topic_pub = format!("/bitcord/community/{community_id_str}/1.0.0");
        if let Ok(encoded) = NetworkEvent::MemberJoined(self_member_join.clone()).encode() {
            let _ = ctx
                .swarm_cmd_tx
                .send(NetworkCommand::Publish {
                    topic: community_topic_pub,
                    data: encoded,
                })
                .await;
        }

        // Queue manifest sync for when we connect to seed nodes.
        {
            let mut pending = ctx.pending_manifest_syncs.lock().await;
            if !pending.contains(&community_id_str) {
                pending.push(community_id_str.clone());
            }
        }

        // Proactively try to fetch from any ALREADY connected peers.
        let peers_for_community = {
            let peers_map = ctx.connected_peers.read().await;
            peers_map
                .get(&community_id_str)
                .cloned()
                .unwrap_or_default()
        };
        for peer in peers_for_community {
            let _ = ctx
                .swarm_cmd_tx
                .send(NetworkCommand::FetchManifest {
                    peer_id: peer.peer_id.clone(),
                    community_id: community_id_str.clone(),
                    community_pk: pk_bytes,
                })
                .await;
        }

        // Extract and persist the TLS certificate fingerprint from the invite.
        // Reject invites that contain a fingerprint field but with invalid data
        // (could indicate tampering). Legacy invites without the field are
        // accepted with a warning.
        let invite_cert_fingerprint: [u8; 32] = if invite.cert_fingerprint_hex.len() == 64 {
            match parse_fingerprint_hex(&invite.cert_fingerprint_hex) {
                Some(fp) => fp,
                None => {
                    return Err(invalid_params(
                        "invalid invite: cert_fingerprint_hex is present but malformed",
                    ));
                }
            }
        } else if invite.cert_fingerprint_hex.is_empty() {
            warn!(
                "community_join: invite does not include cert_fingerprint_hex; \
                 seed node connections will not use certificate pinning"
            );
            [0u8; 32]
        } else {
            return Err(invalid_params(
                "invalid invite: cert_fingerprint_hex must be exactly 64 hex characters",
            ));
        };
        if invite_cert_fingerprint != [0u8; 32] {
            let mut fps = ctx.seed_fingerprints.write().await;
            for addr_str in &invite.seed_nodes {
                fps.insert(addr_str.clone(), invite_cert_fingerprint);
            }
            save_table(
                &ctx.data_dir.join("seed_fingerprints.json"),
                &*fps,
                ctx.encryption_key.as_ref(),
            );
        }

        // Dial each seed node.
        let seed_nodes_for_fetch = invite.seed_nodes.clone();
        let swarm_cmd_tx = ctx.swarm_cmd_tx.clone();
        let community_id_for_dht = community_id_str.clone();
        tokio::spawn(async move {
            for addr_str in &seed_nodes_for_fetch {
                let Ok(addr) = addr_str.parse::<crate::network::NodeAddr>() else {
                    warn!(
                        "community_join: failed to parse seed node address: {}",
                        addr_str
                    );
                    continue;
                };
                let _ = swarm_cmd_tx
                    .send(NetworkCommand::Dial {
                        addr,
                        is_seed: true,
                        join_community: Some((pk_bytes, community_id_for_dht.clone())),
                        join_community_password: p.hosting_password.clone(),
                        cert_fingerprint: invite_cert_fingerprint,
                    })
                    .await;
            }
            // Announce our presence and discover existing peers via DHT.
            // This is the primary discovery mechanism for seedless communities
            // and supplements seed-based discovery for seeded ones.
            if let Some(dht) = &ctx.dht {
                let dht_ann = dht.clone();
                tokio::spawn(async move { dht_ann.register_community_peer(pk_bytes).await });
                let dht_disc = dht.clone();
                tokio::spawn(async move {
                    let peers = dht_disc
                        .find_community_peers(pk_bytes)
                        .await
                        .unwrap_or_default();
                    if !peers.is_empty() {
                        let peer_addrs: Vec<([u8; 32], crate::network::NodeAddr)> =
                            peers.into_iter().map(|r| (r.node_pk, r.addr)).collect();
                        let _ = swarm_cmd_tx
                            .send(NetworkCommand::DiscoverAndDial {
                                peers: peer_addrs,
                                community_pk: pk_bytes,
                                community_id: community_id_for_dht,
                            })
                            .await;
                    }
                });
            }
        });

        Ok::<super::super::types::CommunityInfo, ErrorObjectOwned>(info)
    })?;

    // Generate a cryptographically signed invite link for a community.
    // Only admins of the community can call this method.
    module.register_async_method("community_generate_invite", |params, ctx, _| async move {
        let community_id: String = params.one().map_err(|e| invalid_params(e.to_string()))?;

        let signed_manifest = {
            let communities = ctx.communities.read().await;
            communities
                .get(&community_id)
                .cloned()
                .ok_or_else(|| not_found("community not found"))?
        };

        // Verify the caller is an admin.
        let is_admin = signed_manifest
            .manifest
            .admin_ids
            .iter()
            .any(|u| u.to_string() == ctx.peer_id);
        if !is_admin {
            return Err(forbidden(
                "only community admins can generate signed invite links",
            ));
        }

        // Build seed node list from the manifest.
        // We do NOT auto-add our local public address here unless it is already
        // present in the manifest's seed_nodes list. This prevents an admin
        // from accidentally leaking their personal IP when generating invites
        // for a community hosted on a dedicated server.
        let seed_nodes = signed_manifest.manifest.seed_nodes.clone();

        let public_key_hex = bytes_to_hex(&signed_manifest.manifest.public_key);

        // Canonical signing payload — field order here must match verification in
        // community_join.  Use a struct so serde_json serialises fields in declaration
        // order, making the bytes deterministic on both sides.
        #[derive(serde::Serialize)]
        struct InviteSigningPayload<'a> {
            community_id: &'a str,
            name: &'a str,
            description: &'a str,
            public_key_hex: &'a str,
            seed_nodes: &'a [String],
        }
        let signing_payload = InviteSigningPayload {
            community_id: &community_id,
            name: &signed_manifest.manifest.name,
            description: &signed_manifest.manifest.description,
            public_key_hex: &public_key_hex,
            seed_nodes: &seed_nodes,
        };
        let payload_bytes = serde_json::to_vec(&signing_payload)
            .map_err(|e| internal_err(format!("serialization error: {e}")))?;

        let sig_bytes: [u8; 64] = ctx.signing_key.sign(&payload_bytes).to_bytes();
        let sig_hex = bytes_to_hex(&sig_bytes);

        // Determine the TLS certificate fingerprint for certificate pinning.
        // If we have a stored fingerprint for the first seed node (e.g. from
        // community_update_manifest), use that. Otherwise compute our own cert
        // fingerprint (correct when this node IS the seed).
        let cert_fingerprint_hex = {
            let first_seed = seed_nodes.first().cloned().unwrap_or_default();
            let stored_fp = if !first_seed.is_empty() {
                ctx.seed_fingerprints.read().await.get(&first_seed).copied()
            } else {
                None
            };
            if let Some(fp) = stored_fp.filter(|fp| *fp != [0u8; 32]) {
                bytes_to_hex(&fp)
            } else {
                let tls_cert = crate::network::tls::NodeTlsCert::generate(&ctx.signing_key)
                    .map_err(|e| {
                        internal_err(format!("failed to generate TLS cert for fingerprint: {e}"))
                    })?;
                bytes_to_hex(&tls_cert.fingerprint)
            }
        };

        // Build the full invite JSON and base64url-encode it.
        let invite_json = serde_json::json!({
            "community_id": community_id,
            "name": signed_manifest.manifest.name,
            "description": signed_manifest.manifest.description,
            "public_key_hex": public_key_hex,
            "seed_nodes": seed_nodes,
            "sig_hex": sig_hex,
            "cert_fingerprint_hex": cert_fingerprint_hex,
        });
        let json_str = serde_json::to_string(&invite_json)
            .map_err(|e| internal_err(format!("serialization error: {e}")))?;
        let b64 = general_purpose::URL_SAFE_NO_PAD.encode(json_str.as_bytes());

        Ok::<String, ErrorObjectOwned>(format!("bitcord://join/{b64}"))
    })?;

    module.register_async_method("community_leave", |params, ctx, _| async move {
        let community_id: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
        {
            let communities = ctx.communities.read().await;
            if !communities.contains_key(&community_id) {
                return Err(not_found("community not found"));
            }
        }

        // Sign and publish MemberLeft gossip before removing locally so peers learn we left.
        let user_id_bytes: [u8; 32] = (0..ctx.peer_id.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&ctx.peer_id[i..i + 2], 16).unwrap_or(0))
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap_or([0u8; 32]);
        if let Ok(community_ulid) = ulid::Ulid::from_string(&community_id) {
            let community_id_typed = CommunityId(community_ulid);
            let mut msg = Vec::with_capacity(32 + 16 + 5);
            msg.extend_from_slice(&user_id_bytes);
            msg.extend_from_slice(&community_ulid.to_bytes());
            msg.extend_from_slice(b"leave");
            let signature = ctx.signing_key.sign(&msg).to_bytes().to_vec();
            let payload = crate::model::network_event::MemberLeftPayload {
                user_id: UserId(user_id_bytes),
                community_id: community_id_typed,
                timestamp: chrono::Utc::now(),
                signature,
            };
            let community_topic = format!("/bitcord/community/{community_id}/1.0.0");
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

        remove_community_local(&ctx, &community_id).await;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("community_delete", |params, ctx, _| async move {
        let community_id: String = params.one().map_err(|e| invalid_params(e.to_string()))?;

        // Verify admin and build a signed tombstone manifest to broadcast.
        let signed_tombstone = {
            let mut communities = ctx.communities.write().await;
            let signed = communities
                .get_mut(&community_id)
                .ok_or_else(|| not_found("community not found"))?;
            let is_admin = signed
                .manifest
                .admin_ids
                .iter()
                .any(|u| u.to_string() == ctx.peer_id);
            if !is_admin {
                return Err(forbidden("only admins can delete a community"));
            }
            signed.manifest.deleted = true;
            signed.manifest.version += 1;
            let tombstone = signed.manifest.clone().sign(&ctx.signing_key);
            *signed = tombstone.clone();
            tombstone
        };

        // Broadcast the tombstone so connected peers learn the community was deleted.
        let community_topic = format!("/bitcord/community/{community_id}/1.0.0");
        if let Ok(encoded) = NetworkEvent::ManifestUpdate(signed_tombstone).encode() {
            let _ = ctx
                .swarm_cmd_tx
                .send(NetworkCommand::Publish {
                    topic: community_topic,
                    data: encoded,
                })
                .await;
        }

        remove_community_local(&ctx, &community_id).await;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("community_list", |_params, ctx, _| async move {
        let communities = ctx.communities.read().await;
        let seed_connected = ctx.seed_connected_communities.read().await;
        let pub_addr = ctx.public_addr.read().await.clone();
        let listen_addrs = ctx.actual_listen_addrs.read().await.clone();
        let list: Vec<super::super::types::CommunityInfo> = communities
            .values()
            .map(|s| {
                let m = &s.manifest;
                let comm_id = m.id.to_string();
                let self_seeded = !m.seed_nodes.is_empty()
                    && (pub_addr
                        .as_ref()
                        .map(|a| m.seed_nodes.contains(a))
                        .unwrap_or(false)
                        || m.seed_nodes.iter().any(|s| listen_addrs.contains(s)));
                let reachable =
                    m.seed_nodes.is_empty() || self_seeded || seed_connected.contains(&comm_id);
                super::super::types::CommunityInfo {
                    id: comm_id,
                    name: m.name.clone(),
                    description: m.description.clone(),
                    public_key_hex: m.public_key.iter().map(|b| format!("{b:02x}")).collect(),
                    admin_ids: m.admin_ids.iter().map(|u| u.to_string()).collect(),
                    channel_ids: m.channel_ids.iter().map(|c| c.to_string()).collect(),
                    seed_nodes: m.seed_nodes.clone(),
                    version: m.version,
                    created_at: m.created_at,
                    reachable,
                    seeded: !m.seed_nodes.is_empty(),
                }
            })
            .collect();
        Ok::<Vec<super::super::types::CommunityInfo>, ErrorObjectOwned>(list)
    })?;

    module.register_async_method("community_get", |params, ctx, _| async move {
        let community_id: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
        let communities = ctx.communities.read().await;
        let s = communities
            .get(&community_id)
            .ok_or_else(|| not_found("community not found"))?;
        let m = &s.manifest;
        let seed_connected = ctx.seed_connected_communities.read().await;
        let self_seeded = we_are_seed(&ctx, &m.seed_nodes).await;
        let reachable =
            m.seed_nodes.is_empty() || self_seeded || seed_connected.contains(&community_id);
        let info = super::super::types::CommunityInfo {
            id: m.id.to_string(),
            name: m.name.clone(),
            description: m.description.clone(),
            public_key_hex: m.public_key.iter().map(|b| format!("{b:02x}")).collect(),
            admin_ids: m.admin_ids.iter().map(|u| u.to_string()).collect(),
            channel_ids: m.channel_ids.iter().map(|c| c.to_string()).collect(),
            seed_nodes: m.seed_nodes.clone(),
            version: m.version,
            created_at: m.created_at,
            reachable,
            seeded: !m.seed_nodes.is_empty(),
        };
        Ok::<super::super::types::CommunityInfo, ErrorObjectOwned>(info)
    })?;

    module.register_async_method("community_update_manifest", |params, ctx, _| async move {
        let p: UpdateManifestParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        require_seed_connected(&ctx, &p.community_id).await?;

        if let Some(ref name) = p.name {
            if name.is_empty() || name.len() > 100 {
                return Err(invalid_params("name must be 1–100 characters"));
            }
        }
        if let Some(ref desc) = p.description {
            if desc.len() > 500 {
                return Err(invalid_params("description must be ≤500 characters"));
            }
        }

        // Capture old seed_nodes before the update so we can detect changes.
        let old_seed_nodes = {
            let communities = ctx.communities.read().await;
            communities
                .get(&p.community_id)
                .map(|c| c.manifest.seed_nodes.clone())
                .unwrap_or_default()
        };

        let (community_version, updated_manifest) = {
            let mut communities = ctx.communities.write().await;
            let signed = communities
                .get_mut(&p.community_id)
                .ok_or_else(|| not_found("community not found"))?;
            let is_admin = signed
                .manifest
                .admin_ids
                .iter()
                .any(|u| u.to_string() == ctx.peer_id);
            if !is_admin {
                return Err(forbidden("only admins may update the community manifest"));
            }
            let mut updated = signed.manifest.clone();
            if let Some(name) = p.name {
                updated.name = name;
            }
            if let Some(description) = p.description {
                updated.description = description;
            }
            if let Some(seed_nodes) = p.seed_nodes {
                updated.seed_nodes = seed_nodes;
            }
            updated.version += 1;
            let version = updated.version;
            *signed = updated.sign(&ctx.signing_key);
            let cloned = signed.clone();
            save_table(
                &ctx.data_dir.join("communities.json"),
                &*communities,
                ctx.encryption_key.as_ref(),
            );
            (version, cloned)
        };

        // Broadcast the updated manifest to peers via the community GossipSub topic.
        let community_topic = format!("/bitcord/community/{}/1.0.0", p.community_id);
        match NetworkEvent::ManifestUpdate(updated_manifest.clone()).encode() {
            Ok(encoded) => {
                let _ = ctx
                    .swarm_cmd_tx
                    .send(NetworkCommand::Publish {
                        topic: community_topic,
                        data: encoded,
                    })
                    .await;
            }
            Err(e) => debug!("community_update_manifest: failed to encode ManifestUpdate: {e}"),
        }
        ctx.broadcaster.send(PushEvent::CommunityManifestUpdated(
            push_broadcaster::CommunityEventData {
                community_id: p.community_id.clone(),
                version: community_version,
                reason: String::new(),
            },
        ));

        // If seed_nodes changed to a new address, sync all community data to the
        // new seed node so it starts with a complete copy (manifest, channels,
        // keys, members, message history).
        // Only the community creator (whose signing key == community_pk) can sign
        // a valid hosting certificate for the new seed node.
        let new_seed_nodes = &updated_manifest.manifest.seed_nodes;
        let new_seed = new_seed_nodes.first().cloned().unwrap_or_default();
        let old_seed = old_seed_nodes.first().cloned().unwrap_or_default();

        // Persist the new seed node's fingerprint so that future connections
        // — including reconnects and invites — use certificate pinning.
        // Require a valid fingerprint when seed nodes change; without it an
        // attacker could MITM the new connection.
        if !new_seed.is_empty() && new_seed != old_seed {
            let new_fp: [u8; 32] = p
                .seed_fingerprint_hex
                .as_deref()
                .and_then(parse_fingerprint_hex)
                .unwrap_or([0u8; 32]);
            if new_fp == [0u8; 32] {
                return Err(invalid_params(
                    "seed_fingerprint_hex is required when changing seed_nodes \
                     (64-char hex SHA-256 of the new seed node's TLS certificate)",
                ));
            }
            let mut fps = ctx.seed_fingerprints.write().await;
            for addr_str in new_seed_nodes {
                fps.insert(addr_str.clone(), new_fp);
            }
            save_table(
                &ctx.data_dir.join("seed_fingerprints.json"),
                &*fps,
                ctx.encryption_key.as_ref(),
            );
        }

        let is_creator =
            ctx.signing_key.verifying_key().to_bytes() == updated_manifest.manifest.public_key;
        if !new_seed.is_empty() && new_seed != old_seed && is_creator {
            let community_id = p.community_id.clone();
            let community_pk = updated_manifest.manifest.public_key;
            let manifest_box = Box::new(updated_manifest);
            let ctx = Arc::clone(&ctx);
            tokio::spawn(async move {
                if let Err(e) = sync_community_to_seed(
                    &ctx,
                    &community_id,
                    community_pk,
                    manifest_box,
                    &new_seed,
                )
                .await
                {
                    warn!(
                        %community_id,
                        seed = %new_seed,
                        "seed sync failed: {e:#}"
                    );
                }
            });
        }

        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    Ok(())
}

// ── Seed-node data sync ──────────────────────────────────────────────────────

/// Push full community data to a new seed node.
///
/// Connects directly to the seed node, registers the community, and transfers
/// all metadata (manifest, channels, keys, members) followed by the complete
/// message history for every channel.
async fn sync_community_to_seed(
    state: &AppState,
    community_id: &str,
    community_pk: [u8; 32],
    manifest: Box<SignedManifest>,
    seed_addr_str: &str,
) -> anyhow::Result<()> {
    use crate::identity::NodeIdentity;
    use crate::network::NodeAddr;
    use crate::network::client::NodeClient;
    use tracing::info;

    let addr: NodeAddr = seed_addr_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid seed address: {seed_addr_str}"))?;

    let identity = Arc::new(NodeIdentity::from_signing_key_bytes(
        &state.signing_key.to_bytes(),
    ));

    info!(
        %community_id,
        seed = %seed_addr_str,
        "connecting to seed node for community data sync"
    );

    // Look up stored fingerprint for this seed address.
    let cert_fp = state
        .seed_fingerprints
        .read()
        .await
        .get(seed_addr_str)
        .copied()
        .unwrap_or([0u8; 32]);

    let (client, _node_pk, _push_rx) = NodeClient::connect(addr, cert_fp, identity).await?;

    // 1. Join the community on the seed node so it creates a CommunityMeta entry.
    let cert = crate::crypto::certificate::HostingCert::new(&state.signing_key, _node_pk, u64::MAX);
    client
        .join_community(cert, Some(community_id.to_string()), None)
        .await?;

    // 2. Gather community metadata.
    let channels: Vec<_> = {
        let ch_store = state.channels.read().await;
        manifest
            .manifest
            .channel_ids
            .iter()
            .filter_map(|cid| ch_store.get(&cid.to_string()).cloned())
            .collect()
    };

    let channel_keys: std::collections::HashMap<String, Vec<u8>> = {
        let keys = state.channel_keys.read().await;
        keys.iter().map(|(k, v)| (k.clone(), v.to_vec())).collect()
    };

    let members: Vec<_> = {
        let members_store = state.members.read().await;
        members_store
            .get(community_id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    };

    info!(
        %community_id,
        channels = channels.len(),
        members = members.len(),
        "pushing manifest to seed node"
    );

    client
        .push_manifest(
            community_pk,
            manifest.clone(),
            channels.clone(),
            channel_keys,
            members,
        )
        .await?;

    // 3. Push message history for each channel.
    for ch in &channels {
        let ch_id_str = ch.id.to_string();
        let entries = {
            let log = state.message_log.lock().await;
            log.get_since(&ch_id_str, 0).to_vec()
        };

        if entries.is_empty() {
            continue;
        }

        info!(
            %community_id,
            channel_id = %ch_id_str,
            count = entries.len(),
            "pushing channel history to seed node"
        );

        // Send in batches to avoid oversized frames.
        for batch in entries.chunks(500) {
            client
                .push_history(community_pk, ch.id.0, batch.to_vec())
                .await?;
        }
    }

    info!(
        %community_id,
        seed = %seed_addr_str,
        "seed node sync complete"
    );

    Ok(())
}
