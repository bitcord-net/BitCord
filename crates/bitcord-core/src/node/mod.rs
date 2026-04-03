//! Node server-side implementation.
//!
//! Provides the persistent storage layer (`store`), per-connection request
//! handler (`handler`), and the QUIC accept loop (`server`).

use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use crate::dht::DhtHandle;
use crate::network::NetworkCommand;
use crate::resource::connection_limiter::ConnectionLimiter;

pub mod handler;
pub mod init;
pub mod server;
pub mod store;

pub use init::{NodeInitConfig, NodeInitResult, init_node};

/// Shared services and configuration for the BitCord node.
///
/// This struct groups common dependencies (storage, DHT, networking) to avoid
/// passing many individual arguments to handlers and servers.
pub struct NodeServices {
    pub store: Arc<store::NodeStore>,
    /// DHT handle for routing lookups; `None` for `GossipClient` mode.
    pub dht: Option<Arc<DhtHandle>>,
    pub limiter: Arc<ConnectionLimiter>,
    pub node_pk: [u8; 32],
    pub swarm_cmd_tx: mpsc::Sender<NetworkCommand>,
    pub push_tx: broadcast::Sender<handler::PushPayload>,
    /// Password required for new community registrations; `None` = open node.
    pub join_password: Option<String>,
}

/// Configuration for creating NodeServices.
pub struct NodeServicesConfig {
    pub store: Arc<store::NodeStore>,
    /// DHT handle; `None` for `GossipClient` mode.
    pub dht: Option<Arc<DhtHandle>>,
    pub limiter: Arc<ConnectionLimiter>,
    pub node_pk: [u8; 32],
    pub swarm_cmd_tx: mpsc::Sender<NetworkCommand>,
    pub join_password: Option<String>,
}
