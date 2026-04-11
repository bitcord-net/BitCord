//! Shared node initialization logic used by both the headless binary and the
//! embedded Tauri node.  Callers supply an already-loaded `NodeIdentity` and
//! caller-specific paths; this module handles every shared startup step.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, atomic::AtomicU64},
    time::Instant,
};

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    api::{AppState, process_swarm_events},
    config::{NodeConfig, NodeMode},
    crypto::encrypted_io::{derive_table_key, load_or_create_salt},
    dht::{DhtConfig, DhtHandle},
    identity::NodeIdentity,
    network::{NetworkCommand, NetworkHandle, NodeAddr, tls::NodeTlsCert},
    node::{NodeServicesConfig, server::NodeServer, store::NodeStore},
    resource::{
        connection_limiter::ConnectionLimiter,
        metrics::{MetricsUpdate, NodeMetrics, spawn_metrics_task},
    },
    state::MessageLog,
};

// ── Input ──────────────────────────────────────────────────────────────────────

/// Parameters supplied by the caller when bootstrapping a node.
///
/// The identity is passed in already-constructed because the two callers load
/// it differently (interactive prompt vs OS keychain / Tauri command).
pub struct NodeInitConfig {
    pub identity: Arc<NodeIdentity>,
    /// Passphrase used to derive the at-rest table encryption key.
    /// `None` or empty string → no encryption.
    pub passphrase: Option<String>,
    pub config: NodeConfig,
    pub config_path: PathBuf,
    /// Password required for new community registrations; `None` = open node.
    pub join_password: Option<String>,
    /// Fall back to an OS-assigned port if the configured port is unavailable.
    /// Enabled for Tauri embedded nodes; disabled for the headless binary.
    pub fallback_to_random_port: bool,
    /// Optional DHT self-address hint.  `None` = Tauri embedded node (doesn't
    /// know its external port at construction time).
    pub dht_self_addr: Option<NodeAddr>,
    /// Path to the `node.redb` database file.  Callers may place it in
    /// different subdirectories (`data_dir/state/` vs `data_dir/`).
    pub store_db_path: PathBuf,
}

// ── Output ─────────────────────────────────────────────────────────────────────

/// Everything the caller needs after node initialization.
pub struct NodeInitResult {
    pub app_state: Arc<AppState>,
    /// Channel for sending `NetworkCommand`s (shutdown, dial, etc.).
    pub cmd_tx: mpsc::Sender<NetworkCommand>,
    /// Channel for sending `MetricsUpdate`s (disk usage, bandwidth, etc.).
    pub metrics_tx: mpsc::Sender<MetricsUpdate>,
    /// Bandwidth-in and bandwidth-out atomics wired into the metrics task.
    /// The node binary uses these to sync BandwidthLimiter stats after init.
    pub bw_in: Arc<AtomicU64>,
    pub bw_out: Arc<AtomicU64>,
    /// Running QUIC server handle; `None` when node is `GossipClient`.
    pub quic_server: Option<Arc<NodeServer>>,
    /// Actual bound QUIC port; `None` when node is `GossipClient`.
    pub quic_port: Option<u16>,
    /// Raw TLS cert fingerprint bytes for `NodeClient::connect` in Tauri.
    pub cert_fingerprint: Option<[u8; 32]>,
    /// Hex fingerprint string for logging in the headless node binary.
    pub cert_fingerprint_hex: Option<String>,
    /// JoinHandle for the QUIC serve loop; `None` when node is `GossipClient`.
    pub quic_task: Option<tokio::task::JoinHandle<()>>,
    /// JoinHandle for the swarm event processor; await for graceful shutdown.
    pub event_proc: tokio::task::JoinHandle<()>,
}

// ── Shared init ────────────────────────────────────────────────────────────────

