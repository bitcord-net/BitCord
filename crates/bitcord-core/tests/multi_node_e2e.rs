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
    dht::{DhtConfig, DhtHandle},
    identity::{NodeIdentity, SigningKey},
    network::{NetworkCommand, NodeAddr, client::NodeClient, tls::NodeTlsCert},
    node::{server::NodeServer, store::NodeStore},
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
        let store = Arc::new(NodeStore::open(&db_path, None).expect("open node store"));
        let limiter = Arc::new(ConnectionLimiter::new(50));

        let node_pk = node_identity.verifying_key().to_bytes();
        let dht = Arc::new(
            DhtHandle::new(DhtConfig {
                node_pk,
                self_addr: None,
                store_path: tmp.path().join("dht.redb"),
                identity: Arc::new(NodeIdentity::from_signing_key_bytes(
                    &node_identity.signing_key_bytes(),
                )),
            })
            .await
            .expect("create test DHT"),
        );
        let (swarm_cmd_tx, _swarm_cmd_rx) = tokio::sync::mpsc::channel::<NetworkCommand>(1);
        let server = Arc::new(
            NodeServer::bind(
                "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                &tls_cert,
                bitcord_core::node::NodeServicesConfig {
                    store,
                    dht: Some(dht),
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

// ── Test 5: Cross-node DHT — store on node B, query from node A ──────────────
//
// Simulates the real peer-discovery flow: node A's client pushes a
// CommunityPeerRecord directly to node B (as AnnounceCommunityPresence does
// via NodeClient::store_community_peer), then a fresh client on node B can
// retrieve it with FindCommunityPeers.
//
// This exercises the full network path:
//   client → QUIC → node B handler → Dht::announce_community_peer
//   client → QUIC → node B handler → Dht::lookup_community_peers → response

#[tokio::test]
async fn cross_node_dht_store_and_retrieve() {
    let node_a = TestNode::spawn().await;
    let node_b = TestNode::spawn().await;

    let identity = Arc::new(NodeIdentity::generate());

    // Two clients, each connected to a different node.
    let (client_on_a, _, _) = NodeClient::connect(
        node_a.node_addr(),
        node_a.fingerprint(),
        Arc::clone(&identity),
    )
    .await
    .expect("connect to node A");

    let (client_on_b, _, _) = NodeClient::connect(
        node_b.node_addr(),
        node_b.fingerprint(),
        Arc::clone(&identity),
    )
    .await
    .expect("connect to node B");

    let community_pk = [0x77u8; 32];
    // Announce node A's address into node B's DHT.
    let node_a_pk = node_a.node_identity.verifying_key().to_bytes();
    client_on_b
        .store_community_peer(community_pk, node_a_pk, node_a.node_addr())
        .await
        .expect("store node A's record on node B");

    // A second client (representing a third peer) also pushes its record to node B.
    let other_pk = [0x88u8; 32];
    let other_addr = NodeAddr::new("127.0.0.1".parse().unwrap(), 19042);
    client_on_b
        .store_community_peer(community_pk, other_pk, other_addr.clone())
        .await
        .expect("store other peer record on node B");

    // Query node B — client_on_b is the right handle (connected to node B).
    let records = client_on_b
        .find_community_peers(community_pk)
        .await
        .expect("find_community_peers on node B");

    assert_eq!(records.len(), 2, "node B should have both peer records");

    let node_pks: Vec<[u8; 32]> = records.iter().map(|r| r.node_pk).collect();
    assert!(node_pks.contains(&node_a_pk), "node A's record missing");
    assert!(node_pks.contains(&other_pk), "other peer's record missing");

    // Node A's DHT is independent — it has no records for this community.
    let records_on_a = client_on_a
        .find_community_peers(community_pk)
        .await
        .expect("find_community_peers on node A");
    assert!(
        records_on_a.is_empty(),
        "node A should have no records (DHTs are independent)"
    );

    node_a.server.close();
    node_b.server.close();
}

// ── Test 7: Seed-routed DHT peer discovery ────────────────────────────────────
//
// Topology:
//   Node A ──auth──▶ Node S (seed)
//   Node B ──auth──▶ Node S (seed)
//
// Steps:
//   1. Node A creates a community (no explicit seed for the community).
//   2. Node B joins the same community.
//   3. Both nodes announce their presence to Node S via StoreCommunityPeer.
//   4. Node A issues FIND_NODE for Node B's pk to Node S — Node B should appear
//      in the routing-table response because Node B authenticated to Node S.
//   5. Node A queries Node S with FindCommunityPeers — should include Node B.
//   6. Node B queries Node S with FindCommunityPeers — should include Node A.

#[tokio::test]
async fn seed_routed_dht_peer_discovery() {
    let node_s = TestNode::spawn().await;
    let node_a = TestNode::spawn().await;
    let node_b = TestNode::spawn().await;

    // Community admin issues hosting certs for both Node A and Node B.
    let community_identity = NodeIdentity::generate();
    let community_sk = SigningKey::from_bytes(&community_identity.signing_key_bytes());
    let cert_a = node_a.issue_cert(&community_sk);
    let cert_b = node_b.issue_cert(&community_sk);
    let community_pk = cert_a.community_pk;

    // Node A creates the community (no seed).
    let node_a_identity = Arc::new(NodeIdentity::from_signing_key_bytes(
        &node_a.node_identity.signing_key_bytes(),
    ));
    let (client_a, _, _) = NodeClient::connect(
        node_a.node_addr(),
        node_a.fingerprint(),
        Arc::clone(&node_a_identity),
    )
    .await
    .expect("connect to Node A");
    client_a
        .join_community(cert_a, None, None)
        .await
        .expect("Node A creates community");

    // Node B joins the community.
    let node_b_identity = Arc::new(NodeIdentity::from_signing_key_bytes(
        &node_b.node_identity.signing_key_bytes(),
    ));
    let (client_b, _, _) = NodeClient::connect(
        node_b.node_addr(),
        node_b.fingerprint(),
        Arc::clone(&node_b_identity),
    )
    .await
    .expect("connect to Node B");
    client_b
        .join_community(cert_b, None, None)
        .await
        .expect("Node B joins community");

    // Both nodes connect to Node S using their own identities, causing Node S
    // to add each of them to its Kademlia routing table on Authenticate.
    let (client_a_on_s, _, _) = NodeClient::connect(
        node_s.node_addr(),
        node_s.fingerprint(),
        Arc::clone(&node_a_identity),
    )
    .await
    .expect("Node A connects to seed");

    let (client_b_on_s, _, _) = NodeClient::connect(
        node_s.node_addr(),
        node_s.fingerprint(),
        Arc::clone(&node_b_identity),
    )
    .await
    .expect("Node B connects to seed");

    // Both nodes announce their community presence to Node S.
    let node_a_pk = node_a.node_identity.verifying_key().to_bytes();
    let node_b_pk = node_b.node_identity.verifying_key().to_bytes();

    client_a_on_s
        .store_community_peer(community_pk, node_a_pk, node_a.node_addr())
        .await
        .expect("Node A announces community presence to seed");
    client_b_on_s
        .store_community_peer(community_pk, node_b_pk, node_b.node_addr())
        .await
        .expect("Node B announces community presence to seed");

    // Step 4: FIND_NODE — Node S's routing table should include Node B because
    // Node B authenticated to Node S.
    let (closest_to_b, _) = client_a_on_s
        .find_node(node_b_pk)
        .await
        .expect("FIND_NODE for Node B on seed");
    let routing_pks: Vec<[u8; 32]> = closest_to_b.iter().map(|(pk, _)| *pk).collect();
    assert!(
        routing_pks.contains(&node_b_pk),
        "Node S's routing table should include Node B after it authenticated"
    );

    // Step 5: Node A discovers Node B via seed's community peer records.
    let peers_for_a = client_a_on_s
        .find_community_peers(community_pk)
        .await
        .expect("Node A queries seed for community peers");
    let pks_for_a: Vec<[u8; 32]> = peers_for_a.iter().map(|r| r.node_pk).collect();
    assert!(
        pks_for_a.contains(&node_b_pk),
        "Node A should discover Node B via seed"
    );

    // Step 6: Node B discovers Node A via seed's community peer records.
    let peers_for_b = client_b_on_s
        .find_community_peers(community_pk)
        .await
        .expect("Node B queries seed for community peers");
    let pks_for_b: Vec<[u8; 32]> = peers_for_b.iter().map(|r| r.node_pk).collect();
    assert!(
        pks_for_b.contains(&node_a_pk),
        "Node B should discover Node A via seed"
    );

    node_s.server.close();
    node_a.server.close();
    node_b.server.close();
}

// ── Test 6: Kademlia FIND_NODE response includes community peer's address ──────
//
// Node A connects to node B using node A's own identity, which causes node B to
// add node A to its routing table on Authenticate.  A subsequent FIND_NODE
// query to node B should return node A as one of the closest peers, which is
// the exact mechanism that kademlia_find_community_peers phase-1 relies on.

#[tokio::test]
async fn find_node_returns_peers_known_to_remote_node() {
    let node_a = TestNode::spawn().await;
    let node_b = TestNode::spawn().await;

    // Connect to node B *as node A's identity* so that node B's routing table
    // gets node A's pk added on successful authentication.
    let node_a_identity = Arc::new(NodeIdentity::from_signing_key_bytes(
        &node_a.node_identity.signing_key_bytes(),
    ));
    let (client_a_on_b, _, _) =
        NodeClient::connect(node_b.node_addr(), node_b.fingerprint(), node_a_identity)
            .await
            .expect("connect to node B as node A");

    let node_a_pk = node_a.node_identity.verifying_key().to_bytes();
    let community_pk = [0x99u8; 32];

    // Ask node B for nodes closest to community_pk. Node B's routing table now
    // contains node A (authenticated above), so it should appear in the response.
    let (closest_peers, _mailbox) = client_a_on_b
        .find_node(community_pk)
        .await
        .expect("find_node on node B");

    let returned_pks: Vec<[u8; 32]> = closest_peers.iter().map(|(pk, _)| *pk).collect();
    assert!(
        returned_pks.contains(&node_a_pk),
        "FIND_NODE should include node A (the only authenticated peer in node B's routing table)"
    );

    node_a.server.close();
    node_b.server.close();
}
