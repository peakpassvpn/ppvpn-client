//! Profile → sail configuration (sing-box JSON). The port of Go's
//! internal/config builder, with the three types the Go core plugged into
//! sing-box mapped to what sail has (as in the Go sail prototype):
//!
//! - The local proxy is a stock `mixed` inbound with the node users and the
//!   routed user; one `auth_user` rule per node pins that user to its node,
//!   and a final rule rejects any other user of the inbound.
//! - A multi-ingress node is a `selector` tagged as the node whose default is
//!   a `fallback` group over the ingresses (tag + [`AUTO_SUFFIX`]) and whose
//!   other members are the ingresses themselves: selecting an ingress pins
//!   it, selecting the fallback group returns to automatic failover.
//! - The domain-destination wrapper (TUN) is sail's `override_destination`
//!   (see [`tun`]).
//!
//! D4: a node whose capabilities say no UDP gets its UDP rejected, never
//! re-routed: before every rule routing to it, the same match on UDP is
//! rejected. What a rule cannot know (the selected node, an ingress of a
//! failover group) is the Engine's.

#![allow(dead_code)] // the Engine is wired to it with the runtime

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::net::IpAddr;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{codes, Error};
use crate::profile::{
    normalize_domain, parse_port_range, Ingress, Node, Profile, RoutingAction, RoutingMatch, Tls,
};
use crate::request::RoutingMode;

mod tun;
#[allow(unused_imports)] // the Engine's, once it is wired to the runtime
pub(crate) use tun::{
    interface_name, local_dns_servers, LocalDns, Tun, DNS_LOCAL_TAG, IPROUTE2_RULE_INDEX,
    IPROUTE2_TABLE_INDEX, TUN_INBOUND_TAG,
};

/// The selector over every node, in profile order.
pub(crate) const SELECTED_TAG: &str = "selected";
pub(crate) const DIRECT_TAG: &str = "direct";
pub(crate) const LOCAL_PROXY_INBOUND_TAG: &str = "local-proxy";
pub(crate) const SYSTEM_PROXY_INBOUND_TAG: &str = "system-proxy";
/// Tags the fallback group behind a multi-ingress node.
pub(crate) const AUTO_SUFFIX: &str = "-auto";

/// Loopback only, as Go's localproxy.Listen.
const LOOPBACK: &str = "127.0.0.1";

/// The destinations `ip_is_private` matches (Go profile.PrivatePrefixes).
pub(crate) const PRIVATE_PREFIXES: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "0.0.0.0/8",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "224.0.0.0/4",
    "240.0.0.0/4",
    "255.255.255.255/32",
    "fc00::/7",
    "fe80::/10",
    "ff00::/8",
    "::1/128",
];

/// The failover group's health check (Go internal/failover).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HealthCheck {
    pub url: String,
    pub interval: String,
    pub timeout: String,
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            url: "http://www.gstatic.com/generate_204".into(),
            interval: "15s".into(),
            timeout: "5s".into(),
        }
    }
}

/// The shared local proxy: one port, one password, a username per node
/// (`<prefix>-<node id>`) and the routed user (`<prefix>`). Its `Debug`
/// leaves the password out.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LocalProxy {
    /// The address it listens on (`LocalProxyConfig::listen`, 127.0.0.1 by
    /// default).
    pub listen: String,
    pub port: u16,
    pub prefix: String,
    pub password: String,
}

impl std::fmt::Debug for LocalProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProxy")
            .field("listen", &self.listen)
            .field("port", &self.port)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl LocalProxy {
    pub(crate) fn username(&self, node_id: &str) -> String {
        if node_id.is_empty() {
            self.prefix.clone()
        } else {
            format!("{}-{}", self.prefix, node_id)
        }
    }
}

/// A verified local copy of a profile rule set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleSetFile {
    /// Absolute path of the binary (`.srs`) file.
    pub path: String,
    /// The set matches domains and no destination CIDRs: only such sets are
    /// mirrored into DNS rules (a DNS rule whose set carries CIDRs would
    /// resolve every name through its server to test the answer).
    pub mirror_dns: bool,
}

