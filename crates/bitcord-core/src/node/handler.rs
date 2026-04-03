//! Per-connection request handler for the BitCord QUIC node server.
//!
//! Each inbound QUIC connection gets a `ConnectionHandler` that:
//! 1. Relays server-push events back to the client over unidirectional streams.
//! 2. Accepts bidirectional streams, reads one `ClientRequest` frame each,
//!    dispatches to the appropriate handler, and writes one `NodeResponse`.
//!
//! # Session state
//! Authentication and community membership are tracked in `ClientSession`,
//! shared across all streams on the same connection via `Arc<Mutex<…>>`.

use std::sync::Arc;

use ed25519_dalek::{Signature, VerifyingKey};
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, warn};
use ulid::Ulid;

use crate::{
    crypto::{certificate::HostingCert, channel_keys::ChannelKey},
    model::network_event::NetworkEvent,
    network::protocol::{ClientRequest, NodePush, NodeResponse, decode_payload, encode_frame},
    network::{NetworkCommand, NodeAddr},
    node::{NodeServices, store::CommunityMeta, store::NodeStore},
    state::message_log::LogEntry,
};

// ── Session state ─────────────────────────────────────────────────────────────

/// Mutable per-connection state shared across all request streams.
#[derive(Default)]
pub struct ClientSession {
    /// Authenticated client public key. `None` until `Authenticate` succeeds.
    pub client_pk: Option<[u8; 32]>,
    /// Communities this client has joined (by presenting a valid `HostingCert`).
    pub joined_communities: Vec<[u8; 32]>,
}

impl ClientSession {
    pub fn is_authenticated(&self) -> bool {
        self.client_pk.is_some()
    }

    /// Return the first joined community's public key (used for
    /// `SendMessage`/`GetMessages` when no explicit community is specified).
    pub fn current_community(&self) -> Option<[u8; 32]> {
        self.joined_communities.first().copied()
    }
}

// ── Push broadcast tuple ──────────────────────────────────────────────────────

/// A push event with an optional community filter.
///
/// `(Some(community_pk), push)` — deliver only to clients that have joined
/// `community_pk`.  `(None, push)` — deliver to all connected clients.
pub type PushPayload = (Option<[u8; 32]>, NodePush);

// ── ConnectionHandler ─────────────────────────────────────────────────────────

/// Handles all streams for a single inbound QUIC connection.
pub struct ConnectionHandler {
    conn: quinn::Connection,
    services: Arc<NodeServices>,
}

impl ConnectionHandler {
    pub fn new(conn: quinn::Connection, services: Arc<NodeServices>) -> Self {
        Self { conn, services }
    }

    /// Drive the connection to completion.
    ///
    /// Spawns a background task to relay push events and then processes
    /// inbound bidirectional streams (requests) in a loop.
    pub async fn run(self) {
        let session = Arc::new(Mutex::new(ClientSession::default()));

        // ── Push relay task ───────────────────────────────────────────────
        let conn_push = self.conn.clone();
        let mut push_rx = self.services.push_tx.subscribe();
        let session_push = Arc::clone(&session);
        tokio::spawn(async move {
            loop {
                match push_rx.recv().await {
                    Ok((community_filter, push)) => {
                        let should_send = {
                            let sess = session_push.lock().await;
                            match &community_filter {
                                Some(cpk) => sess.joined_communities.contains(cpk),
                                None => sess.is_authenticated(),
                            }
                        };
                        if !should_send {
                            continue;
                        }
                        // Open a fresh unidirectional stream per push event.
                        let Ok(mut send) = conn_push.open_uni().await else {
                            break;
                        };
                        let Ok(frame) = encode_frame(&push) else {
                            continue;
                        };
                        let _ = send.write_all(&frame).await;
                        let _ = send.finish();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("push relay lagged by {n} events");
                    }
                }
            }
        });

        // ── Request loop ──────────────────────────────────────────────────
        loop {
            let (send_stream, mut recv_stream) = match self.conn.accept_bi().await {
                Ok(s) => s,
                Err(e) => {
                    debug!("connection closed: {e}");
                    break;
                }
            };

            // Read length-prefixed request frame.
            let mut len_buf = [0u8; 4];
            if recv_stream.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            // Reject suspiciously large frames (max 4 MiB).
            if len > 4 * 1024 * 1024 {
                warn!("oversized request frame ({len} bytes); closing connection");
                break;
            }
            let mut payload = vec![0u8; len];
            if recv_stream.read_exact(&mut payload).await.is_err() {
                break;
            }

            let req = match decode_payload::<ClientRequest>(&payload) {
                Ok(r) => r,
                Err(e) => {
                    warn!("malformed request: {e}");
                    continue;
                }
            };

            let remote_addr = self.conn.remote_address();
            let resp = Self::dispatch(
                &req,
                Arc::clone(&session),
                Arc::clone(&self.services),
                remote_addr,
            )
            .await;

            // Write length-prefixed response frame.
            let mut send_stream = send_stream;
            match encode_frame(&resp) {
                Ok(frame) => {
                    if send_stream.write_all(&frame).await.is_err() {
                        break;
                    }
                    let _ = send_stream.finish();
                }
                Err(e) => {
                    warn!("failed to encode response: {e}");
                    break;
                }
            }
        }
    }

