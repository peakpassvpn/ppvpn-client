//! Linux: the disable_ipv6 sysctls, /proc/net/if_inet6 and
//! /proc/net/ipv6_route.

use std::collections::HashSet;
use std::io;
use std::net::Ipv6Addr;

use super::global_unicast;

/// The sysctls that decide whether a new interface (the TUN) gets IPv6:
/// "all" overrides every interface, "default" is what interfaces created
/// later inherit. Without /proc/sys/net/ipv6 at all the kernel runs with
/// ipv6.disable=1.
pub(crate) const DISABLE_IPV6_FILES: [&str; 2] = [
    "/proc/sys/net/ipv6/conf/all/disable_ipv6",
    "/proc/sys/net/ipv6/conf/default/disable_ipv6",
];
pub(crate) const IF_INET6_FILE: &str = "/proc/net/if_inet6";
pub(crate) const IPV6_ROUTE_FILE: &str = "/proc/net/ipv6_route";

const RTF_UP: u32 = 0x1;
const RTF_REJECT: u32 = 0x200;

pub(crate) fn available() -> bool {
    available_from(|name| std::fs::read(name))
}

pub(crate) fn route() -> Result<bool, String> {
    route_from(|name| std::fs::read(name))
}

pub(crate) fn available_from(read: impl Fn(&str) -> io::Result<Vec<u8>>) -> bool {
    for name in DISABLE_IPV6_FILES {
        match read(name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
            // Unreadable but present: keep IPv6 so a leak stays impossible;
            // a stack really disabled then fails the start visibly.
            Err(_) => continue,
            Ok(value) if String::from_utf8_lossy(&value).trim() != "0" => return false,
            Ok(_) => {}
        }
    }
    true
}

/// /proc/net/if_inet6 ("addr ifindex plen scope flags name") and
/// /proc/net/ipv6_route ("dst dstlen src srclen nexthop metric refcnt use
/// flags name"): one interface with a global unicast address and a usable
/// default route (::/0, up, not a reject route).
pub(crate) fn route_from(read: impl Fn(&str) -> io::Result<Vec<u8>>) -> Result<bool, String> {
    let addrs = read(IF_INET6_FILE).map_err(|e| format!("{IF_INET6_FILE}: {e}"))?;
    let mut global = HashSet::new();
    for line in String::from_utf8_lossy(&addrs).lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        if hex_addr(fields[0]).is_some_and(|a| global_unicast(&a)) {
            global.insert(fields[5].to_owned());
        }
    }
    if global.is_empty() {
        return Ok(false);
    }
    let routes = read(IPV6_ROUTE_FILE).map_err(|e| format!("{IPV6_ROUTE_FILE}: {e}"))?;
    for line in String::from_utf8_lossy(&routes).lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 || fields[1] != "00" || !fields[0].bytes().all(|b| b == b'0') {
            continue;
        }
        let Ok(flags) = u32::from_str_radix(fields[8], 16) else {
            continue;
        };
        if flags & RTF_UP == 0 || flags & RTF_REJECT != 0 {
            continue;
        }
        if global.contains(fields[9]) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The 32 hex digits /proc uses for an IPv6 address.
fn hex_addr(s: &str) -> Option<Ipv6Addr> {
    if s.len() != 32 {
        return None;
    }
    let mut octets = [0u8; 16];
    for (i, octet) in octets.iter_mut().enumerate() {
        *octet = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(Ipv6Addr::from(octets))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn not_found() -> io::Error {
        io::Error::from(io::ErrorKind::NotFound)
    }

    // Go: TestLinuxAvailability.
    #[test]
    fn availability() {
        struct Case {
            name: &'static str,
            all: &'static str,
            default: &'static str,
            all_err: Option<io::ErrorKind>,
            want: bool,
        }
        let case = |name, all, default, all_err, want| Case {
            name,
            all,
            default,
            all_err,
            want,
        };
        for c in [
            case("enabled", "0\n", "0\n", None, true),
            case("all disabled", "1\n", "0\n", None, false),
            case("default disabled", "0\n", "1\n", None, false),
            case(
                "ipv6.disable=1",
                "",
                "",
                Some(io::ErrorKind::NotFound),
                false,
            ),
            case(
                "unreadable keeps ipv6",
                "",
                "0\n",
                Some(io::ErrorKind::PermissionDenied),
                true,
            ),
        ] {
            let got = available_from(|name| {
                if name == DISABLE_IPV6_FILES[0] {
                    return match c.all_err {
                        Some(kind) => Err(io::Error::from(kind)),
                        None => Ok(c.all.into()),
                    };
                }
                assert_eq!(name, DISABLE_IPV6_FILES[1], "unexpected read");
                if c.all_err == Some(io::ErrorKind::NotFound) {
                    return Err(not_found());
                }
                Ok(c.default.into())
            });
            assert_eq!(got, c.want, "{}", c.name);
        }
    }

    // Go: TestLinuxRoute.
    #[test]
    fn route_needs_a_global_address_and_a_default_route_on_one_interface() {
        const ETH0_GLOBAL: &str = "20010db8000000000000000000000050 02 40 00 80     eth0\n";
        const ETH0_LINK: &str = "fe800000000000000000000000000001 02 40 20 80     eth0\n";
        const TUN_ULA: &str = "fde2ec409312c7fd0000000000000001 05 7e 00 80     tun0\n";
        const LO: &str = "00000000000000000000000000000001 01 80 10 80       lo\n";
        // ::/0 via fe80::1 on eth0, up+gateway.
        const DEFAULT_ETH0: &str = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003     eth0\n";
        // ::/0 in the TUN's own table.
        const DEFAULT_TUN: &str = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 00000400 00000001 00000000 00000001     tun0\n";
        // The kernel's reject ::/0 on lo (no route).
        const REJECT_LO: &str = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo\n";
        // A default route that is a reject route on eth0.
        const REJECT_ETH0: &str = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00000201     eth0\n";
        let cases: [(&str, String, String, bool); 5] = [
            (
                "global address and default route",
                [LO, ETH0_LINK, ETH0_GLOBAL].concat(),
                [REJECT_LO, DEFAULT_ETH0].concat(),
                true,
            ),
            (
                "no global address",
                [LO, ETH0_LINK].concat(),
                [REJECT_LO, DEFAULT_ETH0].concat(),
                false,
            ),
            (
                "global address, no default route",
                [LO, ETH0_GLOBAL].concat(),
                REJECT_LO.into(),
                false,
            ),
            (
                "only the TUN has a default route",
                [LO, ETH0_GLOBAL, TUN_ULA].concat(),
                [REJECT_LO, DEFAULT_TUN].concat(),
                false,
            ),
            (
                "reject default route",
                ETH0_GLOBAL.into(),
                REJECT_ETH0.into(),
                false,
            ),
        ];
        for (name, addrs, routes, want) in cases {
            let got = route_from(|file| match file {
                IF_INET6_FILE => Ok(addrs.clone().into_bytes()),
                IPV6_ROUTE_FILE => Ok(routes.clone().into_bytes()),
                other => panic!("unexpected read {other}"),
            });
            assert_eq!(got, Ok(want), "{name}");
        }
        // Unreadable: an error, which the caller takes as "IPv6 as before".
        assert!(route_from(|_| Err(not_found())).is_err());
    }
}
