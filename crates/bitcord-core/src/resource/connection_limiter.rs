use tracing::warn;

/// Enforces a ceiling on simultaneous inbound connections, preferring
/// high-reputation peers when at capacity.
pub struct ConnectionLimiter {
    max_connections: usize,
}

impl ConnectionLimiter {
    pub fn new(max_connections: usize) -> Self {
        Self { max_connections }
    }

    /// Returns `true` if an inbound connection from a peer with `incoming_rep`
    /// reputation score should be accepted.
    ///
    /// - If `connected < max_connections`: always allow.
    /// - If at capacity and the incoming peer has a higher reputation than the
    ///   `lowest_rep_connected` peer: allow (caller should evict lowest peer).
    /// - Otherwise: reject.
    pub fn allow_inbound(
        &self,
        connected: usize,
        incoming_rep: i32,
        lowest_rep_connected: Option<i32>,
    ) -> bool {
        if connected < self.max_connections {
            return true;
        }
        if let Some(lowest) = lowest_rep_connected {
            if incoming_rep > lowest {
                return true;
            }
        }
        warn!(
            connected,
            max = self.max_connections,
            "rejecting inbound connection: at limit"
        );
        false
    }

    pub fn max_connections(&self) -> usize {
        self.max_connections
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_when_under_limit() {
        let lim = ConnectionLimiter::new(10);
        assert!(lim.allow_inbound(9, 50, Some(30)));
    }

    #[test]
    fn rejects_when_at_limit_same_rep() {
        let lim = ConnectionLimiter::new(3);
        assert!(!lim.allow_inbound(3, 50, Some(50)));
    }

    #[test]
    fn allows_higher_rep_over_limit() {
        let lim = ConnectionLimiter::new(3);
        assert!(lim.allow_inbound(3, 80, Some(20)));
    }

    #[test]
    fn rejects_at_limit_no_connected_peers() {
        let lim = ConnectionLimiter::new(3);
        assert!(!lim.allow_inbound(3, 50, None));
    }
}
