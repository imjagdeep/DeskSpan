//! Finding other computers running DeskSpan on the same network.
//!
//! While sharing, this computer announces itself over multicast DNS (the
//! way printers and AirPlay speakers do) as `_deskspan._tcp`, with its name
//! and port. The Connect page asks for that list; picking one opens its
//! screen page in a DeskSpan window (lib.rs `open_viewer`). Nothing leaves
//! the local network.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SERVICE: &str = "_deskspan._tcp.local.";

/// One DeskSpan computer that is sharing.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Found {
    /// Its computer name.
    pub name: String,
    /// Where to reach it: "ip:port", on the same network as this computer
    /// where possible.
    pub address: String,
    /// Every address it announced, as "ip:port".
    pub addresses: Vec<String>,
}

struct State {
    daemon: Option<ServiceDaemon>,
    /// Our own announcement, while sharing.
    announced: Option<String>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            daemon: None,
            announced: None,
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|p| p.into_inner())
}

fn daemon(s: &mut State) -> Result<ServiceDaemon, String> {
    if let Some(d) = &s.daemon {
        return Ok(d.clone());
    }
    let d = ServiceDaemon::new().map_err(|e| format!("network discovery didn't start: {e}"))?;
    s.daemon = Some(d.clone());
    Ok(d)
}

fn host_name() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// Announce this computer (sharing started). Failures only cost discovery:
/// the address can still be typed in.
pub fn announce(port: u16) {
    let mut s = lock();
    let d = match daemon(&mut s) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("{e}");
            return;
        }
    };
    if let Some(old) = s.announced.take() {
        let _ = d.unregister(&old);
    }
    let name = host_name();
    // mDNS host names are plain labels ending in ".local.".
    let label: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut props = HashMap::new();
    props.insert("name".to_string(), name.clone());
    props.insert("v".to_string(), "1".to_string());
    let info = match ServiceInfo::new(
        SERVICE,
        &name,
        &format!("{label}-deskspan.local."),
        "",
        port,
        props,
    ) {
        Ok(i) => i.enable_addr_auto(),
        Err(e) => {
            tracing::warn!("cannot announce this computer: {e}");
            return;
        }
    };
    let full = info.get_fullname().to_string();
    match d.register(info) {
        Ok(()) => {
            tracing::info!("announced as {full} on port {port}");
            s.announced = Some(full);
        }
        Err(e) => tracing::warn!("cannot announce this computer: {e}"),
    }
}

/// Stop announcing (sharing stopped).
pub fn withdraw() {
    let mut s = lock();
    if let (Some(full), Some(d)) = (s.announced.take(), s.daemon.as_ref()) {
        if let Err(e) = d.unregister(&full) {
            tracing::warn!("cannot withdraw the announcement: {e}");
        }
    }
}

