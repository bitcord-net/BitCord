use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Operating mode of the node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeMode {
    /// No QUIC server, no DHT, no mDNS.  Pure gossip receiver.
    /// Ideal for users who always connect to a seed node.
    GossipClient,
    /// Full peer: QUIC server + DHT + gossip relay + user identity.
    /// This is the default for interactive users.
    #[default]
    Peer,
    /// Headless only.  No user identity.
    /// Hosts communities, DM mailboxes, and DHT routing.  Never sends
    /// presence/`MemberJoined` events.  No GUI.
    HeadlessSeed,
}

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
    /// Maximum number of simultaneous connections.
    pub max_connections: usize,
    /// Maximum disk storage for channel state in megabytes.
    pub storage_limit_mb: u64,
    /// Bandwidth limit in kilobits per second (`None` = unlimited).
    pub bandwidth_limit_kbps: Option<u64>,
    /// Node operating mode.
    pub node_mode: NodeMode,
    /// Priority for seed node selection (higher = preferred).
    pub seed_priority: u8,
    /// Tracing log level filter (e.g. `"info"`, `"debug"`, `"warn"`).
    pub log_level: String,
    /// UDP port for the QUIC transport server.
    pub quic_port: u16,
    /// Persisted display name set during onboarding.
    pub display_name: Option<String>,
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
            max_connections: 50,
            storage_limit_mb: 512,
            bandwidth_limit_kbps: None,
            node_mode: NodeMode::Peer,
            seed_priority: 0,
            log_level: "info".to_string(),
            quic_port: 9042,
            display_name: None,
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
            let cfg: Self =
                toml::from_str(&s).with_context(|| format!("parse config {:?}", path))?;
            Ok(cfg)
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
    /// - **Linux:** `~/.local/share/net.bitcord.node/config.toml`
    /// - **macOS:** `~/Library/Application Support/net.bitcord.node/config.toml`
    /// - **Windows:** `%APPDATA%\net.bitcord.node\config.toml`
    pub fn default_path() -> PathBuf {
        default_data_dir().join("config.toml")
    }
}

fn default_data_dir() -> PathBuf {
    ProjectDirs::from("net", "bitcord", "node")
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
        assert_eq!(loaded.node_mode, NodeMode::Peer);
    }

    #[test]
    fn missing_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        let cfg = NodeConfig::load(&path).unwrap();
        assert_eq!(cfg.max_connections, 50);
        assert_eq!(cfg.storage_limit_mb, 512);
        assert_eq!(cfg.node_mode, NodeMode::Peer);
    }

    #[test]
    fn node_mode_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let cfg = NodeConfig {
            node_mode: NodeMode::GossipClient,
            ..Default::default()
        };
        cfg.save(&path).unwrap();
        let loaded = NodeConfig::load(&path).unwrap();
        assert_eq!(loaded.node_mode, NodeMode::GossipClient);
    }
}
