//! Integration tests for the community role and permission system.
//!
//! Each test spins up a real `ApiServer` with pre-seeded in-memory state
//! (no swarm, no disk I/O), connects a JSON-RPC WebSocket client, and
//! exercises the permission checks for `member_update_role`, `member_kick`,
//! `member_ban`, and announcement-channel `message_send`.

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use bitcord_core::{
    api::{ApiHandle, ApiServer, AppState},
    config::NodeConfig,
    identity::{NodeIdentity, SigningKey},
    model::{
        channel::{ChannelKind, ChannelManifest},
        community::CommunityManifest,
        membership::{MembershipRecord, Role},
        types::{ChannelId, CommunityId, UserId},
    },
    network::NetworkHandle,
    resource::metrics::NodeMetrics,
    state::MessageLog,
};
use chrono::Utc;
use jsonrpsee::{
    core::{client::ClientT, params::ObjectParams},
    ws_client::WsClientBuilder,
};
use serde_json::Value;
use tempfile::TempDir;

// ── Helpers ───────────────────────────────────────────────────────────────────

struct TestNode {
    state: Arc<AppState>,
    /// hex(SHA-256(verifying_key)) — used as the HashMap key in state.members
    peer_id: String,
    /// raw Ed25519 verifying key bytes — used as `public_key` in MembershipRecord
    vk_bytes: [u8; 32],
    /// kept so we can sign community manifests as this node's identity
    signing_key: SigningKey,
}

fn make_test_node(tmp: &TempDir) -> TestNode {
    let identity = NodeIdentity::generate();
    let peer_id = identity.to_peer_id().to_string();
    let vk_bytes = identity.verifying_key().to_bytes();
    let public_key_hex = vk_bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let node_address = identity.node_address();
    let signing_key = SigningKey::from_bytes(&identity.signing_key_bytes());
    let message_log = MessageLog::new();
    let identity_arc = Arc::new(identity);
    let (swarm_cmd_tx, _) = NetworkHandle::spawn(Arc::clone(&identity_arc), vec![], None);
    let metrics = Arc::new(NodeMetrics::default());
    let config = NodeConfig {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let config_path = tmp.path().join("node.toml");
    let sk_clone = SigningKey::from_bytes(&identity_arc.signing_key_bytes());
    let state = AppState::new(
        peer_id.clone(),
        public_key_hex,
        node_address,
        sk_clone,
        config,
        config_path,
        message_log,
        swarm_cmd_tx,
        metrics,
        None,
        None,
        None, // no TLS server in tests
        None, // no DHT in tests
    );
    TestNode {
        state: Arc::new(state),
        peer_id,
        vk_bytes,
        signing_key,
    }
}

/// Seed a community manifest signed by `creator_key` into `state.communities`.
/// `seed_nodes` is always empty so `require_seed_connected` passes without a
/// real network connection.  Returns the community_id string.
async fn seed_community(state: &Arc<AppState>, creator_key: &SigningKey) -> String {
    let vk = creator_key.verifying_key();
    let admin_id = UserId::from_verifying_key(&vk);
    let community_id = CommunityId::new();
    let manifest = CommunityManifest {
        id: community_id.clone(),
        name: "Test Community".into(),
        description: String::new(),
        public_key: vk.to_bytes(),
        created_at: Utc::now(),
        admin_ids: vec![admin_id],
        channel_ids: vec![],
        seed_nodes: vec![],
        version: 1,
        deleted: false,
    };
    let community_id_str = community_id.to_string();
    state
        .communities
        .write()
        .await
        .insert(community_id_str.clone(), manifest.sign(creator_key));
    community_id_str
}

/// Seed a member record directly into `state.members`.
///
/// `user_id_hex` is the 64-char lowercase hex of SHA-256(verifying_key) — the
/// same format as `NodeIdentity::to_peer_id().to_string()`.
/// `public_key` is the raw Ed25519 verifying key bytes (used for the creator
/// guard comparison `target.public_key == manifest.public_key`).
async fn seed_member(
    state: &Arc<AppState>,
    community_id: &str,
    user_id_hex: &str,
    public_key: [u8; 32],
    roles: Vec<Role>,
) {
    let uid_bytes: [u8; 32] = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&user_id_hex[i..i + 2], 16).unwrap())
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    let record = MembershipRecord {
        user_id: UserId(uid_bytes),
        // community_id field is metadata only — not checked by any permission gate
        community_id: CommunityId::new(),
        display_name: "TestUser".into(),
        avatar_cid: None,
        joined_at: Utc::now(),
        roles,
        public_key,
        x25519_public_key: [0u8; 32],
        // signature is not verified by RPC handlers (only by gossip processor)
        signature: vec![0u8; 64],
    };
    state
        .members
        .write()
        .await
        .entry(community_id.to_string())
        .or_default()
        .insert(user_id_hex.to_string(), record);
}