/// The DeskSpan computers sharing on the network right now (not this one).
/// Blocking: listens for up to `wait`.
pub fn find(wait: Duration) -> Result<Vec<Found>, String> {
    // One search at a time (the page asks every few seconds).
    static ONE: Mutex<()> = Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|p| p.into_inner());
    let (d, own) = {
        let mut s = lock();
        (daemon(&mut s)?, s.announced.clone())
    };
    let rx = d
        .browse(SERVICE)
        .map_err(|e| format!("cannot search the network: {e}"))?;
    let mut by_name: HashMap<String, (String, HashSet<Ipv4Addr>, u16)> = HashMap::new();
    let until = Instant::now() + wait;
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(r)) => {
                if own.as_deref() == Some(r.get_fullname()) {
                    continue;
                }
                let name = r
                    .get_property_val_str("name")
                    .map(str::to_string)
                    .unwrap_or_else(|| instance_name(r.get_fullname()));
                let entry = by_name
                    .entry(r.get_fullname().to_string())
                    .or_insert_with(|| (name, HashSet::new(), r.get_port()));
                entry.1.extend(r.get_addresses_v4());
                entry.2 = r.get_port();
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = d.stop_browse(SERVICE);
    let mine = own_networks();
    let mut out: Vec<Found> = by_name
        .into_values()
        .filter_map(|(name, ips, port)| {
            let mut ips: Vec<Ipv4Addr> = ips
                .into_iter()
                .filter(|ip| !ip.is_loopback() && !ip.is_link_local())
                .collect();
            ips.sort_by_key(|ip| (!same_network(*ip, &mine), ip.octets()));
            let first = *ips.first()?;
            Some(Found {
                name,
                address: format!("{first}:{port}"),
                addresses: ips.iter().map(|ip| format!("{ip}:{port}")).collect(),
            })
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

/// "Jag's Mac._deskspan._tcp.local." → "Jag's Mac"
fn instance_name(full: &str) -> String {
    full.strip_suffix(&format!(".{SERVICE}"))
        .unwrap_or(full)
        .replace("\\.", ".")
}

/// This computer's IPv4 networks: (address, netmask).
fn own_networks() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    if_addrs::get_if_addrs()
        .map(|all| {
            all.into_iter()
                .filter_map(|i| match i.addr {
                    if_addrs::IfAddr::V4(v4) if !v4.ip.is_loopback() => Some((v4.ip, v4.netmask)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn same_network(ip: Ipv4Addr, mine: &[(Ipv4Addr, Ipv4Addr)]) -> bool {
    mine.iter().any(|(own, mask)| {
        let m = u32::from(*mask);
        m != 0 && u32::from(*own) & m == u32::from(ip) & m
    })
}

/// A typed address → "host:port" (the port defaults to DeskSpan's).
/// Only a plain host name or IP address and a port: no path, no scheme
/// other than http.
pub fn normalize_address(input: &str, default_port: u16) -> Result<String, String> {
    let s = input.trim();
    let s = s.strip_prefix("http://").unwrap_or(s);
    let s = s.trim_end_matches('/');
    if s.is_empty() {
        return Err("type the other computer's address, like 192.168.1.20".into());
    }
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => {
            let port: u16 = p
                .parse()
                .map_err(|_| format!("\"{p}\" isn't a port number"))?;
            (h, port)
        }
        _ => (s, default_port),
    };
    let ok_host = !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !ok_host || port == 0 {
        return Err(format!(
            "\"{}\" isn't an address DeskSpan can open: use the other computer's IP address or name",
            input.trim()
        ));
    }
    Ok(format!("{host}:{port}"))
}

/// Is this an address on a private home or office network (or a .local
/// name)? The viewer window only opens those.
pub fn is_local_address(address: &str) -> bool {
    let host = address.rsplit_once(':').map_or(address, |(h, _)| h);
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => {
            v4.is_private() || v4.is_link_local() || v4.is_loopback() || {
                // Tailscale and other carrier-grade NAT addresses (100.64/10).
                let o = v4.octets();
                o[0] == 100 && (64..128).contains(&o[1])
            }
        }
        Ok(IpAddr::V6(_)) => false,
        Err(_) => host.ends_with(".local") || !host.contains('.'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_addresses_get_a_port_and_lose_the_rest() {
        assert_eq!(
            normalize_address("192.168.1.20", 47800).unwrap(),
            "192.168.1.20:47800"
        );
        assert_eq!(
            normalize_address(" http://192.168.1.20:5000/ ", 47800).unwrap(),
            "192.168.1.20:5000"
        );
        assert_eq!(
            normalize_address("jags-mac.local", 47800).unwrap(),
            "jags-mac.local:47800"
        );
        assert!(normalize_address("", 47800).is_err());
        assert!(normalize_address("192.168.1.20:port", 47800).is_err());
        assert!(normalize_address("192.168.1.20/admin", 47800).is_err());
        assert!(normalize_address("evil.com\"><script>", 47800).is_err());
        assert!(normalize_address("192.168.1.20:0", 47800).is_err());
    }

    #[test]
    fn only_local_networks_open() {
        assert!(is_local_address("192.168.1.20:47800"));
        assert!(is_local_address("10.0.0.5:47800"));
        assert!(is_local_address("100.81.109.18:47800"));
        assert!(is_local_address("jags-mac.local:47800"));
        assert!(is_local_address("jaggi:47800"));
        assert!(!is_local_address("8.8.8.8:47800"));
        assert!(!is_local_address("example.com:47800"));
    }

    #[test]
    fn same_network_by_netmask() {
        let mine = [(
            Ipv4Addr::new(192, 168, 1, 222),
            Ipv4Addr::new(255, 255, 255, 0),
        )];
        assert!(same_network(Ipv4Addr::new(192, 168, 1, 223), &mine));
        assert!(!same_network(Ipv4Addr::new(192, 168, 56, 1), &mine));
    }

    #[test]
    fn instance_names_unescape() {
        assert_eq!(
            instance_name("Jag\\.s Mac._deskspan._tcp.local."),
            "Jag.s Mac"
        );
    }
}
