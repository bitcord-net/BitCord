//! API integration test: start `ApiServer` with an in-process stub,
//! connect a JSON-RPC WebSocket client, call `identity_get`, and assert the
//! response shape.

use std::{net::SocketAddr, sync::Arc};

use bitcord_core::{
    api::{ApiServer, AppState},
    config::NodeConfig,
    crypto::channel_keys::ChannelKey,
    identity::{NodeIdentity, SigningKey},
    model::{
        channel::{ChannelKind, ChannelManifest},
        community::CommunityManifest,
        types::{ChannelId, CommunityId, UserId},
    },
    network::NetworkHandle,
    resource::metrics::NodeMetrics,
    state::MessageLog,
};
use jsonrpsee::{
    core::{client::ClientT, params::ObjectParams},
    rpc_params,
    ws_client::WsClientBuilder,
};
use rand::rngs::OsRng;
use serde_json::Value;
use tempfile::TempDir;
use ulid::Ulid;

// ── Test helpers ──────────────────────────────────────────────────────────────

struct TestNode {
    state: Arc<AppState>,
    peer_id: String,
}

/// Build a minimal `AppState` for API-layer tests (no real swarm).
fn make_test_node(tmp: &TempDir) -> TestNode {
    let identity = NodeIdentity::generate();
    let peer_id = identity.to_peer_id().to_string();
    let public_key_hex = identity
        .verifying_key()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let node_address = identity.node_address();

    let signing_key = bitcord_core::identity::SigningKey::from_bytes(&identity.signing_key_bytes());
    let message_log = MessageLog::new();
    let identity_arc = Arc::new(identity);
    let (swarm_cmd_tx, _) = NetworkHandle::spawn(Arc::clone(&identity_arc), vec![], None);
    let metrics = Arc::new(NodeMetrics::default());

    let config = NodeConfig {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let config_path = tmp.path().join("node.toml");

    let state = AppState::new(
        peer_id.clone(),
        public_key_hex,
        node_address,
        signing_key,
        config,
        config_path,
        message_log,
        swarm_cmd_tx,
        metrics,
        None,
        None, // no encryption in tests
        None, // no TLS server in tests
        None, // no DHT in tests
    );

    TestNode {
        state: Arc::new(state),
        peer_id,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn identity_get_returns_valid_shape() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let expected_peer_id = node.peer_id.clone();

    // Set a display name before starting the server.
    node.state.identity_state.write().await.display_name = Some("TestNode".to_string());

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state)
        .await
        .expect("start API server");

    let ws_url = format!("ws://{}", handle.local_addr());
    let client = WsClientBuilder::default()
        .build(&ws_url)
        .await
        .expect("ws connect");

    let result: Value = client
        .request("identity_get", rpc_params!())
        .await
        .expect("identity_get call");

    assert_eq!(
        result.get("peer_id").and_then(|v| v.as_str()),
        Some(expected_peer_id.as_str()),
        "peer_id mismatch"
    );
    assert_eq!(
        result.get("display_name").and_then(|v| v.as_str()),
        Some("TestNode"),
        "display_name mismatch"
    );
    assert!(
        result
            .get("public_key_hex")
            .and_then(|v| v.as_str())
            .is_some(),
        "public_key_hex missing"
    );
    assert!(result.get("status").is_some(), "status field missing");

    handle.stop();
}

#[tokio::test]
async fn node_get_metrics_returns_snapshot() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state)
        .await
        .expect("start API server");

    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let result: Value = client
        .request("node_get_metrics", rpc_params!())
        .await
        .expect("node_get_metrics call");

    assert!(result.get("connected_peers").is_some());
    assert!(result.get("disk_usage_mb").is_some());
    assert!(result.get("uptime_secs").is_some());

    handle.stop();
}

#[tokio::test]
async fn node_get_config_returns_defaults() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state)
        .await
        .expect("start API server");

    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let result: Value = client
        .request("node_get_config", rpc_params!())
        .await
        .expect("node_get_config call");

    assert_eq!(
        result.get("max_connections").and_then(|v| v.as_u64()),
        Some(50),
        "default max_connections should be 50"
    );
    assert_eq!(
        result.get("storage_limit_mb").and_then(|v| v.as_u64()),
        Some(512),
        "default storage_limit_mb should be 512"
    );

    handle.stop();
}

