//! NAT traversal — discovers an externally reachable address for the node.
//!
//! Two strategies are tried in order:
//!
//! 1. **UPnP IGD** — asks the router to open a port mapping and returns the
//!    gateway's external IP with the same port number.
//! 2. **STUN** — sends a minimal RFC 5389 Binding Request to a public STUN
//!    server to learn the external IP, then pairs it with the local QUIC port.
//!
//! Both strategies run with a 5-second timeout.  If both fail the function
//! returns `None` and the node simply advertises its LAN address(es) as usual.

use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    time::Duration,
};

use rand::RngCore;
use tracing::{debug, info, warn};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Well-known public STUN servers (Google + Cloudflare).
const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

// ── Public API ────────────────────────────────────────────────────────────────

/// Attempt to discover an externally reachable `SocketAddr` for `local_port`.
///
/// Tries UPnP first; falls back to STUN.  Returns `None` if both fail.
pub async fn discover_external_addr(local_port: u16) -> Option<SocketAddr> {
    // ── UPnP ─────────────────────────────────────────────────────────────────
    match tokio::time::timeout(DISCOVERY_TIMEOUT, try_upnp(local_port)).await {
        Ok(Some(addr)) => {
            info!(%addr, "NAT: UPnP port mapping established");
            return Some(addr);
        }
        Ok(None) => debug!("NAT: UPnP unavailable, trying STUN"),
        Err(_) => warn!("NAT: UPnP timed out, trying STUN"),
    }

    // ── STUN ─────────────────────────────────────────────────────────────────
    match tokio::time::timeout(DISCOVERY_TIMEOUT, try_stun(local_port)).await {
        Ok(Some(addr)) => {
            info!(%addr, "NAT: STUN reflexive address discovered");
            Some(addr)
        }
        Ok(None) => {
            debug!("NAT: STUN failed — node will only advertise LAN addresses");
            None
        }
        Err(_) => {
            warn!("NAT: STUN timed out — node will only advertise LAN addresses");
            None
        }
    }
}

// ── UPnP ─────────────────────────────────────────────────────────────────────

async fn try_upnp(local_port: u16) -> Option<SocketAddr> {
    let options = igd::SearchOptions {
        timeout: Some(Duration::from_secs(3)),
        ..Default::default()
    };

    let gateway = match igd::aio::search_gateway(options).await {
        Ok(g) => g,
        Err(e) => {
            debug!("UPnP: gateway search failed: {e}");
            return None;
        }
    };

    let external_ip: Ipv4Addr = match gateway.get_external_ip().await {
        Ok(ip) => ip,
        Err(e) => {
            debug!("UPnP: get_external_ip failed: {e}");
            return None;
        }
    };

    let local_ip = local_ipv4()?;
    let local_addr = SocketAddrV4::new(local_ip, local_port);

    match gateway
        .add_port(
            igd::PortMappingProtocol::UDP,
            local_port,
            local_addr,
            0,
            "BitCord QUIC",
        )
        .await
    {
        Ok(()) => {
            debug!(
                "UPnP: port mapping added ({} UDP → {})",
                local_port, local_addr
            );
            Some(SocketAddr::new(external_ip.into(), local_port))
        }
        Err(igd::AddPortError::PortInUse) => {
            // A mapping already exists (e.g. from a previous run) — treat as success.
            debug!("UPnP: port {} already mapped", local_port);
            Some(SocketAddr::new(external_ip.into(), local_port))
        }
        Err(e) => {
            debug!("UPnP: add_port failed: {e}");
            None
        }
    }
}

// ── STUN ─────────────────────────────────────────────────────────────────────

async fn try_stun(local_port: u16) -> Option<SocketAddr> {
    for server in STUN_SERVERS {
        if let Some(addr) = stun_binding_request(server).await {
            // The STUN socket gets a separate external port from the router;
            // pair the discovered external IP with the QUIC port so the invite
            // link is useful for nodes behind full-cone / port-restricted NATs.
            let ext_addr = SocketAddr::new(addr.ip(), local_port);
            return Some(ext_addr);
        }
    }
    None
}