    // ── Dispatch ──────────────────────────────────────────────────────────

    async fn dispatch(
        req: &ClientRequest,
        session: Arc<Mutex<ClientSession>>,
        services: Arc<NodeServices>,
        remote_addr: std::net::SocketAddr,
    ) -> NodeResponse {
        match req {
            ClientRequest::Authenticate {
                pk,
                sig_r,
                sig_s,
                nonce,
            } => {
                let resp = Self::handle_authenticate(
                    *pk,
                    *sig_r,
                    *sig_s,
                    *nonce,
                    session,
                    services.node_pk,
                )
                .await;
                // Track the authenticated peer in the routing table.
                if matches!(resp, NodeResponse::Authenticated { .. }) {
                    if let Some(dht) = &services.dht {
                        dht.add_known_peer(
                            *pk,
                            NodeAddr::new(remote_addr.ip(), remote_addr.port()),
                        );
                    }
                }
                resp
            }

            ClientRequest::JoinCommunity {
                cert,
                community_id,
                password,
            } => {
                let authenticated = session.lock().await.is_authenticated();
                if !authenticated {
                    return err(401, "not authenticated");
                }
                Self::handle_join_community(
                    cert,
                    community_id.clone(),
                    password.as_deref(),
                    services.join_password.as_deref(),
                    session,
                    Arc::clone(&services),
                )
                .await
            }

            ClientRequest::SendMessage {
                community_pk,
                channel_id,
                nonce,
                ciphertext,
            } => {
                let (authenticated, is_member) = {
                    let s = session.lock().await;
                    (
                        s.is_authenticated(),
                        s.joined_communities.contains(community_pk),
                    )
                };
                if !authenticated {
                    return err(401, "not authenticated");
                }
                if !is_member {
                    return err(403, "not a member of this community on this session");
                }
                Self::handle_send_message(
                    *community_pk,
                    *channel_id,
                    *nonce,
                    ciphertext.clone(),
                    session,
                    Arc::clone(&services.store),
                    services.push_tx.clone(),
                )
                .await
            }

            ClientRequest::GetMessages {
                community_pk,
                channel_id,
                since_seq,
            } => {
                let (authenticated, is_member) = {
                    let s = session.lock().await;
                    (
                        s.is_authenticated(),
                        s.joined_communities.contains(community_pk),
                    )
                };
                if !authenticated {
                    return err(401, "not authenticated");
                }
                if !is_member {
                    return err(403, "not a member of this community on this session");
                }
                match services
                    .store
                    .get_messages(community_pk, channel_id, *since_seq)
                {
                    Ok(entries) => NodeResponse::Messages { entries },
                    Err(e) => err(500, &e.to_string()),
                }
            }

            ClientRequest::SendDm {
                recipient_pk,
                envelope,
            } => {
                let (ok, sender_pk) = {
                    let s = session.lock().await;
                    (s.is_authenticated(), s.client_pk)
                };
                if !ok {
                    return err(401, "not authenticated");
                }
                let sender_pk = sender_pk.unwrap();
                match services.store.append_dm(recipient_pk, &sender_pk, envelope) {
                    Ok(seq) => {
                        // Record that this node holds a mailbox for this recipient
                        // and propagate to K closest DHT peers.
                        if let Some(dht) = services.dht.clone() {
                            let pk = *recipient_pk;
                            tokio::spawn(async move { dht.register_mailbox(pk).await });
                        }
                        // Best-effort push to recipient if they are connected.
                        if let Ok(entries) = services.store.get_dms(recipient_pk, seq) {
                            if let Some(entry) = entries.into_iter().next() {
                                let _ = services.push_tx.send((
                                    None,
                                    NodePush::NewDm {
                                        entry,
                                        recipient_pk: *recipient_pk,
                                    },
                                ));
                            }
                        }
                        NodeResponse::DmAck { seq }
                    }
                    Err(e) => err(500, &e.to_string()),
                }
            }

            ClientRequest::GetDms { since_seq } => {
                let (ok, client_pk) = {
                    let s = session.lock().await;
                    (s.is_authenticated(), s.client_pk)
                };
                if !ok {
                    return err(401, "not authenticated");
                }
                let client_pk = client_pk.unwrap();
                // Mailboxes are keyed by the recipient's Ed25519 verifying key,
                // which is the same key used in SendDm { recipient_pk }.
                match services.store.get_dms(&client_pk, *since_seq) {
                    Ok(entries) => NodeResponse::Dms { entries },
                    Err(e) => err(500, &e.to_string()),
                }
            }

            ClientRequest::FetchManifest { community_pk } => {
                let (authenticated, client_pk) = {
                    let s = session.lock().await;
                    (s.is_authenticated(), s.client_pk)
                };
                if !authenticated {
                    return err(401, "not authenticated");
                }
                match services.store.get_community_meta(community_pk) {
                    Ok(Some(meta)) => {
                        if let Some(manifest) = meta.manifest {
                            // Resolve the requesting client's UserId and X25519 public key
                            // from their membership record so we can wrap their channel keys.
                            let client_crypto = client_pk.and_then(|pk| {
                                meta.members
                                    .values()
                                    .find(|m| m.public_key == pk)
                                    .map(|m| (m.user_id.clone(), m.x25519_public_key))
                            });

                            // Destructure meta to avoid partial-move conflicts in the closure.
                            let stored_keys = meta.channel_keys;
                            let members = meta.members;

                            // Only serve channels that are still listed in the manifest.
                            // Without this filter, deleted channels cached in NodeStore
                            // are re-sent to reconnecting peers and resurrect on restart.
                            let live_channel_ids: std::collections::HashSet<String> = manifest
                                .manifest
                                .channel_ids
                                .iter()
                                .map(|id| id.to_string())
                                .collect();

                            // Build channels with the requesting client's key wrapped
                            // individually via X25519 ECDH + XChaCha20-Poly1305.
                            let channels = meta
                                .channels
                                .into_iter()
                                .filter(|ch| live_channel_ids.contains(&ch.id.to_string()))
                                .map(|mut ch| {
                                    if let Some((ref uid, ref x25519_pk)) = client_crypto {
                                        let ch_id = ch.id.to_string();
                                        if let Some(key_bytes) = stored_keys.get(&ch_id) {
                                            if key_bytes.len() == 32 {
                                                let mut arr = [0u8; 32];
                                                arr.copy_from_slice(key_bytes);
                                                let ck = ChannelKey::from_bytes(arr);
                                                if let Ok(wrapped) =
                                                    ck.encrypt_for_member(x25519_pk)
                                                {
                                                    ch.encrypted_channel_key
                                                        .insert(uid.clone(), wrapped);
                                                }
                                            }
                                        }
                                    }
                                    ch
                                })
                                .collect();

                            NodeResponse::Manifest {
                                manifest: Box::new(manifest),
                                channels,
                                // No longer send plaintext keys; recipients decrypt from
                                // encrypted_channel_key inside each ChannelManifest.
                                channel_keys: std::collections::HashMap::new(),
                                members: members.into_values().collect(),
                            }
                        } else {
                            err(404, "community manifest not found on this node")
                        }
                    }
                    Ok(None) => err(404, "community not found"),
                    Err(e) => err(500, &e.to_string()),
                }
            }

            ClientRequest::FindNode { target_id } => {
                // No authentication required — public DHT operation.
                let (peers, mailbox) = if let Some(dht) = &services.dht {
                    let peers = dht
                        .closest_peers(*target_id, 20)
                        .into_iter()
                        .map(|(id, addr)| (id.0, addr))
                        .collect();
                    let mailbox = dht.lookup_mailbox_local(*target_id);
                    (peers, mailbox)
                } else {
                    (vec![], None)
                };
                NodeResponse::ClosestPeers { peers, mailbox }
            }

            ClientRequest::StoreDhtRecord { user_pk, addr } => {
                // No authentication required — public DHT operation.
                if let Some(dht) = &services.dht {
                    dht.add_mailbox_record(*user_pk, addr.clone());
                }
                NodeResponse::DhtAck
            }

            ClientRequest::StoreCommunityPeer {
                community_pk,
                node_pk,
                addr,
            } => {
                // No authentication required — public DHT operation.
                if let Some(dht) = &services.dht {
                    let record = bitcord_dht::CommunityPeerRecord {
                        node_pk: *node_pk,
                        addr: addr.clone(),
                        announced_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    };
                    dht.add_community_peer_record(*community_pk, record);
                }
                NodeResponse::CommunityPeerAck
            }

            ClientRequest::FindCommunityPeers { community_pk } => {
                // No authentication required — public DHT operation.
                let records = services
                    .dht
                    .as_ref()
                    .map(|d| d.lookup_community_peers_local(*community_pk))
                    .unwrap_or_default();
                NodeResponse::CommunityPeers(records)
            }

            ClientRequest::Heartbeat => NodeResponse::Authenticated {
                pk: services.node_pk,
            },

            ClientRequest::Gossip { topic, data } => {
                let (ok, client_pk) = {
                    let s = session.lock().await;
                    (s.is_authenticated(), s.client_pk)
                };
                if !ok {
                    return err(401, "not authenticated");
                }
                let source = client_pk
                    .map(|pk| pk.iter().map(|b| format!("{b:02x}")).collect::<String>())
                    .unwrap_or_default();

                // If this is a channel message, persist it so late-joining
                // members can retrieve history via GetMessages.
                if topic.starts_with("/bitcord/channel/") {
                    if let Ok(NetworkEvent::NewMessage(msg)) = NetworkEvent::decode(data) {
                        let channel_ulid = msg.channel_id.0;
                        // Find the community that owns this channel.
                        if let Ok(community_pks) = services.store.all_communities() {
                            'outer: for cpk in &community_pks {
                                if let Ok(Some(meta)) = services.store.get_community_meta(cpk) {
                                    if meta.channels.iter().any(|c| c.id.0 == channel_ulid) {
                                        let author_id = msg
                                            .author_id
                                            .0
                                            .iter()
                                            .map(|b| format!("{b:02x}"))
                                            .collect::<String>();
                                        let msg_id = msg.id.to_string();
                                        let _ = services.store.append_message(
                                            cpk,
                                            &channel_ulid,
                                            msg.nonce,
                                            msg.ciphertext,
                                            msg_id,
                                            author_id,
                                            msg.timestamp.timestamp_millis(),
                                        );
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                }

                // Broadcast to all authenticated clients (no community filter).
                let _ = services.push_tx.send((
                    None,
                    NodePush::GossipMessage {
                        topic: topic.clone(),
                        source,
                        data: data.clone(),
                    },
                ));
                NodeResponse::GossipAck
            }

            ClientRequest::PushManifest {
                community_pk,
                manifest,
                channels,
                channel_keys,
                members,
            } => {
                let ok = {
                    let s = session.lock().await;
                    s.is_authenticated() && s.joined_communities.contains(community_pk)
                };
                if !ok {
                    return err(403, "not authenticated or not joined to this community");
                }
                // Update community metadata in persistent store.
                let mut meta = services
                    .store
                    .get_community_meta(community_pk)
                    .unwrap_or(None)
                    .unwrap_or_else(|| CommunityMeta {
                        cert: HostingCert {
                            community_pk: *community_pk,
                            node_pk: services.node_pk,
                            expires_at: u64::MAX,
                            signature: Signature::from_bytes(&[0u8; 64]),
                        },
                        manifest: None,
                        channels: Vec::new(),
                        channel_keys: std::collections::HashMap::new(),
                        members: std::collections::HashMap::new(),
                    });
                meta.manifest = Some(*manifest.clone());
                meta.channels = channels.clone();
                meta.channel_keys = channel_keys.clone();
                meta.members = members
                    .iter()
                    .map(|m| (m.user_id.to_string(), m.clone()))
                    .collect();
                if let Err(e) = services.store.set_community_meta(community_pk, &meta) {
                    return err(500, &format!("failed to store community meta: {e}"));
                }
                NodeResponse::PushAck
            }

            ClientRequest::PushHistory {
                community_pk,
                channel_id,
                entries,
            } => {
                let ok = {
                    let s = session.lock().await;
                    s.is_authenticated() && s.joined_communities.contains(community_pk)
                };
                if !ok {
                    return err(403, "not authenticated or not joined to this community");
                }
                for entry in entries {
                    let _ = services.store.append_message(
                        community_pk,
                        channel_id,
                        entry.nonce,
                        entry.ciphertext.clone(),
                        entry.message_id.clone(),
                        entry.author_id.clone(),
                        entry.timestamp_ms,
                    );
                }
                NodeResponse::PushAck
            }
        }
    }

    // ── Individual handlers ───────────────────────────────────────────────

    async fn handle_authenticate(
        pk: [u8; 32],
        sig_r: [u8; 32],
        sig_s: [u8; 32],
        nonce: [u8; 32],
        session: Arc<Mutex<ClientSession>>,
        node_pk: [u8; 32],
    ) -> NodeResponse {
        let vk = match VerifyingKey::from_bytes(&pk) {
            Ok(vk) => vk,
            Err(_) => return err(400, "invalid public key"),
        };

        let mut sig_bytes = [0u8; 64];
        sig_bytes[..32].copy_from_slice(&sig_r);
        sig_bytes[32..].copy_from_slice(&sig_s);
        let sig = Signature::from_bytes(&sig_bytes);

        // verify_strict performs cofactor-safe verification.
        if vk.verify_strict(&nonce, &sig).is_err() {
            return err(401, "signature verification failed");
        }

        let pk_hex: String = pk.iter().map(|b| format!("{b:02x}")).collect();
        debug!("client authenticated: {pk_hex}");

        let mut sess = session.lock().await;
        sess.client_pk = Some(pk);
        NodeResponse::Authenticated { pk: node_pk }
    }

    async fn handle_join_community(
        cert: &HostingCert,
        community_id_opt: Option<String>,
        provided_password: Option<&str>,
        required_password: Option<&str>,
        session: Arc<Mutex<ClientSession>>,
        services: Arc<NodeServices>,
    ) -> NodeResponse {
        let authenticated_pk = {
            let s = session.lock().await;
            s.client_pk
        };

        // Check if the authenticated client is already a known member of this community.
        // Known members are granted access without a valid hosting cert — this allows
        // members to fetch history using a dummy cert before they have a real one,
        // and avoids blocking on the cert.node_pk field which they cannot populate
        // without knowing the server's key in advance.
        let mut is_member = false;
        if let Ok(Some(meta)) = services.store.get_community_meta(&cert.community_pk) {
            if let Some(client_pk) = authenticated_pk {
                if meta.members.values().any(|m| m.public_key == client_pk) {
                    is_member = true;
                }
                debug!(
                    client_pk = %bs58::encode(client_pk).into_string(),
                    community_pk = %bs58::encode(cert.community_pk).into_string(),
                    member_count = meta.members.len(),
                    is_member,
                    "checking membership for JoinCommunity"
                );
            }
        }

        let community_vk = match VerifyingKey::from_bytes(&cert.community_pk) {
            Ok(vk) => vk,
            Err(_) => return err(400, "invalid community public key in cert"),
        };

        if !is_member {
            // Password check for private nodes — only for non-members.
            // Existing members can reconnect without the password even if the
            // node's join_password was set after they originally joined.
            if let Some(required) = required_password {
                if provided_password.unwrap_or("") != required {
                    let client_id = authenticated_pk
                        .map(|pk| pk.iter().map(|b| format!("{b:02x}")).collect::<String>())
                        .unwrap_or_else(|| "unknown".to_string());
                    warn!(
                        client = %client_id,
                        community_pk = %bs58::encode(cert.community_pk).into_string(),
                        "JoinCommunity rejected: invalid node join password"
                    );
                    return err(403, "invalid node join password");
                }
            }

            // Not a known member — fall back to cert-based auth.
            // Enforce cert pinning: the cert must be addressed to *this* node.
            // Accepting a cert issued for a different node would allow replay attacks
            // where a cert stolen from node A is used to gain hosting rights on node B.
            if cert.node_pk != services.node_pk {
                return err(
                    403,
                    "hosting cert node_pk does not match this node's identity",
                );
            }

            if let Err(e) = cert.verify(&community_vk) {
                return err(403, &format!("not a member and hosting cert invalid: {e}"));
            }
        }

        // Upsert community metadata (only if we have a valid cert, or if it already exists).
        let existing_meta = services
            .store
            .get_community_meta(&cert.community_pk)
            .unwrap_or(None);
        let meta = existing_meta.unwrap_or_else(|| CommunityMeta {
            cert: cert.clone(),
            manifest: None,
            channels: Vec::new(),
            channel_keys: std::collections::HashMap::new(),
            members: std::collections::HashMap::new(),
        });

        // Only store if it's new.
        if services
            .store
            .get_community_meta(&cert.community_pk)
            .unwrap_or(None)
            .is_none()
        {
            let _ = services.store.set_community_meta(&cert.community_pk, &meta);
        }

        // Update session — only notify on the first join for this community in this session.
        let is_new_join = {
            let mut sess = session.lock().await;
            if !sess.joined_communities.contains(&cert.community_pk) {
                sess.joined_communities.push(cert.community_pk);
                true
            } else {
                false
            }
        };

        // Notify the rest of the node so it can subscribe to topics and sync manifest.
        // Only fire on the first join — FetchChannelHistory re-sends JoinCommunity for
        // every channel fetch, so without this guard the notification fires once per channel.
        if is_new_join {
            if let Some(community_id) = community_id_opt {
                let _ = services
                    .swarm_cmd_tx
                    .send(NetworkCommand::NotifyCommunityJoined(
                        cert.community_pk,
                        community_id,
                    ))
                    .await;
            }
        }

        NodeResponse::Authenticated {
            pk: services.node_pk,
        }
    }

    async fn handle_send_message(
        community_pk: [u8; 32],
        channel_id: Ulid,
        nonce: [u8; 24],
        ciphertext: Vec<u8>,
        session: Arc<Mutex<ClientSession>>,
        store: Arc<NodeStore>,
        push_tx: broadcast::Sender<PushPayload>,
    ) -> NodeResponse {
        let author_id = {
            let s = session.lock().await;
            s.client_pk
                .map(|k| k.iter().map(|b| format!("{b:02x}")).collect::<String>())
                .unwrap_or_default()
        };

        let message_id = Ulid::new().to_string();
        let timestamp_ms = chrono::Utc::now().timestamp_millis();

        let seq = match store.append_message(
            &community_pk,
            &channel_id,
            nonce,
            ciphertext.clone(),
            message_id.clone(),
            author_id.clone(),
            timestamp_ms,
        ) {
            Ok(s) => s,
            Err(e) => return err(500, &e.to_string()),
        };

        let entry = LogEntry {
            seq,
            nonce,
            ciphertext,
            message_id,
            author_id,
            timestamp_ms,
            deleted: false,
        };

        // Broadcast to subscribers in this community.
        let _ = push_tx.send((
            Some(community_pk),
            NodePush::NewMessage { channel_id, entry },
        ));

        NodeResponse::MessageAck { seq }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn err(code: u16, msg: &str) -> NodeResponse {
    NodeResponse::Error {
        code,
        msg: msg.to_string(),
    }
}