/// The device-local inputs of a translation.
#[derive(Debug, Clone, Default)]
pub(crate) struct Options {
    pub mode: RoutingMode,
    /// The host's selection; the profile's default node when None.
    pub selected_node_id: Option<String>,
    /// node id → pinned endpoint_key (validated by the request).
    pub pins: HashMap<String, String>,
    pub local_proxy: Option<LocalProxy>,
    pub system_proxy_port: Option<u16>,
    /// Rule set id → its local copy; a rule set without one is unavailable.
    pub rule_sets: HashMap<String, RuleSetFile>,
    pub health_check: HealthCheck,
    /// Enhanced mode.
    pub tun: Option<Tun>,
    /// sail's log level (`info`, `debug`); no log section when empty.
    pub log_level: String,
}

/// The configuration and how its tags map back to the profile. Its `Debug`
/// leaves the configuration out: it carries the nodes' and the local
/// proxy's credentials.
#[derive(Clone, PartialEq)]
pub(crate) struct Translation {
    pub json: String,
    /// node id → the outbound that is the node (the ingress itself for a
    /// single-ingress node, else the node's selector).
    pub node_tags: BTreeMap<String, String>,
    /// Every node and ingress outbound tag → its node id.
    pub outbound_nodes: BTreeMap<String, String>,
    /// Every outbound that is one ingress → its endpoint_key.
    pub ingress_keys: BTreeMap<String, String>,
    /// Multi-ingress node tag → its fallback group tag.
    pub groups: BTreeMap<String, String>,
    /// Multi-ingress node tag → its ingress tags, in failover order.
    pub members: BTreeMap<String, Vec<String>>,
    /// Direct hands a global IPv6 destination its domain and resolves to
    /// IPv4 only (a host without an IPv6 path; see [`tun`]).
    pub direct_ipv6_hand_off: bool,
}

impl std::fmt::Debug for Translation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Translation")
            .field("json", &format_args!("<{} bytes>", self.json.len()))
            .field("node_tags", &self.node_tags)
            .field("outbound_nodes", &self.outbound_nodes)
            .field("ingress_keys", &self.ingress_keys)
            .field("groups", &self.groups)
            .field("members", &self.members)
            .field("direct_ipv6_hand_off", &self.direct_ipv6_hand_off)
            .finish()
    }
}