/// Generate a fresh identity and return its (user_id_hex, vk_bytes).
fn make_member() -> (String, [u8; 32]) {
    let identity = NodeIdentity::generate();
    (
        identity.to_peer_id().to_string(),
        identity.verifying_key().to_bytes(),
    )
}

/// Seed an announcement channel into `state.channels` and its key into
/// `state.channel_keys`.  Returns the channel_id string.
async fn seed_announcement_channel(state: &Arc<AppState>, _community_id: &str) -> String {
    let channel_id = ChannelId::new();
    let channel_id_str = channel_id.to_string();
    let manifest = ChannelManifest {
        id: channel_id,
        // community_id field is metadata only in the manifest
        community_id: CommunityId::new(),
        name: "announcements".into(),
        kind: ChannelKind::Announcement,
        encrypted_channel_key: HashMap::new(),
        created_at: Utc::now(),
        version: 1,
    };
    state
        .channels
        .write()
        .await
        .insert(channel_id_str.clone(), manifest);
    // A real 32-byte key so encryption succeeds for admin/mod send tests.
    state
        .channel_keys
        .write()
        .await
        .insert(channel_id_str.clone(), [7u8; 32]);
    channel_id_str
}

/// Start the API server and return a connected WS client.
async fn start(state: Arc<AppState>) -> (ApiHandle, jsonrpsee::ws_client::WsClient) {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, state).await.expect("start server");
    let ws_url = format!("ws://{}", handle.local_addr());
    let client = WsClientBuilder::default()
        .build(&ws_url)
        .await
        .expect("ws connect");
    (handle, client)
}

// ── member_update_role ────────────────────────────────────────────────────────

#[tokio::test]
async fn promote_to_moderator_succeeds_for_admin() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Admin],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    p.insert("role", "moderator").unwrap();
    let result: Value = client
        .request("member_update_role", p)
        .await
        .expect("rpc call");
    assert_eq!(result, Value::Bool(true));

    let members = node.state.members.read().await;
    let roles = &members[&community_id][&target_id].roles;
    assert!(roles.contains(&Role::Moderator));
    handle.stop();
}

#[tokio::test]
async fn promote_to_admin_succeeds_for_admin() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Admin],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    p.insert("role", "admin").unwrap();
    let result: Value = client
        .request("member_update_role", p)
        .await
        .expect("rpc call");
    assert_eq!(result, Value::Bool(true));

    let members = node.state.members.read().await;
    let roles = &members[&community_id][&target_id].roles;
    assert!(roles.contains(&Role::Admin));
    handle.stop();
}

#[tokio::test]
async fn demote_moderator_to_member_succeeds_for_admin() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Admin],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Moderator],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    p.insert("role", "member").unwrap();
    let result: Value = client
        .request("member_update_role", p)
        .await
        .expect("rpc call");
    assert_eq!(result, Value::Bool(true));

    let members = node.state.members.read().await;
    let roles = &members[&community_id][&target_id].roles;
    assert!(roles.contains(&Role::Member));
    handle.stop();
}

#[tokio::test]
async fn update_role_forbidden_for_moderator() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Moderator],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    p.insert("role", "moderator").unwrap();
    let err = client
        .request::<Value, _>("member_update_role", p)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("admin required"),
        "unexpected error: {err}"
    );
    handle.stop();
}

#[tokio::test]
async fn update_role_forbidden_for_plain_member() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Member],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    p.insert("role", "admin").unwrap();
    let err = client
        .request::<Value, _>("member_update_role", p)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("admin required"),
        "unexpected error: {err}"
    );
    handle.stop();
}

