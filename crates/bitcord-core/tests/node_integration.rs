//! Node integration tests — in-process QUIC node, loopback transport.
//!
//! Each test binds a real `NodeServer` on `127.0.0.1:0`, connects a
//! `NodeClient`, and exercises the full request/response path through the
//! connection handler.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use bitcord_core::{
    crypto::{certificate::HostingCert, channel_keys::ChannelKey, dm::DmEnvelope},
    identity::{NodeIdentity, SigningKey},
    network::{
        NetworkCommand, NodeAddr,
        client::NodeClient,
        protocol::{ClientRequest, NodePush, NodeResponse, decode_payload, encode_frame},
        tls::NodeTlsCert,
    },
    node::{dht::Dht, server::NodeServer, store::NodeStore},
    resource::connection_limiter::ConnectionLimiter,
};
use ed25519_dalek::Signature;
use tempfile::TempDir;
use ulid::Ulid;
use x25519_dalek::PublicKey;

// ── Test helpers ──────────────────────────────────────────────────────────────

struct TestServer {
    server: Arc<NodeServer>,
    tls_cert: NodeTlsCert,
    node_identity: NodeIdentity,
}

impl TestServer {
    async fn spawn(tmp: &TempDir) -> Self {
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
                    store: Arc::clone(&store),
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

        // Drive the accept loop in the background.
        let serve_arc = Arc::clone(&server);
        tokio::spawn(async move { serve_arc.serve().await });

        Self {
            server,
            tls_cert,
            node_identity,
        }
    }

    fn addr(&self) -> SocketAddr {
        self.server.local_addr()
    }

    fn node_addr(&self) -> NodeAddr {
        let sa = self.addr();
        NodeAddr::new(sa.ip(), sa.port())
    }

    fn fingerprint(&self) -> [u8; 32] {
        self.tls_cert.fingerprint
    }
}

/// Connect a fresh client identity and authenticate to the given server.
async fn connect(srv: &TestServer) -> (NodeClient, tokio::sync::mpsc::Receiver<NodePush>) {
    let identity = Arc::new(NodeIdentity::generate());
    let (client, _, push_rx) = NodeClient::connect(srv.node_addr(), srv.fingerprint(), identity)
        .await
        .expect("NodeClient::connect");
    (client, push_rx)
}

/// Build a valid HostingCert signed by a community admin for the given node.
fn hosting_cert(community_sk: &SigningKey, node_identity: &NodeIdentity) -> HostingCert {
    let node_pk = node_identity.verifying_key().to_bytes();
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 86_400 * 365;
    HostingCert::new(community_sk, node_pk, expires_at)
}

// ── Test 1: Authenticate → JoinCommunity → SendMessage → GetMessages ──────────

#[tokio::test]
async fn authenticate_send_get_messages_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;

    let identity = Arc::new(NodeIdentity::generate());
    let (client, _, _push_rx) =
        NodeClient::connect(srv.node_addr(), srv.fingerprint(), Arc::clone(&identity))
            .await
            .expect("connect");

    // Sign a HostingCert as the community admin (also the client here, for simplicity).
    let community_sk = SigningKey::from_bytes(&identity.signing_key_bytes());
    let cert = hosting_cert(&community_sk, &srv.node_identity);
    let community_pk = cert.community_pk;

    client
        .join_community(cert, None, None)
        .await
        .expect("join_community");

    // Encrypt a plaintext message with a fresh channel key.
    let channel_key = ChannelKey::generate();
    let plaintext = b"hello bitcord";
    let (nonce, ciphertext) = channel_key.encrypt_message(plaintext).expect("encrypt");

    let channel_id = Ulid::new();
    let seq = client
        .send_message(community_pk, channel_id, nonce, ciphertext.clone())
        .await
        .expect("send_message");

    assert_eq!(seq, 0, "first message should have seq=0");

    // Retrieve from seq=0.
    let entries = client
        .get_messages(community_pk, channel_id, 0)
        .await
        .expect("get_messages");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 0);
    assert_eq!(entries[0].nonce, nonce);
    assert_eq!(entries[0].ciphertext, ciphertext);

    // Decrypt and verify.
    let recovered = channel_key
        .decrypt_message(&entries[0].nonce, &entries[0].ciphertext)
        .expect("decrypt");
    assert_eq!(recovered, plaintext);

    srv.server.close();
}

// ── Test 2: Multiple messages in sequence ─────────────────────────────────────

#[tokio::test]
async fn send_multiple_messages_seqs_monotonic() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;

    let identity = Arc::new(NodeIdentity::generate());
    let (client, _, _push_rx) =
        NodeClient::connect(srv.node_addr(), srv.fingerprint(), Arc::clone(&identity))
            .await
            .unwrap();

    let community_sk = SigningKey::from_bytes(&identity.signing_key_bytes());
    let cert = hosting_cert(&community_sk, &srv.node_identity);
    let community_pk = cert.community_pk;
    client.join_community(cert, None, None).await.unwrap();

    let channel_key = ChannelKey::generate();
    let channel_id = Ulid::new();

    let mut seqs = Vec::new();
    for i in 0u8..5 {
        let (nonce, ciphertext) = channel_key.encrypt_message(&[i]).expect("encrypt");
        let seq = client
            .send_message(community_pk, channel_id, nonce, ciphertext)
            .await
            .unwrap();
        seqs.push(seq);
    }

    // Sequences must be strictly increasing.
    for w in seqs.windows(2) {
        assert!(w[1] > w[0], "seqs must be monotonically increasing");
    }

    // since_seq=2 should return seqs 2,3,4.
    let entries = client
        .get_messages(community_pk, channel_id, 2)
        .await
        .unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].seq, 2);

    srv.server.close();
}

