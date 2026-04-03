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
