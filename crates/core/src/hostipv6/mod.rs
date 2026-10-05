//! Whether the host can give the desktop TUN an IPv6 address
//! ([`available`]), and whether it reaches the IPv6 internet without the
//! tunnel ([`route`]) (Go: internal/hostipv6).
//!
//! A TUN start fails as a whole when the tunnel's IPv6 address cannot be
//! added (Linux under disable_ipv6, Windows with IPv6 disabled), so IPv6 is
//! left out on such hosts. That never leaks: a host that cannot configure
//! IPv6 on a new interface has no IPv6 path around the tunnel either.
//!
//! [`route`] decides the direct IPv6 hand-off (`translate::Tun::
//! no_host_ipv6_route`): read on every apply and start, and again when sail
//! reports the network changed (never while offline).
//!
//! Each platform's reading is a pure function over what the system gives
//! (files, route messages, adapters), compiled everywhere so its tests run
//! on every CI platform; only the system calls are per platform.

#![allow(dead_code)] // the Engine reads it with the TUN instance

use std::net::Ipv6Addr;

pub(crate) mod darwin;
pub(crate) mod linux;
pub(crate) mod windows;

/// Whether IPv6 is enabled on this host. Not cached: an administrator can
/// toggle it between two starts.
pub(crate) fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::available()
    }
    #[cfg(windows)]
    {
        windows::available()
    }
    // macOS cannot switch IPv6 off system-wide; mobile hosts build the
    // tunnel themselves.
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        true
    }
}

/// Whether the host has an IPv6 path of its own: one interface with both a
/// global unicast IPv6 address and an IPv6 default route. The TUN only ever
/// has a ULA, so it never counts. When the state cannot be read: `Err`, and
/// the caller uses IPv6 as before (no hand-off) and logs the error.
pub(crate) fn route() -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        linux::route()
    }
    #[cfg(target_os = "macos")]
    {
        darwin::route()
    }
    #[cfg(windows)]
    {
        windows::route()
    }
    // Where the core cannot tell, IPv6 is used as before.
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Ok(true)
    }
}

/// Global unicast IPv6 (2000::/3), the only kind that reaches the IPv6
/// internet; an IPv4-mapped address is IPv4.
pub(crate) fn global_unicast(a: &Ipv6Addr) -> bool {
    a.to_ipv4_mapped().is_none() && a.octets()[0] & 0xe0 == 0x20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_unicast_is_2000_slash_3() {
        for (a, want) in [
            ("2001:db8::50", true),
            ("3fff::1", true),
            ("fe80::1", false),
            ("fde2:ec40:9312:c7fd::1", false),
            ("::1", false),
            ("::ffff:192.0.2.1", false),
            ("4000::1", false),
        ] {
            assert_eq!(global_unicast(&a.parse().unwrap()), want, "{a}");
        }
    }

    /// What this host says (logged, not asserted: CI hosts differ).
    #[test]
    fn route_on_this_host() {
        println!("available {}, route {:?}", available(), route());
    }
}