/// Translates a validated profile. Errors here are the translation's own
/// (a profile that passed validation should never fail): CORE_OPERATION_FAILED.
pub(crate) fn translate(profile: &Profile, options: &Options) -> Result<Translation, Error> {
    let mut b = Builder {
        translation: Translation {
            json: String::new(),
            node_tags: BTreeMap::new(),
            outbound_nodes: BTreeMap::new(),
            ingress_keys: BTreeMap::new(),
            groups: BTreeMap::new(),
            members: BTreeMap::new(),
            direct_ipv6_hand_off: false,
        },
        outbounds: Vec::new(),
        inbounds: Vec::new(),
        rules: Vec::new(),
        rule_sets: Vec::new(),
        has_direct: false,
        direct_resolver: None,
        auto_detect_interface: false,
        no_dns_mirror: HashSet::new(),
        udp_off: profile
            .nodes
            .iter()
            .filter(|n| !n.capabilities.udp)
            .map(|n| node_tag(&n.id))
            .collect(),
    };
    let mut node_outbounds = Vec::new();
    for node in &profile.nodes {
        let pin = options.pins.get(&node.id).map(String::as_str);
        node_outbounds.extend(
            b.node(node, pin, &options.health_check)
                .map_err(|e| failed(format!("node {:?}: {e}", node.id)))?,
        );
    }
    let selected = options
        .selected_node_id
        .as_deref()
        .unwrap_or(&profile.selection.default_node_id);
    let default = b
        .translation
        .node_tags
        .get(selected)
        .ok_or_else(|| failed(format!("selected node {selected:?} does not exist")))?
        .clone();
    let members: Vec<&String> = profile
        .nodes
        .iter()
        .map(|n| &b.translation.node_tags[&n.id])
        .collect();
    b.outbounds.push(json!({
        "type": "selector",
        "tag": SELECTED_TAG,
        "outbounds": members,
        "default": default,
        "interrupt_exist_connections": false,
    }));
    b.outbounds.extend(node_outbounds);

    if let Some(tun) = &options.tun {
        b.tun_rules(profile, tun);
    }
    if let Some(local_proxy) = &options.local_proxy {
        b.local_proxy(profile, local_proxy)?;
    }
    if let Some(port) = options.system_proxy_port {
        b.inbounds.push(json!({
            "type": "mixed",
            "tag": SYSTEM_PROXY_INBOUND_TAG,
            "listen": LOOPBACK,
            "listen_port": port,
        }));
    }
    if let Some(tun) = &options.tun {
        b.tun_inbound(profile, tun);
    }
    let routing = effective_routing(profile, options.mode);
    let final_tag = b.routing(&routing, &options.rule_sets)?;
    let dns = match &options.tun {
        Some(tun) => {
            let dns_rule_sets: HashSet<String> = options
                .rule_sets
                .iter()
                .filter(|(_, file)| file.mirror_dns)
                .map(|(id, _)| rule_set_tag(id))
                .collect();
            let final_direct = final_tag.as_deref() == Some(DIRECT_TAG);
            Some(b.tun_dns(tun, final_direct, &dns_rule_sets)?)
        }
        None => None,
    };

    if let Some(resolver) = b.direct_resolver.take() {
        for outbound in &mut b.outbounds {
            if outbound["tag"] == DIRECT_TAG {
                outbound["domain_resolver"] = resolver.clone();
            }
        }
    }
    let mut root = Map::new();
    if !options.log_level.is_empty() {
        root.insert(
            "log".into(),
            json!({ "level": options.log_level, "timestamp": true }),
        );
    }
    if let Some(dns) = dns {
        root.insert("dns".into(), dns);
    }
    if !b.inbounds.is_empty() {
        root.insert(
            "inbounds".into(),
            Value::Array(std::mem::take(&mut b.inbounds)),
        );
    }
    root.insert(
        "outbounds".into(),
        Value::Array(std::mem::take(&mut b.outbounds)),
    );
    let mut route = Map::new();
    if !b.rules.is_empty() {
        route.insert("rules".into(), Value::Array(std::mem::take(&mut b.rules)));
    }
    if !b.rule_sets.is_empty() {
        route.insert(
            "rule_set".into(),
            Value::Array(std::mem::take(&mut b.rule_sets)),
        );
    }
    if let Some(final_tag) = final_tag {
        route.insert("final".into(), Value::String(final_tag));
    }
    if b.auto_detect_interface {
        // The core's own sockets (handshakes, direct traffic) bind to the
        // physical interface so they cannot loop into the tunnel.
        route.insert("auto_detect_interface".into(), true.into());
    }
    if options.tun.is_some() {
        route.insert(
            "default_domain_resolver".into(),
            json!({ "server": tun::DNS_LOCAL_TAG }),
        );
    }
    root.insert("route".into(), Value::Object(route));
    let mut translation = b.translation;
    translation.json =
        serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| failed(e.to_string()))?;
    Ok(translation)
}

/// Runs sail's own check on a translation: it must load with no error and
/// no warning (a warning is a field sail would ignore or degrade).
pub(crate) fn check(json: &str) -> Result<(), Error> {
    let config = sail::embed::Config::Json(json.to_owned());
    let warnings = sail::embed::check(&config, &sail::embed::Options::new())
        .map_err(|e| failed(format!("sail check: {e}")))?;
    if warnings.is_empty() {
        Ok(())
    } else {
        Err(failed(format!(
            "sail check warnings: {}",
            warnings.join("; ")
        )))
    }
}

fn failed(message: impl Into<String>) -> Error {
    Error::new(
        codes::CORE_OPERATION_FAILED,
        false,
        format!("translate: {}", message.into()),
    )
}

