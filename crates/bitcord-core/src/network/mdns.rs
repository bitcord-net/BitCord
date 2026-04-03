//! mDNS peer discovery — advertises this node's QUIC endpoint as a
//! `_bitcord._udp.local.` service and dials discovered LAN peers.

use std::collections::HashSet;
use std::net::Ipv4Addr;

use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

use crate::network::{NetworkCommand, NodeAddr};

/// Returns the best IPv4 address to advertise via mDNS: a non-loopback,
/// non-virtual, RFC-1918 address on a physical LAN interface.
///
/// Prefers `192.168.x.x` (typical home/office LAN) over `10.x.x.x` /
/// `172.16-31.x.x`. Returns `None` if no suitable address is found, in which
/// case the caller falls back to `""` and lets mdns-sd auto-select.
fn pick_lan_ip() -> Option<Ipv4Addr> {
    let ifaces = if_addrs::get_if_addrs().ok()?;
    let mut best: Option<Ipv4Addr> = None;

    for iface in &ifaces {
        if iface.is_loopback() {
            continue;
        }
        let ip = match iface.ip() {
            std::net::IpAddr::V4(v4) => v4,
            _ => continue,
        };
        if !is_private_ipv4(ip) {
            continue;
        }
        if is_virtual_iface(&iface.name.to_lowercase()) {
            continue;
        }
        // Prefer 192.168.x.x (home/office LAN) over 10.x or 172.x.
        let prefer = match best {
            None => true,
            Some(cur) => ip.octets()[0] == 192 && cur.octets()[0] != 192,
        };
        if prefer {
            best = Some(ip);
        }
    }
    best
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168)
}

/// Returns `true` for known virtual / VPN / container interfaces that should
/// not be used for LAN mDNS advertisement.
fn is_virtual_iface(name: &str) -> bool {
    name.starts_with("tun")
        || name.starts_with("tap")
        || name.starts_with("wg")
        || name.starts_with("utun") // macOS VPN tunnels
        || name.starts_with("virbr") // libvirt/KVM bridges
        || name.starts_with("br-") // Docker bridge networks
        || name.starts_with("docker")
        || name.starts_with("veth")
        || name.starts_with("fct") // e.g. fctvpnXXX (Fortinet etc.)
        || name.contains("vpn")
        || name.contains("nordlynx")
        || name.contains("vethernet") // Windows Hyper-V virtual switches
        || name.contains("default switch")
}

const SERVICE_TYPE: &str = "_bitcord._udp.local.";
const TXT_PK_KEY: &str = "pk";

/// Spawns a background task that advertises this node via mDNS and dials
/// any BitCord peers discovered on the local network.
///
/// - When `is_gossip_client` is `false` (i.e., `Peer` or `HeadlessSeed` mode),
///   the node registers itself as a `_bitcord._udp.local.` service so that other
///   LAN peers can find it, and browses for peers to dial.
/// - When `is_gossip_client` is `true`, mDNS is skipped entirely — the node
///   has no QUIC server to advertise and does not participate in LAN discovery.
pub fn spawn_mdns_task(
    own_pk_hex: String,
    quic_port: u16,
    cmd_tx: mpsc::Sender<NetworkCommand>,
    is_gossip_client: bool,
) {
    if is_gossip_client {
        debug!("mDNS: skipped (GossipClient mode has no QUIC server)");
        return;
    }
    tokio::spawn(run_mdns(own_pk_hex, quic_port, cmd_tx));
}

