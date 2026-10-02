//! Enhanced (TUN) mode: the port of Go's internal/config/tundns.go. A TUN
//! sees raw IP packets, so it has to supply what the local proxies get from
//! HTTP CONNECT and SOCKS: the domain of each connection, and DNS answers
//! the OS resolver can trust. Real addresses, sniffing, and the domain
//! handed to the node; no fake-ip.
//!
//! - The leading route rules sniff every TUN connection, hijack DNS (sniffed
//!   protocol dns, or port 53), reject the tunnel's own networks and the
//!   fake-ip range without a known domain, and send private destinations
//!   direct (the client baseline, in every routing mode).
//! - Go's domain-destination wrapper is sail's `override_destination`: one
//!   route-options rule for the TUN inbound makes every proxy dial take the
//!   connection's domain (sniffed, else reverse-mapped) while the rules
//!   still match the address. Direct dials keep the address.
//! - DNS rules mirror the route rules: a domain routed direct resolves
//!   through dns-local, one routed to a proxy through dns-remote (DoT to
//!   1.1.1.1 through the selected node, then 8.8.8.8, then 9.9.9.9, in
//!   order), a rejected domain is refused; dns.final follows route.final.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};

use serde_json::{json, Map, Value};

use super::{failed, Builder, DIRECT_TAG, PRIVATE_PREFIXES, SELECTED_TAG};
use crate::error::Error;
use crate::profile::{normalize_domain, Profile};

pub(crate) const TUN_INBOUND_TAG: &str = "tun";
pub(crate) const DNS_LOCAL_TAG: &str = "dns-local";
pub(crate) const DNS_REMOTE_TAG: &str = "dns-remote";
/// dns-remote's DoT servers, asked in this order through the selected node.
/// Public resolvers outside mainland China only: they resolve the domains
/// routed to a proxy.
pub(crate) const REMOTE_DNS_SERVERS: &[&str] = &["1.1.1.1", "8.8.8.8", "9.9.9.9"];
/// The whole query budget (Go's DNS guard).
const DNS_TIMEOUT: &str = "8s";
/// The benchmark range fake-ip resolvers answer from.
const FAKE_IP_RANGE: &str = "198.18.0.0/15";
/// A domain name but not an IP literal (the HTTP sniffer copies a Host
/// header that is an address as it is).
const KNOWN_DOMAIN_REGEX: &str = "^[^:]*[^0-9.:][^:]*$";

/// The tunnel's addresses: fixed, rarely used values (sing-tun's defaults
/// are every other sing-box client's, and two TUNs with one address fail).
/// The desktop service hard-codes the same.
const TUN_INET4_ADDRESS: &str = "10.60.159.89/30";
const TUN_INET6_ADDRESS: &str = "fde2:ec40:9312:c7fd::1/126";
const TUN_PREFIXES: &[&str] = &["10.60.159.88/30", "fde2:ec40:9312:c7fd::/126"];
/// The tunnel networks before 0.5.7; a host may still list them as DNS.
const TUN_LEGACY_PREFIXES: &[&str] = &["172.19.0.0/30", "fdfe:dcba:9876::/126"];
/// Never into the desktop tunnel: multicast, broadcast and link-local
/// traffic stays on the LAN.
const TUN_ROUTE_EXCLUDED: &[&str] = &[
    "224.0.0.0/4",
    "255.255.255.255/32",
    "169.254.0.0/16",
    "fe80::/10",
    "ff00::/8",
];
/// Our own Linux policy-routing table and rule range, apart from sing-tun's
/// defaults (shared by mihomo, Clash Verge, ...), Tailscale and wg-quick.
const IPROUTE2_TABLE_INDEX: u32 = 2091;
const IPROUTE2_RULE_INDEX: u32 = 9091;

/// The TUN inbound and its DNS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tun {
    /// Desktop: the core owns the default route (auto_route, strict_route,
    /// excluded ingresses). Mobile hosts build the tunnel themselves and
    /// stay IPv4-only.
    pub desktop: bool,
    /// The tunnel's IPv6 address (desktop); off on hosts with IPv6 disabled.
    pub ipv6: bool,
    pub local_dns: LocalDns,
}

/// Where dns-local asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalDns {
    /// sail's `local` server: the system's resolvers, asked from the
    /// physical interface.
    System,
    /// These, in order (the host's override, or what the core's dns-local
    /// read from the default interface; a change is a reload).
    Servers(Vec<SocketAddr>),
}