/// The routing the configuration is built from: in the global mode only the
/// baseline rules (and their rule sets), then the selected node.
pub(crate) fn effective_routing(profile: &Profile, mode: RoutingMode) -> crate::profile::Routing {
    let mut routing = profile.routing.clone();
    if mode == RoutingMode::Global {
        routing.rules.retain(|rule| rule.baseline);
        let referenced: HashSet<&str> = routing
            .rules
            .iter()
            .flat_map(|rule| rule.matcher.rule_set_ids.iter().map(String::as_str))
            .collect();
        let rule_sets = routing
            .rule_sets
            .iter()
            .filter(|set| referenced.contains(set.id.as_str()))
            .cloned()
            .collect();
        routing.rule_sets = rule_sets;
        routing.final_action = RoutingAction {
            kind: "proxy".into(),
            target: "selected".into(),
            node_id: String::new(),
        };
    }
    routing
}

struct Builder {
    translation: Translation,
    outbounds: Vec<Value>,
    inbounds: Vec<Value>,
    rules: Vec<Value>,
    rule_sets: Vec<Value>,
    has_direct: bool,
    /// direct's own resolver (the IPv6 hand-off).
    direct_resolver: Option<Value>,
    auto_detect_interface: bool,
    /// Rules made for D4: never mirrored into DNS.
    no_dns_mirror: HashSet<usize>,
    /// Node tags whose node carries no UDP.
    udp_off: HashSet<String>,
}

impl Builder {
    /// One node: a single ingress is the node's outbound; several are a
    /// selector over a fallback group and the ingresses.
    fn node(
        &mut self,
        node: &Node,
        pin: Option<&str>,
        check: &HealthCheck,
    ) -> Result<Vec<Value>, String> {
        let tag = node_tag(&node.id);
        let t = &mut self.translation;
        t.node_tags.insert(node.id.clone(), tag.clone());
        t.outbound_nodes.insert(tag.clone(), node.id.clone());
        if let [ingress] = node.ingresses.as_slice() {
            t.ingress_keys
                .insert(tag.clone(), ingress.endpoint_key.clone());
            return Ok(vec![outbound(ingress, &tag)?]);
        }
        let auto = format!("{tag}{AUTO_SUFFIX}");
        let mut members = Vec::with_capacity(node.ingresses.len());
        let mut outbounds = Vec::with_capacity(node.ingresses.len() + 2);
        let mut default = auto.clone();
        for (i, ingress) in node.ingresses.iter().enumerate() {
            let member = ingress_tag(&node.id, &ingress.endpoint_key);
            if t.outbound_nodes.contains_key(&member) {
                return Err(format!("ingress {i}: outbound tag collision"));
            }
            if pin == Some(ingress.endpoint_key.as_str()) {
                default = member.clone();
            }
            outbounds.push(outbound(ingress, &member).map_err(|e| format!("ingress {i}: {e}"))?);
            t.outbound_nodes.insert(member.clone(), node.id.clone());
            t.ingress_keys
                .insert(member.clone(), ingress.endpoint_key.clone());
            members.push(member);
        }
        let mut selector_members = vec![auto.clone()];
        selector_members.extend(members.iter().cloned());
        let group = [
            json!({
                "type": "selector",
                "tag": tag,
                "outbounds": selector_members,
                "default": default,
                "interrupt_exist_connections": false,
            }),
            json!({
                "type": "fallback",
                "tag": auto,
                "outbounds": members,
                "url": check.url,
                "interval": check.interval,
                "timeout": check.timeout,
            }),
        ];
        t.groups.insert(tag.clone(), auto);
        t.members.insert(tag, members);
        Ok(group.into_iter().chain(outbounds).collect())
    }