#[tokio::test]
async fn node_set_config_updates_field() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state)
        .await
        .expect("start API server");

    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let mut set_params = ObjectParams::new();
    set_params.insert("max_connections", 25).unwrap();
    let ok: Value = client
        .request("node_set_config", set_params)
        .await
        .expect("node_set_config call");
    assert_eq!(ok, Value::Bool(true));

    let cfg: Value = client
        .request("node_get_config", rpc_params!())
        .await
        .expect("node_get_config call");
    assert_eq!(
        cfg.get("max_connections").and_then(|v| v.as_u64()),
        Some(25)
    );

    handle.stop();
}

// ── Message delete helpers ────────────────────────────────────────────────────

struct ChannelSetup {
    community_id: String,
    channel_id: String,
}

/// Seed `state` with a community (no seed nodes, so `require_seed_connected`
/// passes) and a single Text channel with a freshly generated channel key.
async fn setup_channel(state: &Arc<AppState>) -> ChannelSetup {
    let admin_key = SigningKey::generate(&mut OsRng);
    let vk = admin_key.verifying_key();
    let admin_id = UserId::from_verifying_key(&vk);
    let community_id = CommunityId::new();
    let channel_id = ChannelId::new();

    let manifest = CommunityManifest {
        id: community_id.clone(),
        name: "Test".into(),
        description: String::new(),
        public_key: vk.to_bytes(),
        created_at: chrono::Utc::now(),
        admin_ids: vec![admin_id],
        channel_ids: vec![channel_id.clone()],
        seed_nodes: vec![], // empty → bypasses require_seed_connected
        version: 1,
        deleted: false,
    };
    let signed = manifest.sign(&admin_key);

    let channel_manifest = ChannelManifest {
        id: channel_id.clone(),
        community_id: community_id.clone(),
        name: "general".into(),
        kind: ChannelKind::Text,
        encrypted_channel_key: std::collections::HashMap::new(),
        created_at: chrono::Utc::now(),
        version: 1,
    };

    let key_bytes = *ChannelKey::generate().as_bytes();
    let cid = community_id.to_string();
    let chid = channel_id.to_string();

    state.communities.write().await.insert(cid.clone(), signed);
    state
        .channels
        .write()
        .await
        .insert(chid.clone(), channel_manifest);
    state
        .channel_keys
        .write()
        .await
        .insert(chid.clone(), key_bytes);

    ChannelSetup {
        community_id: cid,
        channel_id: chid,
    }
}

// ── Message delete tests ──────────────────────────────────────────────────────

/// Sending then deleting a message should tombstone it: `get_history` returns
/// the same message ID with `deleted: true` and an empty body.
#[tokio::test]
async fn message_delete_tombstones_own_message() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let setup = setup_channel(&node.state).await;

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state)
        .await
        .expect("start API server");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    // 1. Send a message.
    let mut send_p = ObjectParams::new();
    send_p
        .insert("community_id", setup.community_id.clone())
        .unwrap();
    send_p
        .insert("channel_id", setup.channel_id.clone())
        .unwrap();
    send_p.insert("body", "hello world").unwrap();
    send_p.insert("reply_to", serde_json::Value::Null).unwrap();
    let sent: Value = client
        .request("message_send", send_p)
        .await
        .expect("message_send");
    let msg_id = sent["id"].as_str().expect("id field").to_string();
    assert!(!msg_id.is_empty(), "message_send must return an id");

    // 2. Delete it.
    let mut del_p = ObjectParams::new();
    del_p
        .insert("community_id", setup.community_id.clone())
        .unwrap();
    del_p
        .insert("channel_id", setup.channel_id.clone())
        .unwrap();
    del_p.insert("message_id", msg_id.clone()).unwrap();
    let ok: Value = client
        .request("message_delete", del_p)
        .await
        .expect("message_delete");
    assert_eq!(ok, Value::Bool(true), "message_delete must return true");

    // 3. History must show the entry as tombstoned.
    let mut hist_p = ObjectParams::new();
    hist_p
        .insert("community_id", setup.community_id.clone())
        .unwrap();
    hist_p
        .insert("channel_id", setup.channel_id.clone())
        .unwrap();
    let history: Value = client
        .request("message_get_history", hist_p)
        .await
        .expect("message_get_history");
    let messages = history.as_array().expect("history must be an array");
    let found = messages
        .iter()
        .find(|m| m["id"] == msg_id)
        .expect("deleted message must still appear in history");
    assert_eq!(
        found["deleted"].as_bool(),
        Some(true),
        "deleted field must be true after tombstone"
    );

    handle.stop();
}

