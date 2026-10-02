//! Resolver addresses as read from the system, and the filter that keeps
//! the usable ones (Go: internal/localdns/servers.go `usable`).

use std::fmt;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6};

/// The default interface: what direct sockets are bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub index: u32,
    pub name: String,
}

/// A resolver address: an IP, the zone it was written with (an interface
/// name or index, link-local IPv6 only) and a port.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Server {
    pub ip: IpAddr,
    pub zone: Option<String>,
    pub port: u16,
}

impl Server {
    pub fn new(ip: IpAddr, port: u16) -> Self {
        Server {
            ip,
            zone: None,
            port,
        }
    }

    /// Parses `ip`, `ip%zone`, `ip:port` or `[ip%zone]:port`; port 53 when
    /// none is given.
    pub fn parse(s: &str) -> Option<Server> {
        let s = s.trim();
        let (host, port) = if let Some(rest) = s.strip_prefix('[') {
            let (host, tail) = rest.split_once(']')?;
            let port = match tail.strip_prefix(':') {
                Some(p) => p.parse().ok()?,
                None if tail.is_empty() => 53,
                None => return None,
            };
            (host, port)
        } else if s.matches(':').count() == 1 {
            let (host, port) = s.split_once(':')?;
            (host, port.parse().ok()?)
        } else {
            (s, 53)
        };
        let (ip, zone) = match host.split_once('%') {
            Some((ip, zone)) if !zone.is_empty() => (ip, Some(zone.to_string())),
            Some(_) => return None,
            None => (host, None),
        };
        let ip: IpAddr = ip.parse().ok()?;
        if zone.is_some() && !matches!(ip, IpAddr::V6(_)) {
            return None;
        }
        Some(Server { ip, zone, port })
    }

    /// The socket address to dial: a zone by name resolves to the
    /// interface's index only through `usable`, which keeps link-local
    /// servers of the default interface alone.
    pub fn socket_addr(&self, iface: &Interface) -> SocketAddr {
        match self.ip {
            IpAddr::V6(v6) if self.zone.is_some() => {
                SocketAddr::V6(SocketAddrV6::new(v6, self.port, 0, iface.index))
            }
            ip => SocketAddr::new(ip, self.port),
        }
    }
}

impl fmt::Display for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.ip, &self.zone) {
            (IpAddr::V4(v4), _) => write!(f, "{}:{}", v4, self.port),
            (IpAddr::V6(v6), Some(zone)) => write!(f, "[{}%{}]:{}", v6, zone, self.port),
            (IpAddr::V6(v6), None) => write!(f, "[{}]:{}", v6, self.port),
        }
    }
}

/// Joins servers as the logs print them: "a:53,[b]:53".
pub fn join(servers: &[Server]) -> String {
    servers
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// An IP prefix (the core's own tunnel ranges are excluded with these).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefix {
    addr: IpAddr,
    len: u8,
}

impl Prefix {
    pub fn parse(s: &str) -> Option<Prefix> {
        let (addr, len) = s.split_once('/')?;
        let addr: IpAddr = addr.parse().ok()?;
        let len: u8 = len.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        (len <= max).then_some(Prefix { addr, len })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.len == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.len)
                };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.len == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.len)
                };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// The core's own tunnel ranges, current and from before 0.5.7: a resolver
/// in them is the tunnel's and querying it would loop.
pub fn tunnel_prefixes() -> Vec<Prefix> {
    [
        "10.60.159.88/30",
        "fde2:ec40:9312:c7fd::/126",
        "172.19.0.0/30",
        "fdfe:dcba:9876::/126",
    ]
    .iter()
    .map(|p| Prefix::parse(p).expect("tunnel prefix"))
    .collect()
}

