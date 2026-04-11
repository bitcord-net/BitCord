use anyhow::{Context, Result};
use bitcord_core::{
    api::ApiServer,
    config::{NodeConfig, NodeMode},
    identity::{NodeIdentity, keystore::KeyStore},
    network::NetworkCommand,
    node::{NodeInitConfig, init_node},
    resource::{bandwidth::BandwidthLimiter, metrics::MetricsUpdate, storage::StorageGuard},
};
use clap::{Parser, ValueEnum};
use std::{path::PathBuf, sync::Arc};
use tracing::info;

// ── CLI ───────────────────────────────────────────────────────────────────────

/// Node operating mode.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum CliMode {
    /// No QUIC server, no DHT, no mDNS. Pure gossip receiver.
    GossipClient,
    /// Full peer: QUIC server + DHT + gossip relay.
    Peer,
    /// Headless seed: hosts communities, DM mailboxes, DHT. No user identity.
    HeadlessSeed,
}

#[derive(Parser, Debug)]
#[command(name = "bitcord-node", about = "BitCord headless P2P node")]
struct Args {
    /// Path to config file (default: platform-specific, see NodeConfig::default_path)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Override the data directory
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Node operating mode
    #[arg(long, value_enum)]
    mode: Option<CliMode>,

    /// Log level filter (trace, debug, info, warn, error)
    #[arg(long)]
    log_level: Option<String>,

    /// Port to expose the JSON-RPC API on (0 = disable API server)
    #[arg(long, default_value = "7331")]
    api_port: u16,

    /// Address to bind the JSON-RPC API on.
    /// Defaults to 127.0.0.1 (localhost only). Set to 0.0.0.0 for remote access.
    #[arg(long, default_value = "127.0.0.1")]
    api_bind: std::net::IpAddr,

    /// UDP port for the QUIC transport server (0 = use config value)
    #[arg(long, default_value = "0")]
    quic_port: u16,

    /// Password required for new community registrations on this node.
    /// Can also be set via the BITCORD_JOIN_PASSWORD environment variable.
    #[arg(long, env = "BITCORD_JOIN_PASSWORD")]
    join_password: Option<String>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // ── Load config ───────────────────────────────────────────────────────────
    let config_path = args.config.clone().unwrap_or_else(NodeConfig::default_path);

    let mut config = NodeConfig::load(&config_path)
        .with_context(|| format!("load config from {:?}", config_path))?;

    // Apply CLI overrides.
    if let Some(dir) = args.data_dir {
        config.data_dir = dir.clone();
        config.identity_path = dir.join("identity.key");
    }

    // Resolve node mode: --mode takes precedence.
    if let Some(mode) = args.mode {
        config.node_mode = match mode {
            CliMode::GossipClient => NodeMode::GossipClient,
            CliMode::Peer => NodeMode::Peer,
            CliMode::HeadlessSeed => NodeMode::HeadlessSeed,
        };
    }

    if let Some(level) = args.log_level {
        config.log_level = level;
    }
    if args.quic_port != 0 {
        config.quic_port = args.quic_port;
    }

    // ── Init tracing ──────────────────────────────────────────────────────────
    if std::env::var("BITCORD_TEST_MODE").is_ok() {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(&config.log_level)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(&config.log_level)
            .init();
    }

    info!(
        config = ?config_path,
        data_dir = ?config.data_dir,
        mode = ?config.node_mode,
        "BitCord node starting"
    );

    // ── Identity ──────────────────────────────────────────────────────────────
    // HeadlessSeed nodes auto-generate an identity without prompting for a
    // passphrase — they are meant to be run unattended.
    let passphrase_override = std::env::var("BITCORD_PASSPHRASE").ok();
    let identity = if config.node_mode == NodeMode::HeadlessSeed {
        load_or_create_identity_headless(&config)?
    } else {
        load_or_create_identity(&config, passphrase_override.clone())?
    };
    let identity = Arc::new(identity);

    let join_password = args.join_password.or_else(|| config.join_password.clone());
    let state_dir = config.data_dir.join("state");
    let storage_limit_mb = config.storage_limit_mb;
    let bandwidth_limit_kbps = config.bandwidth_limit_kbps;

    // ── Init node (shared startup logic) ──────────────────────────────────────
    let result = init_node(NodeInitConfig {
        identity,
        passphrase: if config.node_mode == NodeMode::HeadlessSeed {
            Some(String::new()) // empty passphrase for headless
        } else {
            passphrase_override
        },
        config,
        config_path,
        join_password,
        fallback_to_random_port: false,
        dht_self_addr: None, // resolved via NAT/STUN after binding
        store_db_path: state_dir.join("node.redb"),
    })
    .await?;