    fn local_proxy(&mut self, profile: &Profile, proxy: &LocalProxy) -> Result<(), Error> {
        if proxy.listen.parse::<std::net::IpAddr>().is_err()
            || proxy.port == 0
            || proxy.prefix.is_empty()
            || proxy.password.is_empty()
        {
            return Err(failed("invalid local proxy"));
        }
        let mut users = Vec::with_capacity(profile.nodes.len() + 1);
        for node in &profile.nodes {
            let username = proxy.username(&node.id);
            let mut rule = Map::new();
            rule.insert("inbound".into(), json!([LOCAL_PROXY_INBOUND_TAG]));
            rule.insert("auth_user".into(), json!([username]));
            let target = self.translation.node_tags[&node.id].clone();
            self.reject_udp_before(&rule, &target);
            rule.insert("action".into(), "route".into());
            rule.insert("outbound".into(), target.into());
            self.rules.push(Value::Object(rule));
            users.push(json!({ "username": username, "password": proxy.password }));
        }
        let routed = proxy.username("");
        users.push(json!({ "username": routed, "password": proxy.password }));
        // Any other user of the inbound: never fall through to the profile.
        self.rules.push(json!({
            "type": "logical",
            "mode": "and",
            "rules": [
                { "inbound": [LOCAL_PROXY_INBOUND_TAG] },
                { "auth_user": [routed], "invert": true },
            ],
            "action": "reject",
            "method": "default",
        }));
        self.inbounds.push(json!({
            "type": "mixed",
            "tag": LOCAL_PROXY_INBOUND_TAG,
            "listen": proxy.listen,
            "listen_port": proxy.port,
            "users": users,
        }));
        Ok(())
    }

    /// The profile rules, its rule sets and the final action; returns
    /// `route.final`. A rule set without a local copy is unavailable and
    /// dropped from every rule naming it; a rule that loses all of its
    /// address matchers that way is dropped, so it never widens to "match
    /// everything".
    fn routing(
        &mut self,
        routing: &crate::profile::Routing,
        files: &HashMap<String, RuleSetFile>,
    ) -> Result<Option<String>, Error> {
        let mut used = HashSet::new();
        for rule in &routing.rules {
            let tags: Vec<String> = rule
                .matcher
                .rule_set_ids
                .iter()
                .filter(|id| files.contains_key(*id))
                .map(|id| {
                    used.insert(id.as_str());
                    rule_set_tag(id)
                })
                .collect();
            if !rule.matcher.rule_set_ids.is_empty()
                && tags.is_empty()
                && !has_address_match(&rule.matcher)
            {
                continue;
            }
            let mut out = rule_match(&rule.matcher)
                .map_err(|e| failed(format!("rule {:?}: {e}", rule.id)))?;
            if !tags.is_empty() {
                out.insert("rule_set".into(), json!(tags));
            }
            let matcher = out.clone();
            self.action(&mut out, &rule.action)
                .map_err(|e| failed(format!("rule {:?}: {e}", rule.id)))?;
            if let Some(target) = out.get("outbound").and_then(Value::as_str) {
                let target = target.to_owned();
                self.reject_udp_before(&matcher, &target);
            }
            self.rules.push(Value::Object(out));
        }
        for set in &routing.rule_sets {
            if used.contains(set.id.as_str()) {
                self.rule_sets.push(json!({
                    "type": "local",
                    "tag": rule_set_tag(&set.id),
                    "format": "binary",
                    "path": files[&set.id].path,
                }));
            }
        }
        match routing.final_action.kind.as_str() {
            "direct" => {
                self.ensure_direct();
                Ok(Some(DIRECT_TAG.into()))
            }
            "proxy" => {
                let target = self
                    .proxy_target(&routing.final_action)
                    .map_err(|e| failed(format!("final: {e}")))?;
                self.reject_udp_before(&Map::new(), &target);
                Ok(Some(target))
            }
            "reject" => {
                // sail refuses a rule without conditions; both networks is
                // the same catch-all.
                self.rules.push(json!({
                    "network": ["tcp", "udp"],
                    "action": "reject",
                    "method": "default",
                }));
                Ok(None)
            }
            other => Err(failed(format!("unsupported final action {other:?}"))),
        }
    }

