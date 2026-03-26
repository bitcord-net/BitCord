//! Multi-node end-to-end tests.
//!
//! Two independent `NodeServer` instances run on loopback. Each has its own
//! `NodeStore` and in-memory `Dht`. Clients connect to their respective nodes
//! and exercise the full authenticated flow.
//!
//! Note: inter-node message sync (DHT-based routing of messages between nodes)
//! is scheduled for a future phase. These tests verify the single-node
//! authenticated flow in a realistic two-node topology and confirm that each
//! node's state is fully independent.

use std::{net::SocketAddr, sync::Arc};

use bitcord_core::{
    crypto::{certificate::HostingCert, channel_keys::ChannelKey},
    identity::{NodeIdentity, SigningKey},
    network::{NetworkCommand, NodeAddr, client::NodeClient, tls::NodeTlsCert},
    node::{dht::Dht, server::NodeServer, store::NodeStore},
    resource::connection_limiter::ConnectionLimiter,
};
use tempfile::TempDir;
use ulid::Ulid;

// ── Shared helper ─────────────────────────────────────────────────────────────

struct TestNode {
    server: Arc<NodeServer>,
    tls_cert: NodeTlsCert,
    node_identity: NodeIdentity,
    _tmp: TempDir, // keep alive for the lifetime of the node
}

impl TestNode {
    async fn spawn() -> Self {
        let tmp = TempDir::new().unwrap();
        let node_identity = NodeIdentity::generate();
        let sk = SigningKey::from_bytes(&node_identity.signing_key_bytes());
        let tls_cert = NodeTlsCert::generate(&sk).expect("generate TLS cert");

        let db_path = tmp.path().join("node.redb");
        let store = Arc::new(NodeStore::open(&db_path).expect("open node store"));
        let dht = Arc::new(Dht::new(node_identity.verifying_key().to_bytes(), None));
        let limiter = Arc::new(ConnectionLimiter::new(50));

        let node_pk = node_identity.verifying_key().to_bytes();
        let (swarm_cmd_tx, _swarm_cmd_rx) = tokio::sync::mpsc::channel::<NetworkCommand>(1);
        let server = Arc::new(
            NodeServer::bind(
                "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                &tls_cert,
                bitcord_core::node::NodeServicesConfig {
                    store,
                    dht,
                    limiter,
                    node_pk,
                    swarm_cmd_tx,
                    join_password: None,
                },
            )
            .await
            .expect("bind NodeServer"),
        );

        let serve_arc = Arc::clone(&server);
        tokio::spawn(async move { serve_arc.serve().await });

        Self {
            server,
            tls_cert,
            node_identity,
            _tmp: tmp,
        }
    }

    fn node_addr(&self) -> NodeAddr {
        let sa = self.server.local_addr();
        NodeAddr::new(sa.ip(), sa.port())
    }

    fn fingerprint(&self) -> [u8; 32] {
        self.tls_cert.fingerprint
    }

    /// Issue a HostingCert from `community_sk` that authorises *this* node.
    fn issue_cert(&self, community_sk: &SigningKey) -> HostingCert {
        let node_pk = self.node_identity.verifying_key().to_bytes();
        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 86_400 * 365;
        HostingCert::new(community_sk, node_pk, expires_at)
    }
}

// ── Test 1: Two nodes, each serving a client independently ───────────────────

#[tokio::test]
async fn two_nodes_independent_stores() {
    let node_a = TestNode::spawn().await;
    let node_b = TestNode::spawn().await;

    // A shared community admin issues certs for both nodes.
    let community_identity = NodeIdentity::generate();
    let community_sk = SigningKey::from_bytes(&community_identity.signing_key_bytes());

    let cert_a = node_a.issue_cert(&community_sk);
    let cert_b = node_b.issue_cert(&community_sk);

    let client_identity_a = Arc::new(NodeIdentity::generate());
    let client_identity_b = Arc::new(NodeIdentity::generate());

    let community_pk = cert_a.community_pk;

    let (client_a, _, _push_a) = NodeClient::connect(
        node_a.node_addr(),
        node_a.fingerprint(),
        Arc::clone(&client_identity_a),
    )
    .await
    .expect("connect to node A");

    let (client_b, _, _push_b) = NodeClient::connect(
        node_b.node_addr(),
        node_b.fingerprint(),
        Arc::clone(&client_identity_b),
    )
    .await
    .expect("connect to node B");

    client_a
        .join_community(cert_a, None, None)
        .await
        .expect("join node A");
    client_b
        .join_community(cert_b, None, None)
        .await
        .expect("join node B");

    // Send a message on Node A.
    let channel_key = ChannelKey::generate();
    let channel_id = Ulid::new();
    let (nonce_a, ct_a) = channel_key.encrypt_message(b"hello from A").unwrap();
    let seq_a = client_a
        .send_message(community_pk, channel_id, nonce_a, ct_a.clone())
        .await
        .expect("send to node A");

    // Send a different message on Node B.
    let (nonce_b, ct_b) = channel_key.encrypt_message(b"hello from B").unwrap();
    let seq_b = client_b
        .send_message(community_pk, channel_id, nonce_b, ct_b.clone())
        .await
        .expect("send to node B");

    // Each node starts its own seq counter from 0.
    assert_eq!(seq_a, 0);
    assert_eq!(seq_b, 0);

    // Node A only has the message sent to it.
    let msgs_a = client_a
        .get_messages(community_pk, channel_id, 0)
        .await
        .unwrap();
    assert_eq!(msgs_a.len(), 1);
    assert_eq!(msgs_a[0].ciphertext, ct_a);

    // Node B only has the message sent to it.
    let msgs_b = client_b
        .get_messages(community_pk, channel_id, 0)
        .await
        .unwrap();
    assert_eq!(msgs_b.len(), 1);
    assert_eq!(msgs_b[0].ciphertext, ct_b);

    node_a.server.close();
    node_b.server.close();
}