#[tokio::test]
async fn update_role_forbidden_for_community_creator() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    // Community is signed by a separate creator identity, not the node itself.
    let creator_identity = NodeIdentity::generate();
    let creator_signing_key = SigningKey::from_bytes(&creator_identity.signing_key_bytes());
    let creator_id = creator_identity.to_peer_id().to_string();
    let creator_vk_bytes = creator_identity.verifying_key().to_bytes();

    let community_id = seed_community(&node.state, &creator_signing_key).await;

    // The node is an admin (promoted), but not the creator.
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Admin],
    )
    .await;
    // The creator is also a member; their public_key matches manifest.public_key.
    seed_member(
        &node.state,
        &community_id,
        &creator_id,
        creator_vk_bytes,
        vec![Role::Admin],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &creator_id).unwrap();
    p.insert("role", "member").unwrap();
    let err = client
        .request::<Value, _>("member_update_role", p)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("community creator"),
        "unexpected error: {err}"
    );
    handle.stop();
}

// ── member_kick ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn kick_succeeds_for_admin() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Admin],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    let result: Value = client.request("member_kick", p).await.expect("rpc call");
    assert_eq!(result, Value::Bool(true));

    // Member should be gone from state.
    let members = node.state.members.read().await;
    assert!(!members[&community_id].contains_key(&target_id));
    handle.stop();
}

#[tokio::test]
async fn kick_succeeds_for_moderator() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Moderator],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    let result: Value = client.request("member_kick", p).await.expect("rpc call");
    assert_eq!(result, Value::Bool(true));
    handle.stop();
}

#[tokio::test]
async fn kick_forbidden_for_plain_member() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Member],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    let err = client
        .request::<Value, _>("member_kick", p)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("moderator or admin required"),
        "unexpected error: {err}"
    );
    handle.stop();
}

// ── member_ban ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ban_succeeds_for_admin() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Admin],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    let result: Value = client.request("member_ban", p).await.expect("rpc call");
    assert_eq!(result, Value::Bool(true));

    // Member should be removed and ban entry added.
    let members = node.state.members.read().await;
    assert!(!members[&community_id].contains_key(&target_id));
    drop(members);
    let bans = node.state.bans.read().await;
    assert!(
        bans.get(&community_id)
            .map(|list| list.contains(&target_id))
            .unwrap_or(false)
    );
    handle.stop();
}

#[tokio::test]
async fn ban_forbidden_for_moderator() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Moderator],
    )
    .await;
    let (target_id, target_vk) = make_member();
    seed_member(
        &node.state,
        &community_id,
        &target_id,
        target_vk,
        vec![Role::Member],
    )
    .await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("user_id", &target_id).unwrap();
    let err = client
        .request::<Value, _>("member_ban", p)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("admin required"),
        "unexpected error: {err}"
    );
    handle.stop();
}

// ── Announcement channel: message_send ───────────────────────────────────────

#[tokio::test]
async fn announce_post_forbidden_for_plain_member() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Member],
    )
    .await;
    let channel_id = seed_announcement_channel(&node.state, &community_id).await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("channel_id", &channel_id).unwrap();
    p.insert("body", "hello").unwrap();
    let err = client
        .request::<Value, _>("message_send", p)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("admins and moderators"),
        "unexpected error: {err}"
    );
    handle.stop();
}

#[tokio::test]
async fn announce_post_succeeds_for_admin() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Admin],
    )
    .await;
    let channel_id = seed_announcement_channel(&node.state, &community_id).await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("channel_id", &channel_id).unwrap();
    p.insert("body", "hello from admin").unwrap();
    let result: Value = client.request("message_send", p).await.expect("rpc call");
    assert!(result.get("id").is_some(), "expected MessageInfo response");
    handle.stop();
}

#[tokio::test]
async fn announce_post_succeeds_for_moderator() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let community_id = seed_community(&node.state, &node.signing_key).await;
    seed_member(
        &node.state,
        &community_id,
        &node.peer_id,
        node.vk_bytes,
        vec![Role::Moderator],
    )
    .await;
    let channel_id = seed_announcement_channel(&node.state, &community_id).await;

    let (handle, client) = start(Arc::clone(&node.state)).await;
    let mut p = ObjectParams::new();
    p.insert("community_id", &community_id).unwrap();
    p.insert("channel_id", &channel_id).unwrap();
    p.insert("body", "hello from mod").unwrap();
    let result: Value = client.request("message_send", p).await.expect("rpc call");
    assert!(result.get("id").is_some(), "expected MessageInfo response");
    handle.stop();
}
