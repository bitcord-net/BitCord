//! QUIC server for the BitCord node.
//!
//! Accepts incoming QUIC connections, enforces the `ConnectionLimiter`, and
//! spawns a `ConnectionHandler` task for each accepted connection.
//!
//! # Usage
//! ```no_run
//! use std::{net::SocketAddr, sync::Arc};
//! use bitcord_core::{
//!     identity::NodeIdentity,
//!     network::{NetworkCommand, tls::NodeTlsCert},
//!     node::{server::NodeServer, store::NodeStore, NodeServicesConfig},
//!     resource::connection_limiter::ConnectionLimiter,
//! };
//!
//! # async fn example() -> anyhow::Result<()> {
//! let identity = NodeIdentity::generate();
//! let sk = bitcord_core::identity::SigningKey::from_bytes(&identity.signing_key_bytes());
//! let tls_cert = NodeTlsCert::generate(&sk)?;
//! let store = Arc::new(NodeStore::open(std::path::Path::new("node.redb"), None)?);
//! let limiter = Arc::new(ConnectionLimiter::new(50));
//! let node_pk = identity.verifying_key().to_bytes();
//! let (swarm_cmd_tx, _swarm_cmd_rx) = tokio::sync::mpsc::channel::<NetworkCommand>(1);
//!
//! let server = NodeServer::bind(
//!     "0.0.0.0:9042".parse()?,
//!     &tls_cert,
//!     NodeServicesConfig {
//!         store,
//!         dht: None,
//!         limiter,
//!         node_pk,
//!         swarm_cmd_tx,
//!         join_password: None,
//!     },
//! ).await?;
//!
//! println!("QUIC server listening on {}", server.local_addr());
//! server.serve().await;
//! # Ok(())
//! # }
//! ```

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::{
    network::tls::NodeTlsCert,
    node::{NodeServices, NodeServicesConfig, handler::ConnectionHandler, handler::PushPayload},
};

/// Default push broadcast channel capacity.
const PUSH_CAPACITY: usize = 4096;

// ── NodeServer ────────────────────────────────────────────────────────────────

/// The BitCord QUIC node server.
///
/// Wrap in `Arc` to share across tasks (e.g. for graceful shutdown).
pub struct NodeServer {
    endpoint: quinn::Endpoint,
    services: Arc<NodeServices>,
    local_addr: SocketAddr,
    connected: Arc<AtomicUsize>,
}

impl NodeServer {
    /// Bind the QUIC server to `addr` using the node's TLS certificate.
    ///
    /// Does not start accepting connections; call [`serve`](Self::serve) to
    /// enter the accept loop.
    pub async fn bind(
        addr: SocketAddr,
        tls_cert: &NodeTlsCert,
        config: NodeServicesConfig,
    ) -> Result<Self> {
        let server_config = tls_cert
            .server_config()
            .context("build QUIC server TLS config")?;

        let endpoint =
            quinn::Endpoint::server(server_config, addr).context("bind QUIC endpoint")?;
        let local_addr = endpoint.local_addr().context("get QUIC local address")?;

        let (push_tx, _) = broadcast::channel(PUSH_CAPACITY);

        let services = Arc::new(NodeServices {
            store: config.store,
            dht: config.dht,
            limiter: config.limiter,
            node_pk: config.node_pk,
            swarm_cmd_tx: config.swarm_cmd_tx,
            push_tx,
            join_password: config.join_password,
        });

        Ok(Self {
            endpoint,
            services,
            local_addr,
            connected: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The local address this server is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// A clone of the push broadcast sender.
    ///
    /// Callers can use this to inject `NodePush` events from outside the server
    /// (e.g. from the DHT layer or the JSON-RPC API).
    pub fn push_sender(&self) -> broadcast::Sender<PushPayload> {
        self.services.push_tx.clone()
    }

    /// Accept connections in a loop until the endpoint is closed.
    ///
    /// Spawns a `ConnectionHandler` task for each accepted connection.
    pub async fn serve(&self) {
        info!(addr = %self.local_addr, "QUIC node server listening");

        while let Some(incoming) = self.endpoint.accept().await {
            let connected = self.connected.load(Ordering::Relaxed);

            // Enforce connection limit (no reputation data yet → incoming_rep=0).
            if !self.services.limiter.allow_inbound(connected, 0, None) {
                // Dropping `incoming` without awaiting it refuses the connection.
                drop(incoming);
                continue;
            }

            let services = Arc::clone(&self.services);
            let counter = Arc::clone(&self.connected);

            tokio::spawn(async move {
                counter.fetch_add(1, Ordering::Relaxed);
                match incoming.await {
                    Ok(conn) => {
                        let remote = conn.remote_address();
                        info!(%remote, "peer connected");
                        let handler = ConnectionHandler::new(conn, services);
                        handler.run().await;
                        info!(%remote, "peer disconnected");
                    }
                    Err(e) => {
                        warn!("QUIC incoming connection failed: {e}");
                    }
                }
                counter.fetch_sub(1, Ordering::Relaxed);
            });
        }

        info!("QUIC server accept loop exited");
    }

    /// Gracefully close the server endpoint.
    pub fn close(&self) {
        self.endpoint
            .close(quinn::VarInt::from_u32(0), b"server shutting down");
    }

    /// Current number of active connections.
    pub fn connection_count(&self) -> usize {
        self.connected.load(Ordering::Relaxed)
    }
}