// ── Test 3: Reject invalid HostingCert ────────────────────────────────────────

#[tokio::test]
async fn reject_invalid_hosting_cert() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;
    let (client, _push_rx) = connect(&srv).await;

    // Sign a cert with a wrong community key (not the actual community).
    let wrong_community_sk = SigningKey::from_bytes(&NodeIdentity::generate().signing_key_bytes());
    let node_pk = srv.node_identity.verifying_key().to_bytes();
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    // Cert signed by wrong_community_sk — will fail verification because the
    // community_pk in the cert won't match the wrong key when the server
    // re-derives VerifyingKey from it.
    //
    // Actually the cert carries its own community_pk. The server verifies
    // cert.verify(&community_vk) where community_vk is derived from cert.community_pk.
    // So a self-consistent cert from any community key will pass unless we tamper it.
    // To get a rejection we need to tamper the cert after signing.
    let mut cert = HostingCert::new(&wrong_community_sk, node_pk, expires_at);
    // Flip a byte in the signature to break it.
    let mut sig_bytes = cert.signature.to_bytes();
    sig_bytes[0] ^= 0xFF;
    cert.signature = Signature::from_bytes(&sig_bytes);

    let result = client.join_community(cert, None, None).await;
    assert!(result.is_err(), "tampered cert should be rejected");

    srv.server.close();
}

// ── Test 4: Expired HostingCert is rejected ───────────────────────────────────

#[tokio::test]
async fn reject_expired_hosting_cert() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;
    let (client, _push_rx) = connect(&srv).await;

    let community_sk = SigningKey::from_bytes(&NodeIdentity::generate().signing_key_bytes());
    let node_pk = srv.node_identity.verifying_key().to_bytes();
    // expires_at = 1 — always in the past.
    let cert = HostingCert::new(&community_sk, node_pk, 1);

    let result = client.join_community(cert, None, None).await;
    assert!(result.is_err(), "expired cert should be rejected");

    srv.server.close();
}

// ── Test 5: Mailbox deposit and retrieval ─────────────────────────────────────

#[tokio::test]
async fn mailbox_deposit_and_retrieval() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;

    // Sender and recipient each get their own client/identity.
    let sender_identity = Arc::new(NodeIdentity::generate());
    let recipient_identity = Arc::new(NodeIdentity::generate());

    let (sender, _, _) = NodeClient::connect(
        srv.node_addr(),
        srv.fingerprint(),
        Arc::clone(&sender_identity),
    )
    .await
    .unwrap();
    let (recipient, _, _) = NodeClient::connect(
        srv.node_addr(),
        srv.fingerprint(),
        Arc::clone(&recipient_identity),
    )
    .await
    .unwrap();

    let recipient_pk = recipient_identity.verifying_key().to_bytes();
    let sender_x25519_sk = sender_identity.x25519_secret();
    let recipient_x25519_pk = PublicKey::from(&recipient_identity.x25519_secret());

    // Seal a DM.
    let plaintext = b"secret direct message";
    let envelope =
        DmEnvelope::seal(&sender_x25519_sk, &recipient_x25519_pk, plaintext).expect("seal DM");

    // Sender deposits the DM in the recipient's mailbox.
    let seq = sender
        .send_dm(recipient_pk, envelope.clone())
        .await
        .expect("send_dm");
    assert_eq!(seq, 0, "first DM should be seq=0");

    // Recipient retrieves from mailbox.
    let dms = recipient.get_dms(0).await.expect("get_dms");
    assert_eq!(dms.len(), 1, "should have one DM");
    assert_eq!(dms[0].seq, 0);

    // Recipient decrypts.
    let recipient_x25519_sk = recipient_identity.x25519_secret();
    // Rebuild the envelope from the stored ciphertext.
    let retrieved_envelope: DmEnvelope =
        postcard::from_bytes(&dms[0].ciphertext).expect("decode DmEnvelope from mailbox entry");
    let recovered = retrieved_envelope
        .open(&recipient_x25519_sk)
        .expect("open DM");
    assert_eq!(recovered, plaintext);

    srv.server.close();
}

// ── Test 6: Unauthenticated requests are rejected ─────────────────────────────