    fn action(
        &mut self,
        out: &mut Map<String, Value>,
        action: &RoutingAction,
    ) -> Result<(), String> {
        match action.kind.as_str() {
            "direct" => {
                self.ensure_direct();
                out.insert("action".into(), "route".into());
                out.insert("outbound".into(), DIRECT_TAG.into());
            }
            "proxy" => {
                let target = self.proxy_target(action)?;
                out.insert("action".into(), "route".into());
                out.insert("outbound".into(), target.into());
            }
            "reject" => {
                out.insert("action".into(), "reject".into());
                out.insert("method".into(), "default".into());
            }
            other => return Err(format!("unsupported action {other:?}")),
        }
        Ok(())
    }

    fn proxy_target(&self, action: &RoutingAction) -> Result<String, String> {
        match action.target.as_str() {
            "selected" => Ok(SELECTED_TAG.into()),
            "node" => self
                .translation
                .node_tags
                .get(&action.node_id)
                .cloned()
                .ok_or_else(|| "fixed proxy node does not exist".into()),
            other => Err(format!("unsupported proxy target {other:?}")),
        }
    }

    /// D4: before a rule with `matcher` routing to `target`, the same match
    /// on UDP is rejected when the target node carries no UDP.
    fn reject_udp_before(&mut self, matcher: &Map<String, Value>, target: &str) {
        if !self.udp_off.contains(target) {
            return;
        }
        let mut rule = matcher.clone();
        match rule.get("network").and_then(Value::as_array) {
            Some(networks) if !networks.iter().any(|n| n == "udp") => return,
            _ => {}
        }
        rule.insert("network".into(), json!(["udp"]));
        rule.insert("action".into(), "reject".into());
        rule.insert("method".into(), "default".into());
        self.no_dns_mirror.insert(self.rules.len());
        self.rules.push(Value::Object(rule));
    }

    fn ensure_direct(&mut self) {
        if !self.has_direct {
            self.has_direct = true;
            self.outbounds
                .push(json!({ "type": "direct", "tag": DIRECT_TAG }));
        }
    }
}

fn has_address_match(m: &RoutingMatch) -> bool {
    !m.domains.is_empty()
        || !m.domain_suffixes.is_empty()
        || !m.ip_cidrs.is_empty()
        || m.ip_is_private
}

/// The matchers of a profile rule. A suffix matches the domain itself too
/// (`.example.com` alone would not match `example.com`).
fn rule_match(m: &RoutingMatch) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    let mut domains = Vec::new();
    for value in &m.domains {
        domains.push(normalize_domain(value).ok_or("invalid exact domain")?);
    }
    let mut suffixes = Vec::new();
    for value in &m.domain_suffixes {
        let domain = normalize_domain(value).ok_or("invalid domain suffix")?;
        suffixes.push(format!(".{domain}"));
        domains.push(domain);
    }
    let mut cidrs = Vec::new();
    for value in &m.ip_cidrs {
        cidrs.push(masked_prefix(value).ok_or("invalid CIDR")?);
    }
    if m.ip_is_private {
        cidrs.extend(PRIVATE_PREFIXES.iter().map(|p| (*p).to_owned()));
    }
    let mut port_ranges = Vec::new();
    for value in &m.port_ranges {
        let (start, end) = parse_port_range(value)?;
        port_ranges.push(format!("{start}:{end}"));
    }
    insert_list(&mut out, "network", &m.protocols);
    insert_list(&mut out, "domain", &domains);
    insert_list(&mut out, "domain_suffix", &suffixes);
    insert_list(&mut out, "ip_cidr", &cidrs);
    if !m.ports.is_empty() {
        out.insert("port".into(), json!(m.ports));
    }
    insert_list(&mut out, "port_range", &port_ranges);
    Ok(out)
}

fn insert_list(out: &mut Map<String, Value>, key: &str, values: &[String]) {
    if !values.is_empty() {
        out.insert(key.into(), json!(values));
    }
}

