//! macOS: the IPv6 routing table (sysctl NET_RT_DUMP) and the interfaces'
//! addresses (getifaddrs). macOS keeps IPv6 enabled; it cannot be switched
//! off system-wide.

use std::net::Ipv6Addr;

use super::global_unicast;

/// `struct rt_msghdr`: rtm_msglen u16 at 0, rtm_index u16 at 4, rtm_flags
/// i32 at 8, rtm_addrs i32 at 12; the sockaddrs follow the 92 bytes.
const RTM_HEADER_LEN: usize = 92;
const RTF_UP: i32 = 0x1;
const RTF_REJECT: i32 = 0x8;
const RTF_BLACKHOLE: i32 = 0x1000;
const RTA_DST: i32 = 0x1;
const RTA_NETMASK: i32 = 0x4;
const RTAX_MAX: u32 = 8;
const AF_INET6: u8 = 30;

/// One route message, as far as a default route goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteMessage {
    pub index: u16,
    pub flags: i32,
    /// The raw sockaddrs (sa_len, sa_family, ...); None when absent.
    pub dst: Option<Vec<u8>>,
    pub netmask: Option<Vec<u8>>,
}

/// One interface's state, as far as `route` goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Interface {
    pub index: u32,
    pub up: bool,
    pub addrs: Vec<Ipv6Addr>,
}

/// Splits a NET_RT_DUMP buffer into route messages.
pub(crate) fn parse_routes(mut buf: &[u8]) -> Vec<RouteMessage> {
    let mut out = Vec::new();
    while buf.len() >= RTM_HEADER_LEN {
        let len = u16::from_ne_bytes([buf[0], buf[1]]) as usize;
        if len < RTM_HEADER_LEN || len > buf.len() {
            break;
        }
        let msg = &buf[..len];
        let index = u16::from_ne_bytes([msg[4], msg[5]]);
        let flags = i32::from_ne_bytes(msg[8..12].try_into().expect("4 bytes"));
        let addrs = i32::from_ne_bytes(msg[12..16].try_into().expect("4 bytes"));
        let mut sockaddrs = &msg[RTM_HEADER_LEN..];
        let (mut dst, mut netmask) = (None, None);
        for i in 0..RTAX_MAX {
            let bit = 1 << i;
            if addrs & bit == 0 {
                continue;
            }
            let Some(&sa_len) = sockaddrs.first() else {
                break;
            };
            // A sockaddr takes its length rounded up to 4; an empty one, 4.
            let size = if sa_len == 0 {
                4
            } else {
                (sa_len as usize).div_ceil(4) * 4
            };
            let raw = sockaddrs[..(sa_len as usize).min(sockaddrs.len())].to_vec();
            match bit {
                RTA_DST => dst = Some(raw),
                RTA_NETMASK => netmask = Some(raw),
                _ => {}
            }
            sockaddrs = &sockaddrs[size.min(sockaddrs.len())..];
        }
        out.push(RouteMessage {
            index,
            flags,
            dst,
            netmask,
        });
        buf = &buf[len..];
    }
    out
}

/// The interfaces of the usable IPv6 default routes (::/0, up, neither
/// reject nor blackhole). A TUN's split routes (::/1, 8000::/1, 100::/8...)
/// are not default routes.
pub(crate) fn default_route_indexes(messages: &[RouteMessage]) -> Vec<u32> {
    messages
        .iter()
        .filter(|m| m.flags & RTF_UP != 0 && m.flags & (RTF_REJECT | RTF_BLACKHOLE) == 0)
        .filter(|m| m.dst.as_deref().is_some_and(zero_inet6))
        // The kernel omits the netmask of a default route.
        .filter(|m| m.netmask.as_deref().is_none_or(zero_mask))
        .map(|m| m.index as u32)
        .collect()
}

/// An IPv6 sockaddr (sockaddr_in6: the address at 8..24) that is `::`.
fn zero_inet6(sa: &[u8]) -> bool {
    sa.len() >= 24 && sa[1] == AF_INET6 && sa[8..24].iter().all(|&b| b == 0)
}