async fn run_mdns(own_pk_hex: String, quic_port: u16, cmd_tx: mpsc::Sender<NetworkCommand>) {
    use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
    use std::time::Duration;
    use tokio::time;

    info!("mDNS: starting LAN discovery service on port {}", quic_port);

    #[cfg(target_os = "windows")]
    info!("mDNS: Windows platform detected - ensure mDNS (Bonjour) service is running");

    let mdns = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            warn!("mDNS: daemon init failed: {e}; LAN discovery disabled");
            #[cfg(target_os = "windows")]
            warn!(
                "mDNS: On Windows, ensure Bonjour service is installed and running (part of iTunes or Bonjour Print Services)"
            );
            return;
        }
    };

    // Instance name: "bitcord-<first 16 hex chars of pk>" — stays well under
    // the 63-char DNS label limit.
    let instance = format!("bitcord-{}", &own_pk_hex[..16]);
    // Host name used in the SRV record.
    let host = format!("{}.local.", &own_pk_hex[..16]);
    let mut props = std::collections::HashMap::new();
    props.insert(TXT_PK_KEY.to_string(), own_pk_hex.clone());
    props.insert("proto".to_string(), "1".to_string());

    // Prefer a specific physical LAN IP over "" (all interfaces) so that the
    // advertised A record points to the LAN address rather than a VPN or
    // virtual bridge address.
    let lan_ip_str = match pick_lan_ip() {
        Some(ip) => {
            info!(%ip, "mDNS: binding service to LAN interface");
            ip.to_string()
        }
        None => {
            warn!("mDNS: no physical LAN interface found, falling back to auto-select");
            String::new()
        }
    };
    trace!("mDNS: service properties: {:?}", props);
    match ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host,
        lan_ip_str.as_str(),
        quic_port,
        Some(props),
    ) {
        Ok(info) => match mdns.register(info) {
            Ok(()) => {
                info!(port = quic_port, instance, host, "mDNS: registered service");
            }
            Err(e) => warn!("mDNS: register failed: {e}"),
        },
        Err(e) => warn!("mDNS: build ServiceInfo failed: {e}"),
    }

    let browse_rx = match mdns.browse(SERVICE_TYPE) {
        Ok(r) => r,
        Err(e) => {
            warn!("mDNS: browse failed: {e}; LAN discovery disabled");
            return;
        }
    };

    // Spawn health check task
    let health_own_pk_hex = own_pk_hex.clone();
    let health_already_dialed = std::sync::Arc::new(tokio::sync::Mutex::new(HashSet::new()));
    let health_already_dialed_clone = health_already_dialed.clone();

    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let dialed_set = health_already_dialed_clone.lock().await;
            debug!(
                own_pk = %health_own_pk_hex,
                dialed_peers = dialed_set.len(),
                "mDNS: health check"
            );
        }
    });

    // Already-dialed set: keyed by node pk hex to avoid redundant connections.
    // Using Arc<Mutex<HashSet>> so health check task can read it.
    let already_dialed = health_already_dialed;
    // Maps mDNS fullname → pk_hex so we can evict on ServiceRemoved.
    let mut resolved_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    info!("mDNS: browsing for services on {}", SERVICE_TYPE);

    loop {
        trace!("mDNS: waiting for next event...");
        // mdns-sd uses flume channels internally, which expose recv_async().
        match browse_rx.recv_async().await {
            Ok(ServiceEvent::ServiceFound(ty, name)) => {
                debug!(%ty, %name, "mDNS: service found, resolution started by daemon...");
                trace!("mDNS: ServiceFound event - type: {}, name: {}", ty, name);
            }
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let peer_pk = match info.get_property_val_str(TXT_PK_KEY) {
                    Some(pk) => pk.to_string(),
                    None => {
                        debug!("mDNS: service resolved but missing pk property");
                        trace!("mDNS: resolved service info: {:?}", info);
                        continue;
                    }
                };

                let fullname = info.get_fullname().to_string();
                let port = info.get_port();
                let addresses = info.get_addresses();
                let hostname = info.get_hostname();

                trace!(
                    peer_pk,
                    port,
                    hostname = %hostname,
                    "mDNS: service resolved, processing"
                );

                if !addresses.is_empty() {
                    let addr_list: Vec<String> =
                        addresses.iter().map(|ip| format!("{}", ip)).collect();
                    debug!(
                        peer_pk,
                        port,
                        addresses = addr_list.join(", "),
                        "mDNS: service resolved with addresses"
                    );
                }

                if peer_pk == own_pk_hex {
                    trace!("mDNS: ignoring self-advertisement");
                    continue;
                }

                if already_dialed.lock().await.contains(&peer_pk) {
                    debug!(%peer_pk, "mDNS: peer already dialed, skipping");
                    continue;
                }

                if addresses.is_empty() {
                    debug!(%peer_pk, "mDNS: service resolved but no IP addresses found");
                    continue;
                }

                // Prefer IPv4; skip IPv6 for now.
                let mut dialed = false;
                for ip in addresses {
                    if !ip.is_ipv4() {
                        trace!(ip = %ip, "mDNS: skipping IPv6 address");
                        continue;
                    }
                    let addr = NodeAddr::new(*ip, port);
                    info!(
                        addr = %addr,
                        peer_pk,
                        "mDNS: discovered LAN peer, dialing"
                    );
                    already_dialed.lock().await.insert(peer_pk.clone());
                    resolved_names.insert(fullname.clone(), peer_pk.clone());
                    trace!(addr = %addr, peer_pk, "mDNS: sending Dial command");
                    let cmd = NetworkCommand::Dial {
                        addr,
                        is_seed: false,
                        join_community: None,
                        join_community_password: None,
                        // TOFU for LAN peers: we don't know their cert fingerprint
                        // ahead of time.  The node identity (Ed25519 pk in TXT) is
                        // the real authenticator.
                        cert_fingerprint: [0u8; 32],
                    };
                    if let Err(e) = cmd_tx.send(cmd).await {
                        debug!(peer_pk, "mDNS: failed to send Dial command: {e}");
                    }
                    dialed = true;
                    break;
                }

                if !dialed {
                    debug!(%peer_pk, "mDNS: no IPv4 addresses found for peer");
                }
            }
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                if let Some(pk) = resolved_names.remove(&fullname) {
                    already_dialed.lock().await.remove(&pk);
                    debug!(%pk, %fullname, "mDNS: peer removed, evicted from dial cache");
                }
            }
            Ok(ServiceEvent::SearchStopped(ty)) => {
                info!(%ty, "mDNS: search stopped");
                trace!("mDNS: SearchStopped event - type: {}", ty);
                break;
            }
            Ok(evt) => {
                debug!("mDNS: event: {evt:?}");
                trace!("mDNS: detailed event: {:?}", evt);
            }
            Err(e) => {
                warn!("mDNS: browse channel closed: {e}; stopping LAN discovery");
                trace!("mDNS: browse channel error details: {:?}", e);
                break;
            }
        }
    }
}
