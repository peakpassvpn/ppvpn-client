//! Where the default interface's resolvers are read from, per platform (Go:
//! internal/localdns discover_*.go): the `source` of each `local dns
//! servers` log line.
//! - macOS: scutil (Global/DNS of the interface, else its scoped resolver);
//! - Windows: the adapter that is the interface (GetAdaptersAddresses);
//! - Linux: /etc/resolv.conf, or systemd-resolved's upstream list when
//!   resolv.conf only names its 127.0.0.53 stub;
//! - `override`: the host's servers (`--local-dns-servers`), as given;
//! - `testfile` (the `lab` feature): a JSON file of servers per interface
//!   name, `{"eth0": ["192.0.2.53"]}`, named by `PPVPN_LOCALDNS_TEST_FILE`,
//!   which the netns lab rewrites to change the DNS of a namespace's
//!   interfaces.

use super::servers::{Interface, Server};
use super::Discovered;

/// The servers of `iface` as this platform keeps them.
pub(crate) fn system(iface: &Interface) -> Discovered {
    #[cfg(feature = "lab")]
    if let Some(path) = std::env::var_os("PPVPN_LOCALDNS_TEST_FILE") {
        return test_file(std::path::Path::new(&path), iface);
    }
    #[cfg(target_os = "macos")]
    return super::scutil::discover(iface);
    #[cfg(windows)]
    return windows::discover(iface);
    #[cfg(target_os = "linux")]
    {
        let _ = iface;
        return linux::discover();
    }
    #[allow(unreachable_code)]
    Discovered::failed("system", "no DNS source on this platform".into())
}

/// The host's own servers, whatever the interface.
pub(crate) fn overridden(servers: &[Server]) -> Discovered {
    Discovered::read("override", servers.to_vec())
}

/// `{"<interface name>": ["<server>", ...]}`; a missing interface has none.
#[cfg(feature = "lab")]
fn test_file(path: &std::path::Path, iface: &Interface) -> Discovered {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => return Discovered::failed("testfile", format!("{}: {e}", path.display())),
    };
    let map: std::collections::HashMap<String, Vec<String>> = match serde_json::from_str(&text) {
        Ok(map) => map,
        Err(e) => return Discovered::failed("testfile", format!("{}: {e}", path.display())),
    };
    let servers = map
        .get(&iface.name)
        .map(|list| list.iter().filter_map(|s| Server::parse(s)).collect())
        .unwrap_or_default();
    Discovered::read("testfile", servers)
}

/// `nameserver` lines of a resolv.conf.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn resolv_conf_servers(text: &str) -> Vec<Server> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next() == Some("nameserver"))
                .then(|| fields.next())
                .flatten()
        })
        .filter_map(Server::parse)
        .collect()
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    const RESOLV_CONF: &str = "/etc/resolv.conf";
    /// systemd-resolved's upstream servers, behind its 127.0.0.53 stub.
    const RESOLVED: &str = "/run/systemd/resolve/resolv.conf";

    pub(super) fn discover() -> Discovered {
        let text = match std::fs::read_to_string(RESOLV_CONF) {
            Ok(text) => text,
            Err(e) => return Discovered::failed("resolv.conf", format!("{RESOLV_CONF}: {e}")),
        };
        let servers = resolv_conf_servers(&text);
        let stub_only = !servers.is_empty()
            && servers
                .iter()
                .all(|s| s.ip == std::net::Ipv4Addr::new(127, 0, 0, 53));
        if stub_only {
            if let Ok(text) = std::fs::read_to_string(RESOLVED) {
                return Discovered::read("resolved", resolv_conf_servers(&text));
            }
        }
        Discovered::read("resolv.conf", servers)
    }
}