/// Send an RFC 5389 Binding Request and parse the XOR-MAPPED-ADDRESS response.
async fn stun_binding_request(server: &str) -> Option<SocketAddr> {
    let socket = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            debug!("STUN: bind failed: {e}");
            return None;
        }
    };
    if let Err(e) = socket.connect(server).await {
        debug!("STUN: connect to {server} failed: {e}");
        return None;
    }

    // 20-byte STUN Binding Request
    let mut request = [0u8; 20];
    request[0] = 0x00;
    request[1] = 0x01; // message type: Binding Request
    // length = 0 (no attributes)
    request[4] = 0x21;
    request[5] = 0x12;
    request[6] = 0xA4;
    request[7] = 0x42; // magic cookie

    let mut txid = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut txid);
    request[8..20].copy_from_slice(&txid);

    if let Err(e) = socket.send(&request).await {
        debug!("STUN: send to {server} failed: {e}");
        return None;
    }

    let mut buf = [0u8; 512];
    let n = match tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut buf)).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            debug!("STUN: recv from {server} failed: {e}");
            return None;
        }
        Err(_) => {
            debug!("STUN: timeout waiting for response from {server}");
            return None;
        }
    };

    parse_stun_response(&buf[..n], &txid)
}

fn parse_stun_response(buf: &[u8], txid: &[u8; 12]) -> Option<SocketAddr> {
    if buf.len() < 20 {
        return None;
    }
    // Binding Success Response = 0x0101
    if buf[0] != 0x01 || buf[1] != 0x01 {
        return None;
    }
    // Magic cookie
    if buf[4..8] != [0x21, 0x12, 0xA4, 0x42] {
        return None;
    }
    // Transaction ID
    if buf[8..20] != *txid {
        return None;
    }

    let msg_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    if buf.len() < 20 + msg_len {
        return None;
    }

    let mut pos = 20;
    let mut mapped: Option<SocketAddr> = None;

    while pos + 4 <= 20 + msg_len {
        let attr_type = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let attr_len = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
        pos += 4;

        if pos + attr_len > buf.len() {
            break;
        }

        let attr_data = &buf[pos..pos + attr_len];
        match attr_type {
            0x0020 => {
                // XOR-MAPPED-ADDRESS — preferred; return immediately
                if let Some(addr) = parse_xor_mapped_address(attr_data) {
                    return Some(addr);
                }
            }
            0x0001 => {
                // MAPPED-ADDRESS — fallback if XOR variant absent
                mapped = parse_mapped_address(attr_data);
            }
            _ => {}
        }

        // Attributes are padded to 4-byte boundary
        pos += (attr_len + 3) & !3;
    }

    mapped
}

fn parse_xor_mapped_address(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 8 {
        return None;
    }
    let family = data[1];
    let xport = u16::from_be_bytes([data[2], data[3]]);
    let port = xport ^ 0x2112; // XOR with high 16 bits of magic cookie

    match family {
        0x01 if data.len() >= 8 => {
            let xaddr = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
            let addr = xaddr ^ 0x2112_A442;
            Some(SocketAddr::new(Ipv4Addr::from(addr).into(), port))
        }
        _ => None, // IPv6 not needed for basic NAT traversal
    }
}

fn parse_mapped_address(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 8 {
        return None;
    }
    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);

    match family {
        0x01 if data.len() >= 8 => {
            let addr = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
            Some(SocketAddr::new(Ipv4Addr::from(addr).into(), port))
        }
        _ => None,
    }
}

/// Return the first non-loopback, non-link-local IPv4 address on this host.
fn local_ipv4() -> Option<Ipv4Addr> {
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            if let std::net::IpAddr::V4(v4) = iface.ip() {
                if !v4.is_link_local() {
                    return Some(v4);
                }
            }
        }
    }
    None
}
