pub mod bootstrap;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Node configuration, stored as TOML.
///
/// All fields have sensible defaults so a first-run node works without any
/// manual configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeConfig {
    /// Path to the identity keystore file.
    pub identity_path: PathBuf,
    /// Base data directory for state, logs, and config.
    pub data_dir: PathBuf,
    /// Multiaddresses to listen on.
    pub listen_addrs: Vec<String>,
    /// Bootstrap seed node Multiaddresses.
    pub seed_nodes: Vec<String>,
    /// Maximum number of simultaneous connections.
    pub max_connections: usize,
    /// Maximum disk storage for channel state in megabytes.
    pub storage_limit_mb: u64,
    /// Bandwidth limit in kilobits per second (`None` = unlimited).
    pub bandwidth_limit_kbps: Option<u64>,
    /// Whether this node advertises itself as a seed/relay node.
    pub is_seed_node: bool,
    /// Priority for seed node selection (higher = preferred).
    pub seed_priority: u8,
    /// Enable mDNS local network peer discovery.
    pub mdns_enabled: bool,
    /// Tracing log level filter (e.g. `"info"`, `"debug"`, `"warn"`).
    pub log_level: String,
    /// UDP port for the QUIC transport server.
    pub quic_port: u16,
    /// Persisted display name set during onboarding.
    pub display_name: Option<String>,
    /// Whether to run the embedded QUIC server.  When `false` the node acts as
    /// a pure client and does not bind any listening port (reduces resource
    /// usage and attack surface for users who always connect to a seed node).
    pub server_enabled: bool,
    /// If set, clients must supply this password in `JoinCommunity` QUIC requests
    /// to register a new community on this node.  Existing members bypass this
    /// check and can reconnect without a password.  Leave unset for an open node.
    ///
    /// **Deprecated in config.toml** — prefer the `BITCORD_JOIN_PASSWORD`
    /// environment variable or the `--join-password` CLI flag so that the
    /// password is never written to disk.
    #[serde(default, skip_serializing)]
    pub join_password: Option<String>,
    /// Whether the GUI should persist the passphrase in the OS keychain so the
    /// user does not have to re-enter it on every launch.
    #[serde(default)]
    pub save_passphrase: bool,
    /// Preferred DM mailbox node address (`"host:port"`).
    ///
    /// When set, this node announces to the DHT that the user's DM mailbox is
    /// hosted at this address, and falls back to it when no DHT route is found.
    /// Typically set to a community's seed node via the community settings UI.
    #[serde(default)]
    pub preferred_mailbox_node: Option<String>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        let base = default_data_dir();
        Self {
            identity_path: base.join("identity.key"),
            data_dir: base,
            listen_addrs: vec!["0.0.0.0:7332".to_string(), "[::]:7332".to_string()],
            seed_nodes: Vec::new(),
            max_connections: 50,
            storage_limit_mb: 512,
            bandwidth_limit_kbps: None,
            is_seed_node: false,
            seed_priority: 0,
            mdns_enabled: true,
            log_level: "info".to_string(),
            quic_port: 9042,
            display_name: None,
            server_enabled: true,
            join_password: None,
            save_passphrase: false,
            preferred_mailbox_node: None,
        }
    }
}

impl NodeConfig {
    /// Load config from `path`, returning defaults if the file does not exist.
    /// If default is returned, data_dir and identity_path are updated to match path's parent.
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if path.exists() {
            Self::load(path)
        } else {
            let mut cfg = Self::default();
            if let Some(base) = path.parent() {
                cfg.data_dir = base.to_path_buf();
                cfg.identity_path = base.join("identity.key");
            }
            Ok(cfg)
        }
    }

    /// Load config from `path`, returning defaults if the file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let s =
                std::fs::read_to_string(path).with_context(|| format!("read config {:?}", path))?;
            toml::from_str(&s).with_context(|| format!("parse config {:?}", path))
        } else {
            Ok(Self::default())
        }
    }

    /// Serialize and write config to `path` as TOML.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {:?}", parent))?;
        }
        let s = toml::to_string_pretty(self).context("serialize config")?;
        std::fs::write(path, s).with_context(|| format!("write config {:?}", path))
    }

    /// Returns the default config file path (platform-dependent).
    ///
    /// - **Linux:** `~/.local/share/bitcord/config.toml`
    /// - **macOS:** `~/Library/Application Support/com.bitcord.bitcord/config.toml`
    /// - **Windows:** `%LOCALAPPDATA%\bitcord\bitcord\data\config.toml`
    pub fn default_path() -> PathBuf {
        default_data_dir().join("config.toml")
    }
}

fn default_data_dir() -> PathBuf {
    ProjectDirs::from("com", "bitcord", "bitcord")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".bitcord"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let cfg = NodeConfig {
            max_connections: 25,
            log_level: "debug".to_string(),
            bandwidth_limit_kbps: Some(1024),
            ..Default::default()
        };
        cfg.save(&path).unwrap();

        let loaded = NodeConfig::load(&path).unwrap();
        assert_eq!(loaded.max_connections, 25);
        assert_eq!(loaded.log_level, "debug");
        assert_eq!(loaded.bandwidth_limit_kbps, Some(1024));
    }

    #[test]
    fn missing_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        let cfg = NodeConfig::load(&path).unwrap();
        assert_eq!(cfg.max_connections, 50);
        assert_eq!(cfg.storage_limit_mb, 512);
    }
}