/// Host-supplied physical resolvers: an IP, IP:port or [IPv6]:port each
/// (port 53 by default), in order, less those inside the tunnel (a stale
/// system entry: asking it would loop). Go's config.LocalDNSServers.
pub(crate) fn local_dns_servers(entries: &[String]) -> Result<Vec<SocketAddr>, String> {
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.trim();
        let address = match entry.parse::<SocketAddr>() {
            Ok(address) => address,
            Err(_) => match entry.parse::<IpAddr>() {
                Ok(ip) => SocketAddr::new(ip, 53),
                Err(_) => {
                    return Err(format!(
                        "invalid local DNS server {entry:?}: want an IP, IP:port or [IPv6]:port"
                    ))
                }
            },
        };
        if address.port() == 0 || address.ip().is_unspecified() {
            return Err(format!("invalid local DNS server {entry:?}"));
        }
        let ip = match address.ip() {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
            ip => ip,
        };
        if !in_tunnel(ip) {
            out.push(SocketAddr::new(ip, address.port()));
        }
    }
    Ok(out)
}

fn in_tunnel(ip: IpAddr) -> bool {
    TUN_PREFIXES
        .iter()
        .chain(TUN_LEGACY_PREFIXES)
        .any(|prefix| contains(prefix, ip))
}

fn contains(prefix: &str, ip: IpAddr) -> bool {
    let (net, bits) = prefix.split_once('/').expect("a prefix");
    let bits: u32 = bits.parse().expect("prefix bits");
    match (net.parse::<IpAddr>().expect("a prefix"), ip) {
        (IpAddr::V4(net), IpAddr::V4(ip)) => {
            let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
            u32::from(net) & mask == u32::from(ip) & mask
        }
        (IpAddr::V6(net), IpAddr::V6(ip)) => {
            let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
            u128::from(net) & mask == u128::from(ip) & mask
        }
        _ => false,
    }
}

impl Builder {
    /// The rules that precede every other, then the ingress bypass.
    pub(super) fn tun_rules(&mut self, profile: &Profile) {
        self.rules.extend([
            json!({ "inbound": [TUN_INBOUND_TAG], "action": "sniff" }),
            json!({ "inbound": [TUN_INBOUND_TAG], "action": "route-options", "override_destination": true }),
            json!({ "inbound": [TUN_INBOUND_TAG], "protocol": ["dns"], "action": "hijack-dns" }),
            json!({ "inbound": [TUN_INBOUND_TAG], "port": [53], "action": "hijack-dns" }),
            // Nothing exists there: reject at once rather than send it out
            // the physical interface to hang.
            json!({ "inbound": [TUN_INBOUND_TAG], "ip_cidr": TUN_PREFIXES, "action": "reject", "method": "default" }),
            // A LAN fake-ip answer no node can reach, unless a domain is
            // known: proxying it would hang for the whole connect timeout.
            json!({
                "type": "logical",
                "mode": "and",
                "rules": [
                    { "inbound": [TUN_INBOUND_TAG], "ip_cidr": [FAKE_IP_RANGE] },
                    { "domain_regex": [KNOWN_DOMAIN_REGEX], "invert": true },
                ],
                "action": "reject",
                "method": "default",
            }),
        ]);
        self.ensure_direct();
        self.rules.push(json!({
            "inbound": [TUN_INBOUND_TAG],
            "ip_cidr": PRIVATE_PREFIXES,
            "action": "route",
            "outbound": DIRECT_TAG,
        }));
        // Every ingress (primary and backups) bypasses the tunnel, or failing
        // over to a backup would route its handshake back into the TUN.
        let (mut domains, mut ips) = (HashSet::new(), HashSet::new());
        for ingress in profile.nodes.iter().flat_map(|n| &n.ingresses) {
            if let Some(domain) = normalize_domain(&ingress.endpoint.domain) {
                if domains.insert(domain.clone()) {
                    self.rules.push(
                        json!({ "domain": [domain], "action": "route", "outbound": DIRECT_TAG }),
                    );
                }
            }
            if let Some(ip) = ingress_ip(&ingress.endpoint.ip) {
                if ips.insert(ip) {
                    self.rules.push(json!({ "ip_cidr": [ip.to_string()], "action": "route", "outbound": DIRECT_TAG }));
                }
            }
        }
    }

