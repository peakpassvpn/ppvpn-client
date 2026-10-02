//! Windows: the resolvers of the adapter that is the default interface (Go:
//! internal/localdns/discover_windows.go). The selection is a pure function
//! over the adapter list; reading the list (GetAdaptersAddresses) comes with
//! the Windows build, once sail and this crate build for x86_64-pc-windows-msvc.

use std::net::IpAddr;

use super::servers::{Interface, Server};

/// What GetAdaptersAddresses gives for one adapter, as far as DNS goes.
#[derive(Clone, Debug)]
pub struct Adapter {
    pub if_index: u32,
    pub ipv6_if_index: u32,
    pub dns: Vec<IpAddr>,
}

/// The DNS servers of the adapter that is `iface` (by IPv4 or IPv6 index).
/// No gateway is required: `iface` holds the default route. A link-local
/// IPv6 server gets the adapter's IPv6 index as its zone; filtering
/// (fec0::/10, the tunnel, loopback) is `usable`'s.
pub fn adapter_servers(adapters: &[Adapter], iface: &Interface) -> Result<Vec<Server>, String> {
    let adapter = adapters
        .iter()
        .find(|a| a.if_index == iface.index || a.ipv6_if_index == iface.index)
        .ok_or_else(|| format!("no adapter with index {}", iface.index))?;
    Ok(adapter
        .dns
        .iter()
        .map(|ip| {
            let ip = match ip {
                IpAddr::V6(v6) => v6
                    .to_ipv4_mapped()
                    .map(IpAddr::V4)
                    .unwrap_or(IpAddr::V6(*v6)),
                ip => *ip,
            };
            let zone = match ip {
                IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80 => {
                    Some(adapter.ipv6_if_index.to_string())
                }
                _ => None,
            };
            Server { ip, zone, port: 53 }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localdns::servers::{join, tunnel_prefixes, usable};

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    // A8. The Go core has no fixture for this (discover_windows.go reads the
    // live table); the list below is shaped like a host's: Wintun with the
    // tunnel's resolver, Wi-Fi with fec0 defaults and a link-local resolver,
    // and the Ethernet adapter that is the default interface.
    fn table() -> Vec<Adapter> {
        vec![
            Adapter {
                if_index: 21,
                ipv6_if_index: 21,
                dns: vec![ip("10.60.159.90"), ip("fde2:ec40:9312:c7fd::2")],
            },
            Adapter {
                if_index: 9,
                ipv6_if_index: 9,
                dns: vec![
                    ip("fec0:0:0:ffff::1"),
                    ip("fec0:0:0:ffff::2"),
                    ip("fe80::1"),
                ],
            },
            Adapter {
                if_index: 12,
                ipv6_if_index: 13,
                dns: vec![ip("192.168.50.3"), ip("fe80::5"), ip("::ffff:192.168.50.4")],
            },
        ]
    }

    #[test]
    fn only_the_default_adapters_servers() {
        let iface = Interface {
            index: 12,
            name: "Ethernet".into(),
        };
        let raw = adapter_servers(&table(), &iface).unwrap();
        assert_eq!(
            join(&raw),
            "192.168.50.3:53,[fe80::5%13]:53,192.168.50.4:53"
        );
        // The IPv6 index finds it too.
        assert!(adapter_servers(
            &table(),
            &Interface {
                index: 13,
                name: "Ethernet".into()
            }
        )
        .is_ok());
    }

    #[test]
    fn site_local_defaults_and_the_tunnel_are_filtered() {
        let wifi = Interface {
            index: 9,
            name: "Wi-Fi".into(),
        };
        let got = usable(
            &adapter_servers(&table(), &wifi).unwrap(),
            &wifi,
            &tunnel_prefixes(),
        );
        assert_eq!(join(&got), "[fe80::1%9]:53");
        let wintun = Interface {
            index: 21,
            name: "ppvpn".into(),
        };
        assert!(usable(
            &adapter_servers(&table(), &wintun).unwrap(),
            &wintun,
            &tunnel_prefixes()
        )
        .is_empty());
    }

    #[test]
    fn unknown_index_is_an_error() {
        let err = adapter_servers(
            &table(),
            &Interface {
                index: 99,
                name: "x".into(),
            },
        )
        .unwrap_err();
        assert_eq!(err, "no adapter with index 99");
    }
}