    let quic_server = result
        .quic_server
        .expect("Peer/HeadlessSeed mode must have QUIC server");
    let quic_task = result
        .quic_task
        .expect("Peer/HeadlessSeed mode must have QUIC task");
    let cert_fingerprint_hex = result.cert_fingerprint_hex.expect("cert always set");

    info!(
        addr = %quic_server.local_addr(),
        fingerprint = %cert_fingerprint_hex,
        "QUIC node server ready"
    );

    // ── Resource guards ───────────────────────────────────────────────────────
    let storage_guard = StorageGuard::new(state_dir.clone(), storage_limit_mb);
    let bw_limiter = BandwidthLimiter::new(bandwidth_limit_kbps);
    BandwidthLimiter::spawn_stats_updater(bw_limiter.stats.clone());

    use std::sync::atomic::Ordering;
    result.bw_in.store(
        bw_limiter.stats.rate_in_kbps.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
    result.bw_out.store(
        bw_limiter.stats.rate_out_kbps.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );

    let _ = result
        .metrics_tx
        .send(MetricsUpdate::DiskUsageMb(
            storage_guard.used_bytes() / (1024 * 1024),
        ))
        .await;
    let _metrics_tx = result.metrics_tx;

    // ── API server ────────────────────────────────────────────────────────────
    let api_handle = if args.api_port == 0 {
        info!("API server disabled (--api-port 0)");
        None
    } else {
        let api_addr = std::net::SocketAddr::new(args.api_bind, args.api_port);
        let handle = ApiServer::start(api_addr, Arc::clone(&result.app_state))
            .await
            .context("start API server")?;
        info!(addr = %handle.local_addr(), "API server ready");
        Some(handle)
    };

    // ── Graceful shutdown via signal ──────────────────────────────────────────
    let cmd_tx_for_signal = result.cmd_tx.clone();
    let quic_server_for_signal = Arc::clone(&quic_server);
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("shutdown signal received");
        quic_server_for_signal.close();
        let _ = cmd_tx_for_signal.send(NetworkCommand::Shutdown).await;
    });

    info!("node running — press Ctrl-C to stop");
    info!("your node address is: {}", result.app_state.node_address);

    result.event_proc.await.ok();
    quic_task.await.ok();
    if let Some(handle) = api_handle {
        handle.stop();
    }

    drop(storage_guard);
    info!("BitCord node stopped");
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Load an existing identity or create a new one.
/// Prompts for passphrase interactively unless `passphrase_override` is set.
fn load_or_create_identity(
    config: &NodeConfig,
    passphrase_override: Option<String>,
) -> Result<NodeIdentity> {
    let path = &config.identity_path;

    if path.exists() {
        let passphrase = match passphrase_override {
            Some(p) => p,
            None => prompt_passphrase("Enter passphrase: ")?,
        };
        KeyStore::load(path, &passphrase).context("failed to unlock identity (wrong passphrase?)")
    } else {
        info!("no identity found at {:?} — creating new identity", path);
        let passphrase = match passphrase_override {
            Some(p) => p,
            None => prompt_new_passphrase()?,
        };
        let identity = NodeIdentity::generate();
        KeyStore::save(path, &identity, &passphrase).context("save new identity")?;
        info!("identity saved; peer ID = {}", identity.to_peer_id());
        Ok(identity)
    }
}

/// Load or auto-generate identity for HeadlessSeed mode (no passphrase prompt).
fn load_or_create_identity_headless(config: &NodeConfig) -> Result<NodeIdentity> {
    let path = &config.identity_path;
    if path.exists() {
        KeyStore::load(path, "").context("failed to load headless seed identity")
    } else {
        info!(
            "no identity found at {:?} — auto-generating headless seed identity",
            path
        );
        let identity = NodeIdentity::generate();
        KeyStore::save(path, &identity, "").context("save headless seed identity")?;
        info!(
            "headless seed identity created; node address = {}",
            identity.node_address()
        );
        Ok(identity)
    }
}

fn prompt_passphrase(prompt: &str) -> Result<String> {
    rpassword::prompt_password(prompt).context("read passphrase")
}

fn prompt_new_passphrase() -> Result<String> {
    loop {
        let p1 = prompt_passphrase("Choose a passphrase: ")?;
        let p2 = prompt_passphrase("Confirm passphrase:  ")?;
        if p1 == p2 {
            return Ok(p1);
        }
        eprintln!("Passphrases do not match — try again.");
    }
}

/// Future that resolves on SIGINT or SIGTERM.
async fn shutdown_signal() {
    use tokio::signal;

    #[cfg(unix)]
    {
        use signal::unix::{SignalKind, signal as unix_signal};
        let mut sigterm = unix_signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        signal::ctrl_c().await.expect("Ctrl-C handler");
    }
}