    /// The TUN inbound.
    pub(super) fn tun_inbound(&mut self, profile: &Profile, tun: &Tun) {
        let mut inbound = json!({
            "type": "tun",
            "tag": TUN_INBOUND_TAG,
            "address": [TUN_INET4_ADDRESS],
        });
        if tun.desktop {
            if tun.ipv6 {
                inbound["address"] = json!([TUN_INET4_ADDRESS, TUN_INET6_ADDRESS]);
            }
            // Every ingress IP stays out at the routing level too, so any
            // process's handshake or probe never loops.
            let mut excluded = Vec::new();
            let mut seen = HashSet::new();
            for ingress in profile.nodes.iter().flat_map(|n| &n.ingresses) {
                if let Some(ip) = ingress_ip(&ingress.endpoint.ip) {
                    if seen.insert(ip) {
                        let bits = if ip.is_ipv4() { 32 } else { 128 };
                        excluded.push((ip, format!("{ip}/{bits}")));
                    }
                }
            }
            let mut excluded: Vec<String> = excluded
                .into_iter()
                .filter(|(ip, _)| tun.ipv6 || ip.is_ipv4())
                .map(|(_, prefix)| prefix)
                .collect();
            excluded.extend(
                TUN_ROUTE_EXCLUDED
                    .iter()
                    .filter(|p| tun.ipv6 || !p.contains(':'))
                    .map(|p| (*p).to_owned()),
            );
            inbound["auto_route"] = true.into();
            inbound["strict_route"] = true.into();
            inbound["route_exclude_address"] = json!(excluded);
            inbound["iproute2_table_index"] = IPROUTE2_TABLE_INDEX.into();
            inbound["iproute2_rule_index"] = IPROUTE2_RULE_INDEX.into();
            self.auto_detect_interface = true;
        }
        self.inbounds.push(inbound);
    }

    /// The DNS section, from the route rules as they stand.
    pub(super) fn tun_dns(
        &self,
        tun: &Tun,
        final_direct: bool,
        dns_rule_sets: &HashSet<String>,
    ) -> Result<Value, Error> {
        let mut servers = Vec::new();
        match &tun.local_dns {
            LocalDns::System => servers.push(json!({ "type": "local", "tag": DNS_LOCAL_TAG })),
            LocalDns::Servers(list) if list.is_empty() => {
                return Err(failed("no local DNS server outside the tunnel"))
            }
            LocalDns::Servers(list) => {
                let mut members = Vec::new();
                for (i, server) in list.iter().enumerate() {
                    let tag = format!("{DNS_LOCAL_TAG}-{i}");
                    servers.push(json!({
                        "type": "udp",
                        "tag": tag,
                        "server": server.ip().to_string(),
                        "server_port": server.port(),
                    }));
                    members.push(tag);
                }
                servers.push(
                    json!({ "type": "sequential", "tag": DNS_LOCAL_TAG, "servers": members }),
                );
            }
        }
        let mut members = Vec::new();
        for server in REMOTE_DNS_SERVERS {
            let tag = format!("{DNS_REMOTE_TAG}-{server}");
            servers.push(
                json!({ "type": "tls", "tag": tag, "server": server, "detour": SELECTED_TAG }),
            );
            members.push(tag);
        }
        servers.push(json!({ "type": "sequential", "tag": DNS_REMOTE_TAG, "servers": members }));
        Ok(json!({
            "servers": servers,
            "rules": self.mirror_dns_rules(dns_rule_sets),
            "final": if final_direct { DNS_LOCAL_TAG } else { DNS_REMOTE_TAG },
            "reverse_mapping": true,
            "timeout": DNS_TIMEOUT,
        }))
    }

    /// Every inbound-independent route rule matching domains becomes a DNS
    /// rule with the same domain matchers, in order, so the first route rule
    /// naming a domain decides where it resolves. Only domain rule sets
    /// count; port, network and CIDR conditions are unknown at resolve time.
    fn mirror_dns_rules(&self, dns_rule_sets: &HashSet<String>) -> Vec<Value> {
        let mut out = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            let Some(rule) = rule.as_object() else {
                continue;
            };
            if self.no_dns_mirror.contains(&i)
                || rule.contains_key("type")
                || rule.contains_key("inbound")
            {
                continue;
            }
            let rule_sets: Vec<&Value> = rule
                .get("rule_set")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|tag| tag.as_str().is_some_and(|t| dns_rule_sets.contains(t)))
                .collect();
            let mut dns = Map::new();
            for key in ["domain", "domain_suffix"] {
                if let Some(value) = rule.get(key) {
                    dns.insert(key.into(), value.clone());
                }
            }
            if !rule_sets.is_empty() {
                dns.insert("rule_set".into(), json!(rule_sets));
            }
            if dns.is_empty() {
                continue;
            }
            match rule.get("action").and_then(Value::as_str) {
                Some("route") => {
                    let direct = rule.get("outbound").and_then(Value::as_str) == Some(DIRECT_TAG);
                    dns.insert("action".into(), "route".into());
                    dns.insert(
                        "server".into(),
                        if direct {
                            DNS_LOCAL_TAG
                        } else {
                            DNS_REMOTE_TAG
                        }
                        .into(),
                    );
                }
                Some("reject") => {
                    dns.insert("action".into(), "reject".into());
                    dns.insert("method".into(), "default".into());
                }
                _ => continue,
            }
            out.push(Value::Object(dns));
        }
        out
    }
}

fn ingress_ip(value: &str) -> Option<IpAddr> {
    match value.parse::<IpAddr>().ok()? {
        IpAddr::V6(v6) => Some(v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4)),
        ip => Some(ip),
    }
}