/// Attempting to delete a message authored by a different peer must be rejected
/// with a "forbidden" error (-32003).
#[tokio::test]
async fn message_delete_rejects_non_author() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let setup = setup_channel(&node.state).await;

    // Inject a message that belongs to a different author.
    let other_author = "ab".repeat(32); // 64-char hex ≠ node's peer_id
    let foreign_msg_id = Ulid::new().to_string();
    {
        let mut log = node.state.message_log.lock().await;
        log.append(
            &setup.channel_id,
            foreign_msg_id.clone(),
            other_author,
            chrono::Utc::now().timestamp_millis(),
            [0u8; 24],
            vec![],
        );
    }

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state)
        .await
        .expect("start API server");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let mut del_p = ObjectParams::new();
    del_p
        .insert("community_id", setup.community_id.clone())
        .unwrap();
    del_p
        .insert("channel_id", setup.channel_id.clone())
        .unwrap();
    del_p.insert("message_id", foreign_msg_id).unwrap();
    let result: Result<Value, _> = client.request("message_delete", del_p).await;
    assert!(result.is_err(), "deleting another user's message must fail");
    // jsonrpsee wraps the server error; the message should contain our text.
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("only the author"),
        "expected authorship error, got: {err_msg}"
    );

    handle.stop();
}

// ── Invite security tests ─────────────────────────────────────────────────────

/// Base64url-encode a JSON value as an invite link payload.
fn invite_b64(payload: serde_json::Value) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD.encode(serde_json::to_string(&payload).unwrap().as_bytes())
}

/// `community_join` must reject an invite that has no `sig_hex` field.
#[tokio::test]
async fn community_join_rejects_unsigned_invite() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let admin_key = SigningKey::generate(&mut OsRng);
    let pk_hex: String = admin_key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let community_id = Ulid::new().to_string();

    let invite = invite_b64(serde_json::json!({
        "community_id": community_id,
        "name": "Test",
        "description": "",
        "seed_nodes": [],
        "public_key_hex": pk_hex
        // sig_hex intentionally absent
    }));

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let mut p = ObjectParams::new();
    p.insert("invite", invite).unwrap();
    let result: Result<Value, _> = client.request("community_join", p).await;
    assert!(result.is_err(), "unsigned invite must be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("admin signature required"),
        "expected 'admin signature required', got: {err}"
    );

    handle.stop();
}

/// `community_join` must reject an invite whose `sig_hex` does not verify
/// against the embedded public key.
#[tokio::test]
async fn community_join_rejects_bad_signature() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let admin_key = SigningKey::generate(&mut OsRng);
    let pk_hex: String = admin_key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let community_id = Ulid::new().to_string();

    // "ab" repeated 64 times is 128 hex chars but not a valid signature for this key.
    let bad_sig = "ab".repeat(64);
    let invite = invite_b64(serde_json::json!({
        "community_id": community_id,
        "name": "Test",
        "description": "",
        "seed_nodes": [],
        "public_key_hex": pk_hex,
        "sig_hex": bad_sig
    }));

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let mut p = ObjectParams::new();
    p.insert("invite", invite).unwrap();
    let result: Result<Value, _> = client.request("community_join", p).await;
    assert!(result.is_err(), "bad-signature invite must be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("signature verification failed"),
        "expected 'signature verification failed', got: {err}"
    );

    handle.stop();
}

