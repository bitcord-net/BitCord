use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::network::node_addr::NodeAddr;

/// Snapshot of a single connected peer's state.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// Addresses the peer is known to be reachable at.
    pub addresses: Vec<NodeAddr>,
    /// Round-trip latency from the last successful ping, if any.
    pub latency: Option<Duration>,
    /// Whether this peer can act as a relay for us.
    pub relay_capable: bool,
    /// Simple score in [0, 100]: starts at 50, goes up on good behaviour,
    /// down on misbehaviour.
    pub reputation: i32,
    /// Wall-clock time at which the connection was established.
    pub connected_at: Instant,
}

impl PeerInfo {
    fn new(addresses: Vec<NodeAddr>) -> Self {
        Self {
            addresses,
            latency: None,
            relay_capable: false,
            reputation: 50,
            connected_at: Instant::now(),
        }
    }
}

/// Tracks the set of currently connected peers and enforces a connection limit.
///
/// Peer IDs are hex-encoded Ed25519 public key bytes.
#[derive(Debug)]
pub struct PeerManager {
    peers: HashMap<String, PeerInfo>,
    max_connections: usize,
}

impl PeerManager {
    pub fn new(max_connections: usize) -> Self {
        Self {
            peers: HashMap::new(),
            max_connections,
        }
    }

    /// Record a new connection.
    pub fn peer_connected(&mut self, peer_id: String, addresses: Vec<NodeAddr>) {
        self.peers
            .entry(peer_id)
            .or_insert_with(|| PeerInfo::new(addresses));
    }

    /// Remove a disconnected peer.
    pub fn peer_disconnected(&mut self, peer_id: &str) {
        self.peers.remove(peer_id);
    }

    /// Update the latency measurement for a peer after a successful ping.
    pub fn record_latency(&mut self, peer_id: &str, latency: Duration) {
        if let Some(info) = self.peers.get_mut(peer_id) {
            info.latency = Some(latency);
        }
    }

    /// Mark that this peer can act as a relay for us.
    pub fn set_relay_capable(&mut self, peer_id: &str, capable: bool) {
        if let Some(info) = self.peers.get_mut(peer_id) {
            info.relay_capable = capable;
        }
    }

    /// Adjust a peer's reputation score, clamped to [0, 100].
    pub fn adjust_reputation(&mut self, peer_id: &str, delta: i32) {
        if let Some(info) = self.peers.get_mut(peer_id) {
            info.reputation = (info.reputation + delta).clamp(0, 100);
        }
    }

    /// Returns `true` if the connection limit has been reached.
    pub fn is_at_limit(&self) -> bool {
        self.peers.len() >= self.max_connections
    }

    /// Returns the connected peer count.
    pub fn connected_count(&self) -> usize {
        self.peers.len()
    }

    /// Returns `Some(peer_id)` of the lowest-reputation peer if we are over
    /// `max_connections`, so the caller can close that connection.
    pub fn evict_candidate(&self) -> Option<String> {
        if self.peers.len() <= self.max_connections {
            return None;
        }
        self.peers
            .iter()
            .min_by_key(|(_, info)| info.reputation)
            .map(|(id, _)| id.clone())
    }

    /// Look up information for a specific peer.
    pub fn get(&self, peer_id: &str) -> Option<&PeerInfo> {
        self.peers.get(peer_id)
    }

    /// Iterate over all connected peers.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &PeerInfo)> {
        self.peers.iter()
    }
}