/// A netmask sockaddr, possibly cut short after its last non-zero byte,
/// with no bits set.
fn zero_mask(sa: &[u8]) -> bool {
    sa.get(8..).is_none_or(|bits| bits.iter().all(|&b| b == 0))
}

/// One interface of a default route is up with a global unicast address.
pub(crate) fn route_from(messages: &[RouteMessage], interfaces: &[Interface]) -> bool {
    default_route_indexes(messages).iter().any(|index| {
        interfaces
            .iter()
            .any(|i| i.index == *index && i.up && i.addrs.iter().any(global_unicast))
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn route() -> Result<bool, String> {
    let messages = parse_routes(&sys::route_dump()?);
    Ok(route_from(&messages, &sys::interfaces()?))
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::CStr;
    use std::io;
    use std::net::Ipv6Addr;

    use super::Interface;

    /// sysctl {CTL_NET, PF_ROUTE, 0, AF_INET6, NET_RT_DUMP, 0}.
    pub(super) fn route_dump() -> Result<Vec<u8>, String> {
        let mut mib = [
            libc::CTL_NET,
            libc::PF_ROUTE,
            0,
            libc::AF_INET6,
            libc::NET_RT_DUMP,
            0,
        ];
        // The table can grow between the size query and the read: retry.
        for _ in 0..3 {
            let mut size = 0usize;
            // SAFETY: a size query (null buffer) on a valid mib.
            let rc = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    mib.len() as u32,
                    std::ptr::null_mut(),
                    &mut size,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if rc != 0 {
                return Err(format!("route dump: {}", io::Error::last_os_error()));
            }
            let mut buf = vec![0u8; size + size / 4];
            let mut len = buf.len();
            // SAFETY: buf holds len bytes; the kernel writes at most len.
            let rc = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    mib.len() as u32,
                    buf.as_mut_ptr().cast(),
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if rc == 0 {
                buf.truncate(len);
                return Ok(buf);
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ENOMEM) {
                return Err(format!("route dump: {err}"));
            }
        }
        Err("route dump: the table kept growing".into())
    }

    /// Every interface with its up flag and IPv6 addresses.
    pub(super) fn interfaces() -> Result<Vec<Interface>, String> {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs fills head; freed below.
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return Err(format!("getifaddrs: {}", io::Error::last_os_error()));
        }
        let mut out: Vec<Interface> = Vec::new();
        let mut cur = head;
        while !cur.is_null() {
            // SAFETY: a node of the list getifaddrs returned, not yet freed.
            let ifa = unsafe { &*cur };
            cur = ifa.ifa_next;
            // SAFETY: ifa_name is a NUL-terminated string.
            let name = unsafe { CStr::from_ptr(ifa.ifa_name) };
            // SAFETY: as above.
            let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
            if index == 0 {
                continue;
            }
            let up = ifa.ifa_flags & libc::IFF_UP as u32 != 0;
            let pos = match out.iter().position(|i| i.index == index) {
                Some(pos) => pos,
                None => {
                    out.push(Interface {
                        index,
                        up,
                        addrs: Vec::new(),
                    });
                    out.len() - 1
                }
            };
            if ifa.ifa_addr.is_null() {
                continue;
            }
            // SAFETY: a non-null sockaddr of the list.
            let family = unsafe { (*ifa.ifa_addr).sa_family };
            if family as i32 == libc::AF_INET6 {
                // SAFETY: an AF_INET6 sockaddr is a sockaddr_in6.
                let sin6 = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                out[pos].addrs.push(Ipv6Addr::from(sin6.sin6_addr.s6_addr));
            }
        }
        // SAFETY: the list getifaddrs returned, freed once.
        unsafe { libc::freeifaddrs(head) };
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inet6(addr: &str) -> Vec<u8> {
        let mut sa = vec![0u8; 28];
        sa[0] = 28;
        sa[1] = AF_INET6;
        sa[8..24].copy_from_slice(&addr.parse::<Ipv6Addr>().unwrap().octets());
        sa
    }

    fn msg(index: u16, flags: i32, dst: &str, mask: Option<&str>) -> RouteMessage {
        RouteMessage {
            index,
            flags,
            dst: Some(inet6(dst)),
            netmask: mask.map(inet6),
        }
    }

    // Go: TestDarwinDefaultRouteIndexes.
    #[test]
    fn default_routes_are_up_zero_routes() {
        const GATEWAY: i32 = 0x2;
        let up = RTF_UP | GATEWAY;
        let got = default_route_indexes(&[
            msg(4, up, "::", None),                       // en0 default, mask omitted
            msg(5, up, "::", Some("::")),                 // explicit ::/0
            msg(9, up, "8000::", Some("8000::")),         // a TUN's 8000::/1
            msg(6, up | RTF_REJECT, "::", None),          // reject
            msg(7, GATEWAY, "::", None),                  // down
            msg(8, up | RTF_BLACKHOLE, "::", Some("::")), // blackhole
        ]);
        assert_eq!(got, vec![4, 5]);
    }

    #[test]
    fn route_needs_the_default_routes_interface_up_with_a_global_address() {
        let routes = [msg(4, RTF_UP, "::", None)];
        let iface = |index, up, addrs: &[&str]| Interface {
            index,
            up,
            addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
        };
        assert!(route_from(
            &routes,
            &[iface(4, true, &["fe80::1", "2001:db8::50"])]
        ));
        assert!(!route_from(&routes, &[iface(4, true, &["fe80::1"])]));
        assert!(!route_from(&routes, &[iface(4, false, &["2001:db8::50"])]));
        // A global address elsewhere is not this route's.
        assert!(!route_from(
            &routes,
            &[
                iface(4, true, &["fe80::1"]),
                iface(5, true, &["2001:db8::50"])
            ]
        ));
        // The TUN's ULA never counts.
        assert!(!route_from(
            &routes,
            &[iface(4, true, &["fde2:ec40:9312:c7fd::1"])]
        ));
    }

    /// The buffer a dump gives: header, then the sockaddrs the address bits
    /// name, each rounded up to 4; a short netmask after its last byte.
    #[test]
    fn parses_a_route_dump() {
        fn message(index: u16, flags: i32, addrs: i32, sockaddrs: &[&[u8]]) -> Vec<u8> {
            let mut m = vec![0u8; RTM_HEADER_LEN];
            m[2] = 5; // RTM_VERSION
            m[3] = 4; // RTM_GET
            m[4..6].copy_from_slice(&index.to_ne_bytes());
            m[8..12].copy_from_slice(&flags.to_ne_bytes());
            m[12..16].copy_from_slice(&addrs.to_ne_bytes());
            for sa in sockaddrs {
                m.extend_from_slice(sa);
                m.resize(m.len().div_ceil(4) * 4, 0);
            }
            let len = m.len() as u16;
            m[0..2].copy_from_slice(&len.to_ne_bytes());
            m
        }
        const RTA_GATEWAY: i32 = 0x2;
        let gateway = inet6("fe80::1");
        // 8000::/1: the netmask cut short after its one non-zero byte.
        let short_mask: &[u8] = &[9, 0, 0, 0, 0, 0, 0, 0, 0x80];
        // ::/0 with an empty netmask.
        let empty_mask: &[u8] = &[0];
        let mut dump = message(4, RTF_UP, RTA_DST | RTA_GATEWAY, &[&inet6("::"), &gateway]);
        dump.extend(message(
            9,
            RTF_UP,
            RTA_DST | RTA_GATEWAY | RTA_NETMASK,
            &[&inet6("8000::"), &gateway, short_mask],
        ));
        dump.extend(message(
            5,
            RTF_UP,
            RTA_DST | RTA_GATEWAY | RTA_NETMASK,
            &[&inet6("::"), &gateway, empty_mask],
        ));
        let messages = parse_routes(&dump);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].netmask, None);
        assert_eq!(messages[1].netmask.as_deref(), Some(short_mask));
        assert_eq!(default_route_indexes(&messages), vec![4, 5]);
        // A truncated buffer stops cleanly.
        assert_eq!(parse_routes(&dump[..dump.len() - 1]).len(), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn header_length_is_libcs() {
        assert_eq!(std::mem::size_of::<libc::rt_msghdr>(), RTM_HEADER_LEN);
    }
}