/// `netip.ParsePrefix(v).Masked().String()`.
fn masked_prefix(value: &str) -> Option<String> {
    let (addr, bits) = value.split_once('/')?;
    let addr: IpAddr = addr.parse().ok()?;
    if bits.is_empty()
        || !bits.bytes().all(|b| b.is_ascii_digit())
        || (bits.len() > 1 && bits.starts_with('0'))
    {
        return None;
    }
    let bits: u32 = bits.parse().ok()?;
    let masked = match addr {
        IpAddr::V4(v4) if bits <= 32 => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            IpAddr::from((u32::from(v4) & mask).to_be_bytes())
        }
        IpAddr::V6(v6) if bits <= 128 => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            IpAddr::from((u128::from(v6) & mask).to_be_bytes())
        }
        _ => return None,
    };
    Some(format!("{masked}/{bits}"))
}

fn outbound(ingress: &Ingress, tag: &str) -> Result<Value, String> {
    let mut out = json!({
        "tag": tag,
        "server": dial_address(ingress),
        "server_port": ingress.endpoint.port,
    });
    let c = &ingress.credentials;
    match ingress.protocol.as_str() {
        "shadowsocks" => {
            let ss = c
                .shadowsocks
                .as_ref()
                .ok_or("missing shadowsocks credentials")?;
            // SIP022 EIH: server iPSKs outermost first, then the user's uPSK.
            let mut keys = ss.identity_keys.clone();
            keys.push(ss.user_key.clone());
            out["type"] = "shadowsocks".into();
            out["method"] = ss.method.clone().into();
            out["password"] = keys.join(":").into();
        }
        "vless" => {
            let vless = c.vless.as_ref().ok_or("missing vless credentials")?;
            out["type"] = "vless".into();
            out["uuid"] = vless.uuid.clone().into();
            if !vless.flow.is_empty() {
                out["flow"] = vless.flow.clone().into();
            }
        }
        "anytls" => {
            let anytls = c.anytls.as_ref().ok_or("missing anytls credentials")?;
            out["type"] = "anytls".into();
            out["password"] = anytls.password.clone().into();
        }
        other => return Err(format!("unsupported protocol {other:?}")),
    }
    if let Some(t) = &ingress.tls {
        out["tls"] = tls(t);
    }
    Ok(out)
}

/// The ingress IP when the profile gives one (no DNS lookup to reach a
/// node, which in TUN mode could loop into the tunnel's own DNS), else its
/// domain. TLS keeps its explicit server_name.
fn dial_address(ingress: &Ingress) -> String {
    match ingress.endpoint.ip.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => v6.to_string(),
        },
        Ok(ip) => ip.to_string(),
        Err(_) => ingress.endpoint.domain.clone(),
    }
}

fn tls(t: &Tls) -> Value {
    let mut out = json!({ "enabled": true });
    if !t.server_name.is_empty() {
        out["server_name"] = t.server_name.clone().into();
    }
    if t.insecure {
        out["insecure"] = true.into();
    }
    if !t.alpn.is_empty() {
        out["alpn"] = json!(t.alpn);
    }
    if let Some(reality) = &t.reality {
        out["reality"] = json!({
            "enabled": true,
            "public_key": reality.public_key,
            "short_id": reality.short_id,
        });
        out["utls"] = json!({ "enabled": true, "fingerprint": "chrome" });
    }
    out
}

/// `node-` and the first 8 bytes of the id's SHA-256, as Go's.
pub(crate) fn node_tag(id: &str) -> String {
    format!("node-{}", hex_prefix(id, 8))
}

/// A failover member's tag from the node id and the ingress endpoint_key: a
/// replica keeps its tag across revisions whatever its position.
pub(crate) fn ingress_tag(node_id: &str, endpoint_key: &str) -> String {
    format!("{}-{}", node_tag(node_id), hex_prefix(endpoint_key, 4))
}

pub(crate) fn rule_set_tag(id: &str) -> String {
    format!("rule-set-{id}")
}

fn hex_prefix(value: &str, bytes: usize) -> String {
    let sum = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(bytes * 2);
    for b in &sum[..bytes] {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests;
