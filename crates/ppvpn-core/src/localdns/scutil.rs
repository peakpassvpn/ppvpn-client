//! macOS: the default interface's resolvers from `scutil` (Go:
//! internal/localdns/scutil.go and discover_darwin.go). No cgo or
//! SystemConfiguration bindings: scutil prints what SystemConfiguration holds,
//! manual servers included, which DHCP-only sources miss on static networks.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::servers::{Interface, Server};
use super::Discovered;

/// Fed to scutil: the primary interfaces of IPv4 and IPv6, then the primary
/// service's DNS. The desktop's own entry (a supplemental resolver pointing
/// at the tunnel, State:/Network/Service/com.peakpassvpn.ppvpn.tun/DNS) is
/// a separate service and not part of it; tunnel addresses are filtered
/// anyway.
pub const SCRIPT: &str = "show State:/Network/Global/IPv4\nshow State:/Network/Global/IPv6\nshow State:/Network/Global/DNS\nquit\n";

const TIMEOUT: Duration = Duration::from_secs(2);

/// One top-level dictionary printed by `show`: plain values and arrays
/// (nested dictionaries are skipped).
#[derive(Debug, Default, PartialEq)]
pub struct Dict {
    pub values: HashMap<String, String>,
    pub arrays: HashMap<String, Vec<String>>,
}

/// Splits the output of a script of `show` commands into one result per
/// command, in order: None for "No such key".
pub fn parse_shows(output: &str) -> Vec<Option<Dict>> {
    let mut results = Vec::new();
    let mut current: Option<Dict> = None;
    let mut array: Option<String> = None;
    let mut depth = 0usize;
    for raw in output.split('\n') {
        let line = raw.trim();
        match current.as_mut() {
            None if line == "No such key" => results.push(None),
            None if line == "<dictionary> {" => {
                current = Some(Dict::default());
                depth = 1;
            }
            None => {}
            Some(_) if line == "}" => {
                depth -= 1;
                array = None;
                if depth == 0 {
                    results.push(current.take());
                }
            }
            Some(dict) => {
                let Some((key, value)) = line.split_once(" : ") else {
                    continue;
                };
                let (key, value) = (key.trim(), value.trim());
                if value.ends_with('{') {
                    depth += 1;
                    if depth == 2 && value == "<array> {" {
                        array = Some(key.to_string());
                        dict.arrays.insert(key.to_string(), Vec::new());
                    }
                } else if depth == 1 {
                    dict.values.insert(key.to_string(), value.to_string());
                } else if depth == 2 {
                    if let Some(name) = &array {
                        dict.arrays
                            .get_mut(name)
                            .expect("array")
                            .push(value.to_string());
                    }
                }
            }
        }
    }
    results
}

/// The primary service's DNS servers from the output of [`SCRIPT`] when they
/// belong to `iface`: by the entry's `__IF_INDEX__` when configd recorded
/// it, else when `iface` is the primary interface (IPv4's, or IPv6's on an
/// IPv6-only network). None otherwise: another VPN's service is primary
/// (its utun, with its own DNS), or configd has not caught up with a switch.
pub fn global_servers(output: &str, iface: &str, index: u32) -> Option<Vec<Server>> {
    let shows = parse_shows(output);
    if shows.len() != 3 {
        return None;
    }
    let dns = shows[2].as_ref()?;
    match dns.values.get("__IF_INDEX__") {
        Some(owner) if !owner.is_empty() => {
            if *owner != index.to_string() {
                return None;
            }
        }
        _ => {
            let primary = shows[..2]
                .iter()
                .flatten()
                .find_map(|d| d.values.get("PrimaryInterface").filter(|p| !p.is_empty()));
            if primary.map(String::as_str) != Some(iface) {
                return None;
            }
        }
    }
    Some(parse_addresses(
        dns.arrays
            .get("ServerAddresses")
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    ))
}

/// The nameservers of interface `index`'s scoped resolver in the output of
/// `scutil --dns` ("DNS configuration (for scoped queries)"): the resolvers
/// configd keeps per interface. Ones with a `domain` (split DNS) are skipped.
pub fn scoped_servers(output: &str, index: u32) -> Vec<Server> {
    let Some((_, scoped)) = output.split_once("DNS configuration (for scoped queries)") else {
        return Vec::new();
    };
    let want = index.to_string();
    for resolver in scoped.split("resolver #").skip(1) {
        let mut names = Vec::new();
        let (mut matched, mut domain) = (false, false);
        for raw in resolver.split('\n') {
            let Some((key, value)) = raw.trim().split_once(" : ") else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if key.starts_with("nameserver[") {
                names.push(value.to_string());
            } else if key == "if_index" {
                matched = value.split(' ').next() == Some(want.as_str());
            } else if key == "domain" {
                domain = true;
            }
        }
        if matched && !domain {
            return parse_addresses(&names);
        }
    }
    Vec::new()
}

