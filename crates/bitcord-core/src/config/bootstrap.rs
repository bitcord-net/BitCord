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
/// Format: `"ip:port"` strings (same as [`NodeAddr`]'s `FromStr` input).
///
/// # Note
/// This list is intentionally empty for the initial release. The decentralised
/// invite-link model means communities bootstrap themselves via links
/// shared out-of-band, rather than relying on a central seed server.
pub const BOOTSTRAP_NODES: &[&str] = &[
    // Reserved for future public infrastructure nodes.
    // "bootstrap1.bitcord.example:9042",
];

/// Return the effective list of seed node addresses for DHT bootstrapping.
///
/// Merges [`NodeConfig::seed_nodes`] (operator overrides) with
/// [`BOOTSTRAP_NODES`] (built-in list). Addresses that appear in both are
/// deduplicated. Unparseable strings are logged and skipped.
///
/// The returned slice is ordered: config overrides come first, built-ins last.
pub fn effective_seed_addrs(config: &NodeConfig) -> Vec<NodeAddr> {
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
        match addr_str.parse::<NodeAddr>() {
            Ok(addr) => result.push(addr),
            Err(_) => warn!("bootstrap: skipping unparseable address {:?}", addr_str),
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
        assert_eq!(addrs.len(), BOOTSTRAP_NODES.len());
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
        let count_9042 = addrs.iter().filter(|a| a.port == 9042).count();
        assert_eq!(count_9042, 1);
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
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].port, 9042);
    }

    #[test]
    fn empty_seed_nodes_and_no_builtins_gives_empty_result() {
        // BOOTSTRAP_NODES is empty in this build; effective_seed_addrs returns empty.
        let config = NodeConfig::default();
        let addrs = effective_seed_addrs(&config);
        assert_eq!(addrs.len(), BOOTSTRAP_NODES.len());
    }
}