#[cfg(windows)]
mod windows {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST,
        GAA_FLAG_SKIP_UNICAST, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
    };

    use super::*;
    use crate::localdns::adapters::{adapter_servers, Adapter};

    pub(super) fn discover(iface: &Interface) -> Discovered {
        match adapters().and_then(|list| adapter_servers(&list, iface)) {
            Ok(servers) => Discovered::read("adapter", servers),
            Err(e) => Discovered::failed("adapter", e),
        }
    }

    /// Every adapter's indexes and DNS servers.
    fn adapters() -> Result<Vec<Adapter>, String> {
        let flags = GAA_FLAG_SKIP_UNICAST | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
        let mut size: u32 = 16 * 1024;
        // u64 words: the list's structures need 8-byte alignment.
        let mut buf: Vec<u64>;
        loop {
            buf = vec![0u64; (size as usize).div_ceil(8)];
            // SAFETY: the buffer holds `size` bytes; the call writes at most
            // that many, or says how many it needs.
            let rc = unsafe {
                GetAdaptersAddresses(
                    u32::from(AF_UNSPEC),
                    flags,
                    std::ptr::null(),
                    buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
            match rc {
                0 => break,
                ERROR_BUFFER_OVERFLOW => continue,
                rc => return Err(format!("GetAdaptersAddresses: error {rc}")),
            }
        }
        let mut out = Vec::new();
        let mut next = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: a list the call above wrote into `buf`, alive until the
        // end of this function; each pointer is null or into it.
        unsafe {
            while !next.is_null() {
                let adapter = &*next;
                let mut dns = Vec::new();
                let mut server = adapter.FirstDnsServerAddress;
                while !server.is_null() {
                    if let Some(ip) = address(&(*server).Address) {
                        dns.push(ip);
                    }
                    server = (*server).Next;
                }
                out.push(Adapter {
                    if_index: adapter.Anonymous1.Anonymous.IfIndex,
                    ipv6_if_index: adapter.Ipv6IfIndex,
                    dns,
                });
                next = adapter.Next;
            }
        }
        Ok(out)
    }

    /// SAFETY: `a` comes from GetAdaptersAddresses's list.
    unsafe fn address(
        a: &windows_sys::Win32::Networking::WinSock::SOCKET_ADDRESS,
    ) -> Option<IpAddr> {
        let sockaddr = a.lpSockaddr;
        if sockaddr.is_null() {
            return None;
        }
        match (*sockaddr).sa_family {
            AF_INET if a.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN>() => {
                let v4 = &*sockaddr.cast::<SOCKADDR_IN>();
                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                    v4.sin_addr.S_un.S_addr,
                ))))
            }
            AF_INET6 if a.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN6>() => {
                let v6 = &*sockaddr.cast::<SOCKADDR_IN6>();
                Some(IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.u.Byte)))
            }
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // The live table reads: whatever this machine has, the call works
        // and every adapter has an index.
        #[test]
        fn reads_the_adapter_table() {
            let list = adapters().expect("GetAdaptersAddresses");
            assert!(!list.is_empty());
            assert!(list.iter().all(|a| a.if_index != 0 || a.ipv6_if_index != 0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localdns::servers::join;

    #[test]
    fn resolv_conf_nameservers() {
        let text = "# generated\nsearch lan\nnameserver 192.168.1.1\nnameserver  fe80::1%eth0\noptions edns0\nnameserver bogus\n";
        assert_eq!(
            join(&resolv_conf_servers(text)),
            "192.168.1.1:53,[fe80::1%eth0]:53"
        );
    }

    #[test]
    fn the_hosts_servers_are_the_override() {
        let found = overridden(&[Server::parse("10.0.0.53").unwrap()]);
        assert_eq!(
            (found.source.as_str(), join(&found.servers)),
            ("override", "10.0.0.53:53".into())
        );
    }

    #[cfg(feature = "lab")]
    #[test]
    fn the_test_file_names_servers_per_interface() {
        let path =
            std::env::temp_dir().join(format!("ppvpn-core-ldns-test-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"cb": ["10.202.0.1", "10.202.0.53"], "ca": []}"#).unwrap();
        let cb = Interface {
            index: 3,
            name: "cb".into(),
        };
        assert_eq!(
            join(&test_file(&path, &cb).servers),
            "10.202.0.1:53,10.202.0.53:53"
        );
        let missing = Interface {
            index: 4,
            name: "cc".into(),
        };
        assert!(test_file(&path, &missing).servers.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
