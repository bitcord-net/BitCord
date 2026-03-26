use serde::{Deserialize, Serialize};
use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

/// A lightweight node address: an IP + port pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeAddr {
    pub ip: IpAddr,
    pub port: u16,
}

impl NodeAddr {
    pub fn new(ip: IpAddr, port: u16) -> Self {
        Self { ip, port }
    }
}

impl fmt::Display for NodeAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.ip, self.port)
    }
}

impl FromStr for NodeAddr {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let addr: SocketAddr = s
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid NodeAddr {:?}: {}", s, e))?;
        Ok(NodeAddr {
            ip: addr.ip(),
            port: addr.port(),
        })
    }
}
