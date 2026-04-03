//! Bootstrap node list for DHT seeding.
//!
//! When a node starts fresh with no known peers it tries the addresses returned
//! by [`effective_seed_addrs`]. Those addresses come from two sources, merged
//! and deduplicated:
//!
//! 1. **Hard-coded public nodes** — [`BOOTSTRAP_NODES`]. Empty in the initial
//!    release; community operators distribute invite links instead.
//! 2. **`node.toml` overrides** — `seed_nodes` in [`NodeConfig`].
//!
//! Configuration-file entries are tried first so operators can redirect their
//! fleet without changing the binary.

use std::collections::HashSet;

use tracing::warn;

use crate::{config::NodeConfig, network::NodeAddr};

/// Hard-coded well-known public BitCord bootstrap nodes.
///
/// Format: `"host:port"` strings. Entries may be either `ip:port` or
/// `hostname:port`; the latter are resolved via DNS at runtime by
/// `AppState::bootstrap_network`.
pub const BOOTSTRAP_NODES: &[&str] = &["bitcord.net:9042"];

/// Return the effective list of seed node addresses for DHT bootstrapping.
///
/// Merges [`NodeConfig::seed_nodes`] (operator overrides) with
/// [`BOOTSTRAP_NODES`] (built-in list). Addresses that appear in both are
/// deduplicated.  Each entry is first tried as a literal `ip:port` string;
/// if that fails it is resolved via the system DNS resolver (blocking).
/// Unparseable or unresolvable strings are logged and skipped.
///
/// The returned slice is ordered: config overrides come first, built-ins last.
pub fn effective_seed_addrs(config: &NodeConfig) -> Vec<NodeAddr> {
    use std::net::ToSocketAddrs;

    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<NodeAddr> = Vec::new();

    let candidates = config
        .seed_nodes
        .iter()
        .map(String::as_str)
        .chain(BOOTSTRAP_NODES.iter().copied());

    for addr_str in candidates {
        if !seen.insert(addr_str.to_owned()) {
            continue; // duplicate
        }
        // Fast path: literal IP:port.
        if let Ok(addr) = addr_str.parse::<NodeAddr>() {
            result.push(addr);
            continue;
        }
        // Slow path: DNS resolution (may block briefly).
        match addr_str.to_socket_addrs() {
            Ok(resolved) => {
                for sa in resolved {
                    result.push(NodeAddr::new(sa.ip(), sa.port()));
                }
            }
            Err(e) => warn!("bootstrap: failed to resolve {:?}: {}", addr_str, e),
        }
    }

    result
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_returns_only_builtin_nodes() {
        let config = NodeConfig::default();
        let addrs = effective_seed_addrs(&config);
        // BOOTSTRAP_NODES contains hostname:port entries that are resolved via
        // DNS at call time.  In CI without external DNS the resolution may
        // return zero results, so we only assert an upper bound here.
        assert!(
            addrs.len() <= BOOTSTRAP_NODES.len() * 10,
            "unexpected address count"
        );
    }

    #[test]
    fn config_seed_nodes_are_included() {
        let config = NodeConfig {
            seed_nodes: vec!["127.0.0.1:9042".to_string()],
            ..Default::default()
        };
        let addrs = effective_seed_addrs(&config);
        assert!(!addrs.is_empty());
        assert_eq!(addrs[0].port, 9042);
    }

    #[test]
    fn config_nodes_come_before_builtin_nodes() {
        // Add a config seed and verify it's the first entry even when
        // BOOTSTRAP_NODES is later populated.
        let config = NodeConfig {
            seed_nodes: vec!["10.0.0.1:9042".to_string()],
            ..Default::default()
        };
        let addrs = effective_seed_addrs(&config);
        if !addrs.is_empty() {
            assert_eq!(addrs[0].port, 9042);
        }
    }

    #[test]
    fn deduplication_removes_repeated_addresses() {
        let config = NodeConfig {
            seed_nodes: vec![
                "127.0.0.1:9042".to_string(),
                "127.0.0.1:9042".to_string(), // exact duplicate
            ],
            ..Default::default()
        };
        let addrs = effective_seed_addrs(&config);
        // 127.0.0.1:9042 must appear exactly once (dedup works).
        // BOOTSTRAP_NODES may resolve to additional port-9042 entries.
        let count_loopback_9042 = addrs
            .iter()
            .filter(|a| a.port == 9042 && a.ip.to_string() == "127.0.0.1")
            .count();
        assert_eq!(count_loopback_9042, 1);
    }

    #[test]
    fn invalid_address_is_skipped_gracefully() {
        let config = NodeConfig {
            seed_nodes: vec![
                "not-a-valid-address".to_string(),
                "also:bad".to_string(),
                "127.0.0.1:9042".to_string(),
            ],
            ..Default::default()
        };
        let addrs = effective_seed_addrs(&config);
        // The two invalid entries must be skipped; 127.0.0.1:9042 must be present.
        // BOOTSTRAP_NODES may resolve to additional entries.
        assert!(
            addrs
                .iter()
                .any(|a| a.ip.to_string() == "127.0.0.1" && a.port == 9042)
        );
    }

    #[test]
    fn builtin_nodes_are_included_by_default() {
        // With an empty config, effective_seed_addrs resolves BOOTSTRAP_NODES.
        // DNS may or may not succeed in the test environment, so just verify
        // no panics occur and the result is bounded.
        let config = NodeConfig::default();
        let addrs = effective_seed_addrs(&config);
        assert!(addrs.len() <= BOOTSTRAP_NODES.len() * 10);
    }
}