// ── Test 2: Same community cert authorises both nodes ────────────────────────

#[tokio::test]
async fn single_community_cert_works_on_both_nodes() {
    let node_a = TestNode::spawn().await;
    let node_b = TestNode::spawn().await;

    let community_identity = NodeIdentity::generate();
    let community_sk = SigningKey::from_bytes(&community_identity.signing_key_bytes());

    // Both nodes are authorised by the same community signing key.
    let cert_for_a = node_a.issue_cert(&community_sk);
    let cert_for_b = node_b.issue_cert(&community_sk);

    let identity = Arc::new(NodeIdentity::generate());

    let (client_a, _, _) = NodeClient::connect(
        node_a.node_addr(),
        node_a.fingerprint(),
        Arc::clone(&identity),
    )
    .await
    .unwrap();
    let (client_b, _, _) = NodeClient::connect(
        node_b.node_addr(),
        node_b.fingerprint(),
        Arc::clone(&identity),
    )
    .await
    .unwrap();

    // Both join operations succeed.
    client_a
        .join_community(cert_for_a, None, None)
        .await
        .expect("join A with community cert");
    client_b
        .join_community(cert_for_b, None, None)
        .await
        .expect("join B with community cert");

    node_a.server.close();
    node_b.server.close();
}

// ── Test 3: Cross-node cert mismatch is rejected ──────────────────────────────

#[tokio::test]
async fn cert_issued_for_node_a_rejected_on_node_b() {
    let node_a = TestNode::spawn().await;
    let node_b = TestNode::spawn().await;

    let community_identity = NodeIdentity::generate();
    let community_sk = SigningKey::from_bytes(&community_identity.signing_key_bytes());

    // Issue a cert specifically for Node A's public key.
    let cert_for_a = node_a.issue_cert(&community_sk);

    let identity = Arc::new(NodeIdentity::generate());

    let (client_b, _, _) = NodeClient::connect(
        node_b.node_addr(),
        node_b.fingerprint(),
        Arc::clone(&identity),
    )
    .await
    .unwrap();

    // The cert was signed for Node A's public key; Node B has a different pk.
    // The server must reject certs whose node_pk does not match its own identity
    // to prevent replay attacks (cert issued for A being used to gain rights on B).
    let result = client_b.join_community(cert_for_a, None, None).await;
    assert!(
        result.is_err(),
        "cert with mismatched node_pk must be rejected"
    );

    node_a.server.close();
    node_b.server.close();
}

// ── Test 4: Concurrent clients on the same node ───────────────────────────────

#[tokio::test]
async fn concurrent_clients_on_same_node() {
    let node = TestNode::spawn().await;

    let community_identity = NodeIdentity::generate();
    let community_sk = SigningKey::from_bytes(&community_identity.signing_key_bytes());

    let channel_id = Ulid::new();
    let channel_key = ChannelKey::generate();
    let cert_a = node.issue_cert(&community_sk);
    let cert_b = node.issue_cert(&community_sk);

    let identity_a = Arc::new(NodeIdentity::generate());
    let identity_b = Arc::new(NodeIdentity::generate());

    let community_pk = cert_a.community_pk;

    let (client_a, _, _) = NodeClient::connect(
        node.node_addr(),
        node.fingerprint(),
        Arc::clone(&identity_a),
    )
    .await
    .unwrap();
    let (client_b, _, _) = NodeClient::connect(
        node.node_addr(),
        node.fingerprint(),
        Arc::clone(&identity_b),
    )
    .await
    .unwrap();

    client_a.join_community(cert_a, None, None).await.unwrap();
    client_b.join_community(cert_b, None, None).await.unwrap();

    // Both clients send messages concurrently.
    let ck_a = ChannelKey::from_bytes(*channel_key.as_bytes());
    let ck_b = ChannelKey::from_bytes(*channel_key.as_bytes());

    let (handle_a, handle_b) = tokio::join!(
        async {
            let (nonce, ct) = ck_a.encrypt_message(b"from A").unwrap();
            client_a
                .send_message(community_pk, channel_id, nonce, ct)
                .await
                .unwrap()
        },
        async {
            let (nonce, ct) = ck_b.encrypt_message(b"from B").unwrap();
            client_b
                .send_message(community_pk, channel_id, nonce, ct)
                .await
                .unwrap()
        },
    );

    // Sequences must be distinct (0 and 1 in some order).
    let mut seqs = [handle_a, handle_b];
    seqs.sort_unstable();
    assert_eq!(seqs, [0, 1]);

    // Both messages are visible to either client.
    let all = client_a
        .get_messages(community_pk, channel_id, 0)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);

    node.server.close();
}