fn is_link_local_v6(ip: &Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

fn is_site_local_v6(ip: &Ipv6Addr) -> bool {
    // fec0::/10, the deprecated defaults Windows lists on adapters without
    // IPv6 DNS of their own.
    ip.segments()[0] & 0xffc0 == 0xfec0
}

/// Keeps the usable resolvers read for `iface`: loopback, unspecified and
/// multicast addresses, fec0::/10 and the excluded prefixes are left out,
/// as are duplicates; IPv4-mapped addresses become IPv4. A link-local IPv6
/// resolver is kept only on `iface`: without a zone it gets the interface's
/// index, with another interface's zone it is dropped (the socket is bound
/// to `iface` and could not reach it). Port 0 becomes 53.
pub fn usable(servers: &[Server], iface: &Interface, exclude: &[Prefix]) -> Vec<Server> {
    let mut out: Vec<Server> = Vec::new();
    for server in servers {
        let ip = match server.ip {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => IpAddr::V4(v4),
                None => IpAddr::V6(v6),
            },
            ip => ip,
        };
        if ip.is_unspecified()
            || ip.is_loopback()
            || ip.is_multicast()
            || exclude.iter().any(|p| p.contains(ip))
        {
            continue;
        }
        let zone = match ip {
            IpAddr::V6(v6) if is_site_local_v6(&v6) => continue,
            IpAddr::V6(v6) if is_link_local_v6(&v6) => match server.zone.as_deref() {
                None => Some(iface.index.to_string()),
                Some(z) if z == iface.name || z == iface.index.to_string() => Some(z.to_string()),
                Some(_) => continue,
            },
            _ => None,
        };
        let port = if server.port == 0 { 53 } else { server.port };
        let server = Server { ip, zone, port };
        if !out.contains(&server) {
            out.push(server);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn servers(values: &[&str]) -> Vec<Server> {
        values
            .iter()
            .map(|v| Server::parse(v).unwrap_or_else(|| panic!("parse {v}")))
            .collect()
    }

    pub(crate) fn en0() -> Interface {
        Interface {
            index: 6,
            name: "en0".into(),
        }
    }

    // Go: TestUsableLeavesOutTunnelLoopbackAndForeignLinkLocal (A1-A3).
    #[test]
    fn usable_leaves_out_tunnel_loopback_and_foreign_link_local() {
        let got = usable(
            &servers(&[
                "192.168.50.3",
                "172.19.0.2",
                "10.60.159.90",
                "fde2:ec40:9312:c7fd::2",
                "fdfe:dcba:9876::2",
                "127.0.0.1",
                "::1",
                "0.0.0.0",
                "fec0:0:0:ffff::1",
                "224.0.0.251",
                "fe80::1%en0",
                "fe80::2%en1",
                "fe80::3",
                "fe80::4%6",
                "::ffff:192.168.1.1",
                "192.168.50.3",
                "[2001:db8::53]:5353",
            ]),
            &en0(),
            &tunnel_prefixes(),
        );
        assert_eq!(
            join(&got),
            "192.168.50.3:53,[fe80::1%en0]:53,[fe80::3%6]:53,[fe80::4%6]:53,192.168.1.1:53,[2001:db8::53]:5353"
        );
    }

    #[test]
    fn parse_forms() {
        assert_eq!(
            Server::parse("10.0.0.1").unwrap().to_string(),
            "10.0.0.1:53"
        );
        assert_eq!(
            Server::parse("10.0.0.1:5353").unwrap().to_string(),
            "10.0.0.1:5353"
        );
        assert_eq!(
            Server::parse("[fe80::1%en0]:53").unwrap().to_string(),
            "[fe80::1%en0]:53"
        );
        assert_eq!(
            Server::parse("fe80::1%en0").unwrap().to_string(),
            "[fe80::1%en0]:53"
        );
        assert_eq!(
            Server::parse("2001:db8::53").unwrap().to_string(),
            "[2001:db8::53]:53"
        );
        assert!(Server::parse("dns.example").is_none());
        assert!(Server::parse("10.0.0.1%en0").is_none());
    }

    #[test]
    fn link_local_dials_the_interface_index() {
        let s = usable(&servers(&["fe80::1%en0"]), &en0(), &[]);
        assert_eq!(s[0].socket_addr(&en0()).to_string(), "[fe80::1%6]:53");
    }
}