/// A non-admin node must be forbidden from calling `community_generate_invite`.
/// `setup_channel` seeds a community whose admin is a *different* key, so the
/// test node is not an admin.
#[tokio::test]
async fn community_generate_invite_non_admin_forbidden() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);
    let setup = setup_channel(&node.state).await;

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let result: Result<Value, _> = client
        .request("community_generate_invite", rpc_params!(setup.community_id))
        .await;
    assert!(result.is_err(), "non-admin must not generate invite");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("only community admins"),
        "expected admin-only error, got: {err}"
    );

    handle.stop();
}

/// An admin node calling `community_generate_invite` must receive a
/// `bitcord://join/…` link whose decoded payload includes both `sig_hex`
/// (128-char Ed25519 signature) and `cert_fingerprint_hex` (64-char SHA-256).
#[tokio::test]
async fn community_generate_invite_returns_signed_link() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let vk = node.state.signing_key.verifying_key();
    let admin_id = UserId::from_verifying_key(&vk);
    let community_id = CommunityId::new();
    let manifest = CommunityManifest {
        id: community_id.clone(),
        name: "Invite Test".into(),
        description: String::new(),
        public_key: vk.to_bytes(),
        created_at: chrono::Utc::now(),
        admin_ids: vec![admin_id],
        channel_ids: vec![],
        seed_nodes: vec![],
        version: 1,
        deleted: false,
    };
    node.state.communities.write().await.insert(
        community_id.to_string(),
        manifest.sign(&*node.state.signing_key),
    );

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let link: Value = client
        .request(
            "community_generate_invite",
            rpc_params!(community_id.to_string()),
        )
        .await
        .expect("community_generate_invite");
    let link_str = link.as_str().expect("result must be a string");
    assert!(
        link_str.starts_with("bitcord://join/"),
        "link must start with bitcord://join/"
    );

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let b64_part = link_str.trim_start_matches("bitcord://join/");
    let decoded = URL_SAFE_NO_PAD.decode(b64_part).expect("base64 decode");
    let payload: Value = serde_json::from_slice(&decoded).expect("json parse");

    let sig_hex = payload
        .get("sig_hex")
        .and_then(|v| v.as_str())
        .expect("sig_hex must be present");
    assert_eq!(sig_hex.len(), 128, "sig_hex must be 128 hex chars");

    let fp_hex = payload
        .get("cert_fingerprint_hex")
        .and_then(|v| v.as_str())
        .expect("cert_fingerprint_hex must be present");
    assert_eq!(
        fp_hex.len(),
        64,
        "cert_fingerprint_hex must be 64 hex chars"
    );

    handle.stop();
}

// ── DM RPC tests ──────────────────────────────────────────────────────────────

/// A fake peer ID: 64 hex chars that look like a real SHA-256 digest but belong
/// to no actual node in the test environment.
const FAKE_PEER_ID: &str = "aabbccdd00112233aabbccdd00112233aabbccdd00112233aabbccdd00112233";

/// `dm_send` must persist the message and return a `DmMessageInfo` with the
/// expected fields even when the recipient is unknown (no route is found).
#[tokio::test]
async fn dm_send_stores_message_and_returns_info() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let mut p = ObjectParams::new();
    p.insert("peer_id", FAKE_PEER_ID).unwrap();
    p.insert("body", "hello stranger").unwrap();
    p.insert("reply_to", serde_json::Value::Null).unwrap();

    let result: Value = client
        .request("dm_send", p)
        .await
        .expect("dm_send should succeed even without a delivery route");

    assert_eq!(
        result.get("peer_id").and_then(|v| v.as_str()),
        Some(FAKE_PEER_ID),
        "peer_id must match"
    );
    assert_eq!(
        result.get("body").and_then(|v| v.as_str()),
        Some("hello stranger"),
        "body must be preserved"
    );
    assert!(
        result.get("id").and_then(|v| v.as_str()).is_some(),
        "id must be present"
    );
    assert!(
        result.get("timestamp").is_some(),
        "timestamp must be present"
    );

    handle.stop();
}