#[tokio::test]
async fn unauthenticated_send_message_rejected() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;

    // Manually open a QUIC connection and send a SendMessage without authenticating.
    use bitcord_core::network::tls::client_config_pinned;

    let client_cfg = client_config_pinned(srv.fingerprint()).expect("client config");
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse::<SocketAddr>().unwrap())
        .expect("client endpoint");
    endpoint.set_default_client_config(client_cfg);

    let conn = endpoint
        .connect(srv.addr(), "bitcord-node")
        .unwrap()
        .await
        .expect("connect");

    let (mut send, mut recv) = conn.open_bi().await.expect("open_bi");

    let channel_id = Ulid::new();
    let req = ClientRequest::SendMessage {
        community_pk: [0u8; 32],
        channel_id,
        nonce: [0u8; 24],
        ciphertext: b"test".to_vec(),
    };
    let frame = encode_frame(&req).unwrap();
    send.write_all(&frame).await.unwrap();
    send.finish().unwrap();

    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload).await.unwrap();

    let resp: NodeResponse = decode_payload(&payload).unwrap();
    match resp {
        NodeResponse::Error { code, .. } => assert_eq!(code, 401),
        other => panic!("expected Error(401), got {other:?}"),
    }

    srv.server.close();
}

// ── Test 7: Wrong cert fingerprint is rejected ────────────────────────────────

#[tokio::test]
async fn wrong_cert_fingerprint_rejected() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;

    // Use a deliberately wrong fingerprint (all 0xAB bytes ≠ the real cert).
    // Note: all-zeros is the TOFU sentinel that skips pinning; use a non-zero
    // value to exercise genuine fingerprint rejection.
    let wrong_fingerprint = [0xABu8; 32];

    let identity = Arc::new(NodeIdentity::generate());
    let result = NodeClient::connect(srv.node_addr(), wrong_fingerprint, identity).await;

    assert!(
        result.is_err(),
        "connection with wrong fingerprint must fail"
    );

    srv.server.close();
}

// ── Test 8: Malformed postcard payload does not panic the node ────────────────

#[tokio::test]
async fn malformed_payload_does_not_panic() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;

    use bitcord_core::network::tls::client_config_pinned;

    let client_cfg = client_config_pinned(srv.fingerprint()).expect("client config");
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse::<SocketAddr>().unwrap())
        .expect("client endpoint");
    endpoint.set_default_client_config(client_cfg);

    let conn = endpoint
        .connect(srv.addr(), "bitcord-node")
        .unwrap()
        .await
        .expect("connect");

    // Send 20 frames with random garbage as the postcard payload.
    for i in 0u8..20 {
        let Ok((mut send, mut recv)) = conn.open_bi().await else {
            break;
        };

        // Payload is 8 bytes of garbage (discriminant out of range, truncated fields, etc.)
        let garbage: Vec<u8> = (0..8).map(|j| i.wrapping_add(j * 17)).collect();
        let mut frame = Vec::with_capacity(4 + garbage.len());
        frame.extend_from_slice(&(garbage.len() as u32).to_be_bytes());
        frame.extend_from_slice(&garbage);

        if send.write_all(&frame).await.is_err() {
            break;
        }
        let _ = send.finish();

        // Server should respond with an error (or ignore), not crash.
        // We allow the stream to close without a well-formed response — the
        // important invariant is that the *server process* keeps running.
        let mut len_buf = [0u8; 4];
        let _ = recv.read_exact(&mut len_buf).await; // may get nothing
    }

    // If the node panicked, the connection would be forcibly closed.
    // Send a well-formed heartbeat — if it succeeds the node is alive.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        srv.server.connection_count(),
        // The raw connection above may or may not still be open after
        // garbage frames — either is fine. What matters is no panic.
        srv.server.connection_count(), // tautology guard; real check via heartbeat below
    );

    // Establish a fresh legitimate client connection to prove the server lives.
    let (client, _) = connect(&srv).await;
    client
        .heartbeat()
        .await
        .expect("node still alive after malformed payloads");

    srv.server.close();
}

// ── Test 9: Push event delivered after SendMessage ────────────────────────────

#[tokio::test]
async fn push_new_message_delivered_to_subscriber() {
    let tmp = TempDir::new().unwrap();
    let srv = TestServer::spawn(&tmp).await;

    let identity = Arc::new(NodeIdentity::generate());
    let (client, _, mut push_rx) =
        NodeClient::connect(srv.node_addr(), srv.fingerprint(), Arc::clone(&identity))
            .await
            .unwrap();

    let community_sk = SigningKey::from_bytes(&identity.signing_key_bytes());
    let cert = hosting_cert(&community_sk, &srv.node_identity);
    let community_pk = cert.community_pk;
    client.join_community(cert, None, None).await.unwrap();

    let channel_key = ChannelKey::generate();
    let channel_id = Ulid::new();
    let (nonce, ciphertext) = channel_key.encrypt_message(b"push test").unwrap();

    client
        .send_message(community_pk, channel_id, nonce, ciphertext)
        .await
        .unwrap();

    // The push should arrive within a short timeout.
    let push = tokio::time::timeout(Duration::from_secs(2), push_rx.recv())
        .await
        .expect("push timed out")
        .expect("push channel closed");

    match push {
        NodePush::NewMessage {
            channel_id: cid,
            entry,
        } => {
            assert_eq!(cid, channel_id);
            assert_eq!(entry.seq, 0);
        }
        other => panic!("unexpected push: {other:?}"),
    }

    srv.server.close();
}