/// Resolver addresses (zones kept) on port 53; entries that are not
/// addresses are skipped.
pub fn parse_addresses(values: &[String]) -> Vec<Server> {
    values
        .iter()
        .filter_map(|v| {
            let (ip, zone) = match v.split_once('%') {
                Some((ip, zone)) => (ip, Some(zone.to_string())),
                None => (v.as_str(), None),
            };
            let ip = ip.parse().ok()?;
            Some(Server { ip, zone, port: 53 })
        })
        .collect()
}

/// Reads the resolvers of `iface` through scutil: the primary service's when
/// they are `iface`'s, otherwise its scoped resolver.
pub fn discover(iface: &Interface) -> Discovered {
    match run(&[], SCRIPT) {
        Err(e) => return Discovered::failed("scutil-global", e),
        Ok(output) => {
            if let Some(servers) = global_servers(&output, &iface.name, iface.index) {
                if !servers.is_empty() {
                    return Discovered::read("scutil-global", servers);
                }
            }
        }
    }
    match run(&["--dns"], "") {
        Err(e) => Discovered::failed("scutil-scoped", e),
        Ok(output) => Discovered::read("scutil-scoped", scoped_servers(&output, iface.index)),
    }
}

/// Runs /usr/sbin/scutil with `stdin`, within [`TIMEOUT`].
fn run(args: &[&str], stdin: &str) -> Result<String, String> {
    let mut child = Command::new("/usr/sbin/scutil")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("scutil: {e}"))?;
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(stdin.as_bytes());
    }
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => return Err(format!("scutil: {status}")),
            Ok(None) if started.elapsed() >= TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("scutil: timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("scutil: {e}")),
        }
    }
    let mut out = String::new();
    child
        .stdout
        .take()
        .expect("stdout")
        .read_to_string(&mut out)
        .map_err(|e| format!("scutil: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localdns::servers::{join, tunnel_prefixes, usable};

    // The fixtures are the Go tests' (internal/localdns/servers_test.go).
    const GLOBAL: &str = include_str!("testdata/scutil-global.txt");
    const DNS: &str = include_str!("testdata/scutil-dns.txt");
    const V6ONLY: &str = include_str!("testdata/scutil-global-v6only.txt");
    const PENDING: &str = include_str!("testdata/scutil-global-pending.txt");
    const OTHER_VPN: &str = include_str!("testdata/scutil-global-othervpn.txt");

    fn en0() -> Interface {
        Interface {
            index: 6,
            name: "en0".into(),
        }
    }

    // Go: TestGlobalServers (A4-A6).
    #[test]
    fn global_servers_of_the_default_interface() {
        let servers = global_servers(GLOBAL, "en0", 6).expect("en0 is primary");
        assert_eq!(
            join(&usable(&servers, &en0(), &tunnel_prefixes())),
            "192.168.50.3:53,[fe80::1%en0]:53,[2001:db8::53]:53"
        );
        assert!(
            global_servers(GLOBAL, "en1", 7).is_none(),
            "en1 is not primary"
        );
        let v6 = global_servers(V6ONLY, "en1", 7).expect("IPv6-only: primary from Global/IPv6");
        assert_eq!(join(&v6), "[2001:db8::53]:53");
        assert!(global_servers(PENDING, "en0", 6).is_none(), "no DNS yet");
    }

    // A5: another VPN's service is primary; __IF_INDEX__ decides.
    #[test]
    fn another_vpns_dns_is_not_the_default_interfaces() {
        assert!(global_servers(OTHER_VPN, "en0", 6).is_none());
        assert_eq!(
            join(&global_servers(OTHER_VPN, "utun4", 29).expect("by __IF_INDEX__")),
            "10.8.0.53:53"
        );
        let behind = OTHER_VPN.replacen("utun4", "en0", 1);
        assert!(
            global_servers(&behind, "en0", 6).is_none(),
            "__IF_INDEX__ 29 is not en0"
        );
    }

    // Go: TestScopedServers (A7: a resolver with a domain is split DNS).
    #[test]
    fn scoped_servers_of_an_interface() {
        assert_eq!(
            join(&scoped_servers(DNS, 6)),
            "192.168.50.3:53,[fe80::1%en0]:53"
        );
        assert_eq!(join(&scoped_servers(DNS, 16)), "192.168.8.1:53");
        assert!(scoped_servers(DNS, 99).is_empty());
        assert!(scoped_servers(
            "DNS configuration\n\nresolver #1\n  nameserver[0] : 1.1.1.1\n",
            6
        )
        .is_empty());
    }
}