/// `dm_get_history` returns the messages previously sent via `dm_send`.
#[tokio::test]
async fn dm_get_history_returns_sent_messages() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    // Send two messages to the same fake peer.
    for body in &["first message", "second message"] {
        let mut p = ObjectParams::new();
        p.insert("peer_id", FAKE_PEER_ID).unwrap();
        p.insert("body", *body).unwrap();
        p.insert("reply_to", serde_json::Value::Null).unwrap();
        let _: Value = client.request("dm_send", p).await.expect("dm_send");
    }

    // Retrieve history.
    let mut hist_p = ObjectParams::new();
    hist_p.insert("peer_id", FAKE_PEER_ID).unwrap();
    let history: Value = client
        .request("dm_get_history", hist_p)
        .await
        .expect("dm_get_history");

    let messages = history.as_array().expect("history must be an array");
    assert_eq!(messages.len(), 2, "both messages must appear in history");
    assert_eq!(messages[0]["body"].as_str(), Some("first message"));
    assert_eq!(messages[1]["body"].as_str(), Some("second message"));

    handle.stop();
}

/// `dm_get_history` returns an empty array for a peer with whom no messages
/// have been exchanged.
#[tokio::test]
async fn dm_get_history_empty_for_unknown_peer() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let mut p = ObjectParams::new();
    p.insert("peer_id", FAKE_PEER_ID).unwrap();
    let history: Value = client
        .request("dm_get_history", p)
        .await
        .expect("dm_get_history");

    let messages = history.as_array().expect("history must be an array");
    assert!(
        messages.is_empty(),
        "no messages exchanged yet — must return empty array"
    );

    handle.stop();
}

/// `dm_send` must reject an empty body with an invalid-params error.
#[tokio::test]
async fn dm_send_rejects_empty_body() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let mut p = ObjectParams::new();
    p.insert("peer_id", FAKE_PEER_ID).unwrap();
    p.insert("body", "").unwrap();
    p.insert("reply_to", serde_json::Value::Null).unwrap();
    let result: Result<Value, _> = client.request("dm_send", p).await;
    assert!(result.is_err(), "empty body must be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("body must not be empty"),
        "expected 'body must not be empty', got: {err}"
    );

    handle.stop();
}

/// `dm_peer_name` returns `null` (JSON null) for a peer not in the DM cache.
#[tokio::test]
async fn dm_peer_name_returns_null_for_unknown() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    let result: Value = client
        .request("dm_peer_name", rpc_params!(FAKE_PEER_ID))
        .await
        .expect("dm_peer_name should not error");
    assert!(
        result.is_null(),
        "unknown peer must return null, got: {result}"
    );

    handle.stop();
}

/// After `dm_send`, the `dm_get_history` `before` cursor skips messages at and
/// after the specified ID, returning only the earlier batch.
#[tokio::test]
async fn dm_get_history_before_cursor_works() {
    let tmp = TempDir::new().unwrap();
    let node = make_test_node(&tmp);

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let handle = ApiServer::start(addr, node.state).await.expect("start");
    let client = WsClientBuilder::default()
        .build(format!("ws://{}", handle.local_addr()))
        .await
        .expect("ws connect");

    // Send three messages and capture the ID of the second.
    let mut second_id = String::new();
    for (i, body) in ["msg-1", "msg-2", "msg-3"].iter().enumerate() {
        let mut p = ObjectParams::new();
        p.insert("peer_id", FAKE_PEER_ID).unwrap();
        p.insert("body", *body).unwrap();
        p.insert("reply_to", serde_json::Value::Null).unwrap();
        let sent: Value = client.request("dm_send", p).await.expect("dm_send");
        if i == 1 {
            second_id = sent["id"].as_str().unwrap().to_string();
        }
    }

    // Fetch history before the second message — should return only the first.
    let mut p = ObjectParams::new();
    p.insert("peer_id", FAKE_PEER_ID).unwrap();
    p.insert("before", second_id).unwrap();
    let history: Value = client
        .request("dm_get_history", p)
        .await
        .expect("dm_get_history with before");

    let messages = history.as_array().expect("must be an array");
    assert_eq!(messages.len(), 1, "only msg-1 should precede msg-2");
    assert_eq!(messages[0]["body"].as_str(), Some("msg-1"));

    handle.stop();
}