/// Bootstrap a BitCord node and return all running handles.
///
/// Both `tokio::spawn` and `tauri::async_runtime::spawn` delegate to the same
/// tokio runtime, so this function uses `tokio::spawn` directly for all
/// background tasks.
pub async fn init_node(cfg: NodeInitConfig) -> Result<NodeInitResult> {
    let NodeInitConfig {
        identity,
        passphrase,
        mut config,
        config_path,
        join_password,
        fallback_to_random_port,
        dht_self_addr,
        store_db_path,
    } = cfg;

    let node_mode = config.node_mode.clone();
    let server_enabled = node_mode != NodeMode::GossipClient;

    // ── 1. Table encryption key ────────────────────────────────────────────────
    let encryption_key: Option<[u8; 32]> =
        passphrase.as_deref().filter(|p| !p.is_empty()).map(|p| {
            let salt_path = config.data_dir.join("table.salt");
            let salt = load_or_create_salt(&salt_path).expect("failed to load/create table salt");
            derive_table_key(p, &salt)
        });

    // ── 2. Identity fields ─────────────────────────────────────────────────────
    let peer_id = identity.to_peer_id().to_string();
    let public_key_hex: String = identity
        .verifying_key()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let node_address = identity.node_address();
    let node_pk: [u8; 32] = identity.verifying_key().to_bytes();
    let sk_bytes = identity.signing_key_bytes();
    let signing_key_for_state = SigningKey::from_bytes(&sk_bytes);
    let signing_key_for_tls = SigningKey::from_bytes(&sk_bytes);

    info!(%peer_id, %node_address, "identity loaded");

    // ── 3. Data directory ──────────────────────────────────────────────────────
    std::fs::create_dir_all(&config.data_dir).context("create data dir")?;
    if let Some(parent) = store_db_path.parent() {
        std::fs::create_dir_all(parent).context("create store dir")?;
    }

    // ── 4. Persistent store + message log ──────────────────────────────────────
    let store =
        Arc::new(NodeStore::open(&store_db_path, encryption_key).context("open node store")?);
    let message_log = MessageLog::new();

    // ── 5. DHT handle (Peer and HeadlessSeed modes only) ──────────────────────
    let dht: Option<Arc<DhtHandle>> = if server_enabled {
        let dht_store_path = config.data_dir.join("dht.redb");
        let handle = DhtHandle::new(DhtConfig {
            node_pk,
            self_addr: dht_self_addr,
            store_path: dht_store_path,
            identity: Arc::clone(&identity),
        })
        .await
        .context("init DHT")?;
        Some(Arc::new(handle))
    } else {
        info!("DHT disabled (GossipClient mode)");
        None
    };

    // Pre-populate DHT mailbox records from persistent store.
    if let Some(dht) = &dht {
        match store.all_mailbox_recipients() {
            Ok(recipients) => {
                let count = recipients.len();
                for pk in recipients {
                    dht.add_mailbox_record(
                        pk,
                        NodeAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
                    );
                }
                if count > 0 {
                    info!(count, "DHT pre-populated from persistent mailbox store");
                }
            }
            Err(e) => warn!("failed to pre-populate DHT from store: {e}"),
        }
    }

    // ── 6. Metrics ─────────────────────────────────────────────────────────────
    let metrics = Arc::new(NodeMetrics::default());
    let (metrics_tx, metrics_rx) = mpsc::channel::<MetricsUpdate>(64);
    let bw_in = Arc::new(AtomicU64::new(0));
    let bw_out = Arc::new(AtomicU64::new(0));
    spawn_metrics_task(
        Arc::clone(&metrics),
        metrics_rx,
        Arc::clone(&bw_in),
        Arc::clone(&bw_out),
        Instant::now(),
    );

    // ── 7. TLS certificate ─────────────────────────────────────────────────────
    let tls_cert = NodeTlsCert::generate(&signing_key_for_tls).context("generate node TLS cert")?;
    let cert_fingerprint = tls_cert.fingerprint;
    let cert_fingerprint_hex: String = cert_fingerprint
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    // ── 8. NetworkCommand channel ──────────────────────────────────────────────
    let (cmd_tx, cmd_rx) = mpsc::channel::<NetworkCommand>(256);

    // ── 9. QUIC server (Peer and HeadlessSeed modes only) ─────────────────────
    let limiter = Arc::new(ConnectionLimiter::new(config.max_connections.max(10)));
    let (quic_server_opt, quic_port_opt, quic_task_opt, server_push_tx_opt, local_listen_addrs) =
        if server_enabled {
            let quic_addr = SocketAddr::new("0.0.0.0".parse().unwrap(), config.quic_port);

            let make_services = || NodeServicesConfig {
                store: Arc::clone(&store),
                dht: dht.clone(),
                limiter: Arc::clone(&limiter),
                node_pk,
                swarm_cmd_tx: cmd_tx.clone(),
                join_password: join_password.clone(),
            };

            let quic_server = match NodeServer::bind(quic_addr, &tls_cert, make_services()).await {
                Ok(s) => s,
                Err(e) if fallback_to_random_port => {
                    warn!(
                        "Failed to bind to configured port {}: {}; falling back to random port",
                        config.quic_port, e
                    );
                    NodeServer::bind(
                        "0.0.0.0:0".parse::<SocketAddr>().unwrap(),
                        &tls_cert,
                        make_services(),
                    )
                    .await
                    .context("start QUIC server (fallback)")?
                }
                Err(e) => return Err(e).context("start QUIC server"),
            };

            let quic_local_addr = quic_server.local_addr();
            let actual_port = quic_local_addr.port();

            // If the port changed (e.g. fallback to port 0), persist the new value
            // so the next restart binds the same stable port.
            if config.quic_port != actual_port {
                info!(
                    %actual_port,
                    old_port = %config.quic_port,
                    "updating config with stable QUIC port"
                );
                config.quic_port = actual_port;
                if let Err(e) = config.save(&config_path) {
                    warn!("failed to save updated node config: {e}");
                }
            }

            // Update DHT self-address with the bound port.
            // If the socket bound to the wildcard (0.0.0.0 / ::), fall back to
            // loopback so that peer-info announcements contain a reachable address
            // in local / test environments (before NAT/STUN discovers the real IP).
            if let Some(dht) = &dht {
                use std::net::{IpAddr, Ipv4Addr};
                let ip = quic_local_addr.ip();
                let ip = if ip.is_unspecified() {
                    IpAddr::V4(Ipv4Addr::LOCALHOST)
                } else {
                    ip
                };
                dht.update_self_addr(NodeAddr::new(ip, actual_port));
            }

            let server_push_tx = quic_server.push_sender();
            let quic_server_arc = Arc::new(quic_server);
            let quic_for_task = Arc::clone(&quic_server_arc);
            let quic_task = tokio::spawn(async move { quic_for_task.serve().await });

            let local_listen_addr = format!("{}:{}", quic_local_addr.ip(), actual_port);
            info!(addr = %quic_local_addr, "QUIC server ready");

            (
                Some(quic_server_arc),
                Some(actual_port),
                Some(quic_task),
                Some(server_push_tx),
                vec![local_listen_addr],
            )
        } else {
            info!("QUIC server disabled (GossipClient mode)");
            (None, None, None, None, vec![])
        };

    // ── 10. NetworkHandle (QUIC gossip relay) ──────────────────────────────────
    let event_rx = NetworkHandle::spawn_with_channel(
        Arc::clone(&identity),
        local_listen_addrs,
        cmd_rx,
        server_push_tx_opt,
    );
    info!("NetworkHandle gossip task started");

    // ── 11. NAT traversal (background) ────────────────────────────────────────
    if let Some(port) = quic_port_opt {
        let nat_cmd_tx = cmd_tx.clone();
        let dht_nat = dht.clone();
        tokio::spawn(async move {
            if let Some(ext_addr) = crate::network::nat::discover_external_addr(port).await {
                let addr_str = format!("{}:{}", ext_addr.ip(), ext_addr.port());
                // Update DHT self-address with the externally discovered address.
                if let Some(dht) = &dht_nat {
                    dht.update_self_addr(NodeAddr::new(ext_addr.ip(), ext_addr.port()));
                }
                let _ = nat_cmd_tx
                    .send(NetworkCommand::AddListenAddr(addr_str))
                    .await;
            }
        });
    }

    // ── 11b. mDNS LAN discovery ───────────────────────────────────────────────
    {
        let own_pk_hex: String = node_pk.iter().map(|b| format!("{b:02x}")).collect();
        crate::network::mdns::spawn_mdns_task(
            own_pk_hex,
            quic_port_opt.unwrap_or(0),
            cmd_tx.clone(),
            node_mode == NodeMode::GossipClient,
        );
    }

    // ── 12. AppState ───────────────────────────────────────────────────────────
    let app_state = Arc::new(AppState::new(
        peer_id,
        public_key_hex,
        node_address,
        signing_key_for_state,
        config,
        config_path,
        message_log,
        cmd_tx.clone(),
        metrics,
        Some(Arc::clone(&store)),
        encryption_key,
        Some(cert_fingerprint_hex.clone()),
        dht.clone(),
    ));

    // ── 13. Preferred mailbox re-announcement (hourly) ─────────────────────────
    // Re-propagates the user's preferred DM mailbox preference to the DHT.
    // Skipped for GossipClient (no DHT) and HeadlessSeed (no user identity).
    if node_mode == NodeMode::Peer {
        if let Some(dht_repub) = dht.clone() {
            let app_state_repub = Arc::clone(&app_state);
            let our_x25519_pk = {
                let sk = identity.signing_key_bytes();
                crate::identity::NodeIdentity::from_signing_key_bytes(&sk).x25519_public_key_bytes()
            };
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                loop {
                    interval.tick().await;
                    let preferred = app_state_repub
                        .config
                        .read()
                        .await
                        .preferred_mailbox_node
                        .clone();
                    if preferred.is_some() {
                        dht_repub.register_mailbox(our_x25519_pk).await;
                    }
                }
            });
        }
    }

    // ── 14. Community presence re-announcement (hourly) ───────────────────────
    // Re-announces this node's presence in all joined communities to the DHT.
    // Also persists the in-memory community peer snapshot to the DHT store.
    if let Some(dht_comm) = dht.clone() {
        let store_comm = Arc::clone(&store);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.tick().await; // first tick fires immediately — skip it
            loop {
                interval.tick().await;
                if let Ok(communities) = store_comm.all_communities() {
                    for community_pk in communities {
                        dht_comm.register_community_peer(community_pk).await;
                    }
                }
            }
        });
    }

    // ── 14b. Peer info re-announcement (hourly) ───────────────────────────────
    // Announces this node's x25519_pk and QUIC address to the DHT so that
    // other peers can send DMs without a prior shared community.
    // Only for Peer mode (HeadlessSeed has no user identity).
    if node_mode == NodeMode::Peer {
        if let Some(dht_peer_info) = dht.clone() {
            let own_peer_id_bytes = *identity.to_peer_id().as_bytes();
            let own_x25519_pk = identity.x25519_public_key_bytes();
            let display_name_src = Arc::clone(&app_state);
            tokio::spawn(async move {
                // No first-tick skip — fires immediately on startup so peer info is
                // DHT-discoverable right away, unlike the community presence loop
                // (see step 14a) which intentionally delays until the second tick.
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                loop {
                    interval.tick().await;
                    let display_name = display_name_src
                        .config
                        .read()
                        .await
                        .display_name
                        .clone()
                        .unwrap_or_default();
                    dht_peer_info
                        .register_peer_info(own_peer_id_bytes, own_x25519_pk, display_name)
                        .await;
                }
            });
        }
    }

    // ── 15. Swarm event processor ──────────────────────────────────────────────
    let state_for_events = Arc::clone(&app_state);
    let event_proc = tokio::spawn(process_swarm_events(event_rx, state_for_events));

    // ── 16. Bootstrap ──────────────────────────────────────────────────────────
    let _ = app_state.bootstrap_network().await;

    Ok(NodeInitResult {
        app_state,
        cmd_tx,
        metrics_tx,
        bw_in,
        bw_out,
        quic_server: quic_server_opt,
        quic_port: quic_port_opt,
        cert_fingerprint: Some(cert_fingerprint),
        cert_fingerprint_hex: Some(cert_fingerprint_hex),
        quic_task: quic_task_opt,
        event_proc,
    })
}
