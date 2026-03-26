/// Expand a listen address that uses a wildcard IP (`0.0.0.0` or `::`) into one entry
/// per real non-loopback interface, each suffixed with the `/p2p/<peer_id>` component.
///
/// For example `/ip4/0.0.0.0/tcp/7332` with a LAN interface at `192.168.1.10` becomes
/// `/ip4/192.168.1.10/tcp/7332/p2p/<peer_id>`.  Non-wildcard addresses are returned
/// unchanged (just with the suffix appended).
pub(super) fn expand_wildcard_addr(addr_str: &str, _peer_suffix: &str) -> Vec<String> {
    if addr_str.starts_with("0.0.0.0:") {
        let port = addr_str.strip_prefix("0.0.0.0:").unwrap_or("");
        if let Ok(ifaces) = if_addrs::get_if_addrs() {
            let mut expanded: Vec<String> = ifaces
                .into_iter()
                .filter(|i| !i.is_loopback())
                .filter_map(|i| {
                    if let std::net::IpAddr::V4(v4) = i.ip() {
                        Some(format!("{}:{}", v4, port))
                    } else {
                        None
                    }
                })
                .collect();
            // Always include the loopback address so that two nodes running on
            // the same machine can reach each other (nodes on different machines
            // will just get a fast connection-refused on 127.0.0.1 and move on).
            expanded.push(format!("127.0.0.1:{port}"));
            return expanded;
        }
    } else if addr_str.starts_with("[::]:") {
        let port = addr_str.strip_prefix("[::]:").unwrap_or("");
        if let Ok(ifaces) = if_addrs::get_if_addrs() {
            let expanded: Vec<String> = ifaces
                .into_iter()
                .filter(|i| !i.is_loopback())
                .filter_map(|i| {
                    if let std::net::IpAddr::V6(v6) = i.ip() {
                        Some(format!("[{}]:{}", v6, port))
                    } else {
                        None
                    }
                })
                .collect();
            if !expanded.is_empty() {
                return expanded;
            }
        }
    }
    // Non-wildcard address or interface enumeration failed — store as-is.
    vec![addr_str.to_string()]
}

/// Returns `true` if `addr_str` (`"ip:port"`) is a publicly routable address.
///
/// Filters out loopback, RFC1918 private, and link-local addresses so that
/// only STUN/UPnP-discovered external IPs are recorded as the canonical public
/// endpoint for invite links.
pub(super) fn is_publicly_routable(addr_str: &str) -> bool {
    let Ok(sa) = addr_str.parse::<std::net::SocketAddr>() else {
        return false;
    };
    match sa.ip() {
        std::net::IpAddr::V4(v4) => {
            !v4.is_loopback() && !v4.is_private() && !v4.is_link_local() && !v4.is_unspecified()
        }
        std::net::IpAddr::V6(_) => false, // IPv6 public detection is out of scope for now
    }
}
