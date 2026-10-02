//! IP addresses and prefixes with Go `net/netip` semantics, which the
//! profile contract was written against: an IPv6 zone is accepted, and an
//! IPv4-mapped IPv6 address stays IPv6 (it is not unmapped).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// `netip.ParseAddr`: an IPv4 or IPv6 literal; IPv6 may carry a `%zone`.
pub(crate) fn parse_addr(value: &str) -> Option<IpAddr> {
    if let Ok(ip) = value.parse::<Ipv4Addr>() {
        return Some(IpAddr::V4(ip));
    }
    let (address, zone) = match value.split_once('%') {
        Some((address, zone)) => (address, Some(zone)),
        None => (value, None),
    };
    if zone.is_some_and(str::is_empty) {
        return None;
    }
    address.parse::<Ipv6Addr>().ok().map(IpAddr::V6)
}

/// A prefix with Go's semantics; compared after masking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Prefix {
    addr: IpAddr,
    bits: u8,
}

impl Prefix {
    pub(crate) fn masked(self) -> Self {
        let addr = match self.addr {
            IpAddr::V4(ip) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.bits)).unwrap_or(0);
                IpAddr::V4(Ipv4Addr::from(u32::from(ip) & mask))
            }
            IpAddr::V6(ip) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.bits))
                    .unwrap_or(0);
                IpAddr::V6(Ipv6Addr::from(u128::from(ip) & mask))
            }
        };
        Self {
            addr,
            bits: self.bits,
        }
    }

    pub(crate) fn contains(&self, ip: &IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {
                Self {
                    addr: *ip,
                    bits: self.bits,
                }
                .masked()
                .addr
                    == self.masked().addr
            }
            _ => false,
        }
    }
}

/// `netip.ParsePrefix`: `address/bits`, no zone, bits in decimal without
/// leading zeros and within the family's length.
pub(crate) fn parse_prefix(value: &str) -> Option<Prefix> {
    let (address, bits) = value.split_once('/')?;
    if address.contains('%')
        || bits.is_empty()
        || !bits.bytes().all(|b| b.is_ascii_digit())
        || (bits.len() > 1 && bits.starts_with('0'))
    {
        return None;
    }
    let addr = parse_addr(address)?;
    let bits: u8 = bits.parse().ok()?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    (bits <= max).then_some(Prefix { addr, bits })
}

fn prefix(value: &str) -> Prefix {
    parse_prefix(value).expect("valid built-in prefix")
}

/// The entry-IP rule: global unicast, not private, loopback, link-local,
/// multicast or unspecified, and outside the reserved ranges.
pub(crate) fn is_public_unicast(ip: &IpAddr) -> bool {
    if !is_global_unicast(ip) || is_private(ip) {
        return false;
    }
    const RESERVED: [&str; 17] = [
        "0.0.0.0/8",
        "100.64.0.0/10",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "192.0.0.0/24",
        "192.0.2.0/24",
        "198.18.0.0/15",
        "198.51.100.0/24",
        "203.0.113.0/24",
        "224.0.0.0/4",
        "240.0.0.0/4",
        "::/128",
        "::1/128",
        "fc00::/7",
        "fe80::/10",
        "ff00::/8",
        "2001:db8::/32",
    ];
    !RESERVED.iter().any(|p| prefix(p).contains(ip))
}

/// `netip.Addr.IsGlobalUnicast`.
fn is_global_unicast(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !v4.is_unspecified()
                && !v4.is_broadcast()
                && !v4.is_loopback()
                && !v4.is_multicast()
                && !v4.is_link_local()
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            !v6.is_unspecified()
                && !v6.is_loopback()
                && (first & 0xff00) != 0xff00
                && (first & 0xffc0) != 0xfe80
        }
    }
}

/// `netip.Addr.IsPrivate`: RFC 1918 and fc00::/7 (no unmapping).
fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_ips_follow_go() {
        let public = |s: &str| parse_addr(s).is_some_and(|ip| is_public_unicast(&ip));
        assert!(public("8.8.8.8"));
        assert!(!public("192.168.1.10"));
        assert!(!public("203.0.113.5"));
        assert!(!public("100.64.0.1"));
        assert!(!public("255.255.255.255"));
        assert!(public("2606:4700::1111"));
        assert!(!public("2001:db8::1"));
        assert!(!public("fe80::1%en0"));
        // Go does not unmap: an IPv4-mapped private address is "public".
        assert!(public("::ffff:10.0.0.1"));
    }

    #[test]
    fn prefixes_follow_go() {
        assert!(parse_prefix("10.0.0.0/8").is_some());
        assert!(parse_prefix("10.0.0.0/33").is_none());
        assert!(parse_prefix("10.0.0.0/08").is_none());
        assert!(parse_prefix("fe80::/10").is_some());
        assert!(parse_prefix("fe80::%en0/10").is_none());
        assert_eq!(
            parse_prefix("10.1.2.3/8").unwrap().masked(),
            parse_prefix("10.0.0.0/8").unwrap().masked()
        );
    }
}
