//! The Linux TUN's policy routing kept in place (Go: internal/tunrules,
//! 0.5.20). sail's auto_route installs the TUN's rules and the routes of its
//! table once, when the TUN starts, and never again; anything that deletes
//! them (systemd-networkd drops foreign rules whenever a link goes down)
//! leaves the TUN bypassed, strict route included, until the TUN is rebuilt.
//! A [`Guard`] snapshots what sail installed right after the start and puts
//! back what goes missing.
//!
//! The snapshot is what was listed, not rules rebuilt from the options, so
//! priorities, tables and selectors are sail's by construction. sail's
//! auto_route rules are sing-tun's (sail/src/platform/auto_route.rs), so
//! Go's ownership test applies unchanged.
//!
//! This module is the model (rules, routes, what is ours, what is missing,
//! the order of putting back), built everywhere; the netlink side and the
//! guard are Linux only.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::net::IpAddr;

#[cfg(target_os = "linux")]
mod guard;
#[cfg(all(test, target_os = "linux"))]
mod linux_tests;
#[cfg(target_os = "linux")]
mod netlink;
#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(test)]
mod tests;

#[cfg(target_os = "linux")]
pub(crate) use guard::{sweep, Guard};
#[cfg(not(target_os = "linux"))]
pub(crate) use other::{sweep, Guard};

/// The main routing table (RT_TABLE_MAIN).
pub(crate) const TABLE_MAIN: u32 = 254;

/// What the guard reports, on a watch channel: the latest verdict. Each
/// but `Ok` is the Engine's `TunRoutingSignal` of the same name (dropping
/// what the signal does not carry), which makes it `Status::tun_routing`,
/// the state and the events (docs/host-integration.md, sections 5 and 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TunRoutingStatus {
    /// Everything in place since the start.
    Ok,
    /// Found missing; being put back: `Degraded{TunRoutingRestoring}`.
    Restoring { missing: Vec<String> },
    /// What was missing is back: leaves `TunRoutingRestoring` (and, after
    /// `Broken`, is `TunRoutingRestored`). Same as `Ok` otherwise.
    Restored { missing: Vec<String> },
    /// The guard did not start; the routing is as sail installed it:
    /// `Degraded{TunRoutingUnguarded}`. A restart would likely fail the same
    /// way, so it is not `Broken`, which hosts answer with a restart.
    Unguarded { error: String },
    /// Missing and could not be put back; traffic may bypass the TUN:
    /// `Fatal{TunRoutingBroken}` and `TunRoutingBroken`. A later check that
    /// finds it all back reports `Restored`.
    Broken { missing: Vec<String>, error: String },
}

/// An address family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Family {
    V4,
    V6,
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Family::V4 => "v4",
            Family::V6 => "v6",
        })
    }
}

fn family_of(addr: IpAddr) -> Family {
    match addr {
        IpAddr::V4(_) => Family::V4,
        IpAddr::V6(_) => Family::V6,
    }
}

/// An address and a prefix length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Prefix {
    pub addr: IpAddr,
    pub len: u8,
}

impl Prefix {
    pub(crate) fn new(addr: IpAddr, len: u8) -> Prefix {
        Prefix { addr, len }
    }

    /// Whether `addr` is in it.
    fn contains(&self, addr: IpAddr) -> bool {
        match (self.addr, addr) {
            (IpAddr::V4(net), IpAddr::V4(a)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.len)).unwrap_or(0);
                u32::from(net) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(a)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.len))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

impl fmt::Display for Prefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.len)
    }
}

/// What a rule does (FR_ACT_*, linux/fib_rules.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    Table(u32),
    Goto(u32),
    Nop,
    Blackhole,
    Unreachable,
    Prohibit,
    Other(u8),
}

/// A policy routing rule as the kernel lists it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Rule {
    pub family: Family,
    pub priority: u32,
    pub action: Action,
    /// `not`: the action applies to what the selectors do not match.
    pub invert: bool,
    pub src: Option<Prefix>,
    pub dst: Option<Prefix>,
    pub tos: u8,
    /// Mark and mask; a mark without a mask is compared in full.
    pub fwmark: Option<(u32, u32)>,
    pub iif: Option<String>,
    pub oif: Option<String>,
    pub ip_proto: Option<u8>,
    pub sport: Option<(u16, u16)>,
    pub dport: Option<(u16, u16)>,
    pub uid_range: Option<(u32, u32)>,
    /// With `Action::Table` only.
    pub suppress_prefixlen: Option<u32>,
}

impl Rule {
    /// A rule matching everything of `family`.
    pub(crate) fn new(family: Family, priority: u32, action: Action) -> Rule {
        Rule {
            family,
            priority,
            action,
            invert: false,
            src: None,
            dst: None,
            tos: 0,
            fwmark: None,
            iif: None,
            oif: None,
            ip_proto: None,
            sport: None,
            dport: None,
            uid_range: None,
            suppress_prefixlen: None,
        }
    }

    /// The rule matches every packet.
    fn selectorless(&self) -> bool {
        let Rule {
            family: _,
            priority: _,
            action: _,
            invert,
            src,
            dst,
            tos,
            fwmark,
            iif,
            oif,
            ip_proto,
            sport,
            dport,
            uid_range,
            suppress_prefixlen: _,
        } = self;
        !invert
            && src.is_none()
            && dst.is_none()
            && *tos == 0
            && fwmark.is_none()
            && iif.is_none()
            && oif.is_none()
            && ip_proto.is_none()
            && sport.is_none()
            && dport.is_none()
            && uid_range.is_none()
    }
}

/// The rule in `ip rule` terms, prefixed by priority and family ("9093/v4
/// not dport 53-53 lookup 254 suppress_prefixlength 0"): Go's, word for
/// word, as hosts see it in `missing`. It is also the rule's identity: two
/// rules with the same string are the same rule.
impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut b = format!("{}/{}", self.priority, self.family);
        if self.invert {
            b.push_str(" not");
        }
        if let Some(src) = self.src {
            let _ = write!(b, " from {src}");
        }
        if let Some(dst) = self.dst {
            let _ = write!(b, " to {dst}");
        }
        if self.tos != 0 {
            let _ = write!(b, " tos {}", self.tos);
        }
        if let Some((mark, mask)) = self.fwmark {
            let _ = write!(b, " fwmark {mark:#x}/{mask:#x}");
        }
        if let Some(iif) = &self.iif {
            let _ = write!(b, " iif {iif}");
        }
        if let Some(oif) = &self.oif {
            let _ = write!(b, " oif {oif}");
        }
        if let Some(proto) = self.ip_proto {
            let _ = write!(b, " ipproto {proto}");
        }
        if let Some((start, end)) = self.sport {
            let _ = write!(b, " sport {start}-{end}");
        }
        if let Some((start, end)) = self.dport {
            let _ = write!(b, " dport {start}-{end}");
        }
        if let Some((start, end)) = self.uid_range {
            let _ = write!(b, " uidrange {start}-{end}");
        }
        match self.action {
            Action::Table(table) => {
                let _ = write!(b, " lookup {table}");
                if let Some(len) = self.suppress_prefixlen {
                    let _ = write!(b, " suppress_prefixlength {len}");
                }
            }
            Action::Goto(to) => {
                let _ = write!(b, " goto {to}");
            }
            Action::Nop => b.push_str(" nop"),
            Action::Blackhole => b.push_str(" blackhole"),
            Action::Unreachable => b.push_str(" unreachable"),
            Action::Prohibit => b.push_str(" prohibit"),
            Action::Other(action) => {
                let _ = write!(b, " action {action}");
            }
        }
        f.write_str(&b)
    }
}

/// A route of the scope's table through the TUN.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Route {
    pub dst: Prefix,
    pub gateway: Option<IpAddr>,
    /// The output interface's index.
    pub oif: u32,
    pub metric: u32,
}

impl Route {
    fn covers_gateway(&self) -> bool {
        self.gateway.is_some_and(|gw| self.dst.contains(gw))
    }
}

impl fmt::Display for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.dst)?;
        if let Some(gw) = self.gateway {
            write!(f, " via {gw}")?;
        }
        write!(f, " dev#{} metric {}", self.oif, self.metric)
    }
}

/// The namespace sail's auto_route was given: the TUN interface, its table
/// and its rule priorities `rule_start..=rule_end` (sail's cleanup claims
/// exactly that range).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scope {
    pub interface: String,
    pub table: u32,
    pub rule_start: u32,
    pub rule_end: u32,
}

impl Scope {
    /// The desktop TUN's: the table and rule index the translation gives
    /// sail (`iproute2_table_index`, `iproute2_rule_index`), and the ten
    /// priorities after it auto_route numbers its rules in
    /// (sail/src/platform/auto_route.rs, `RULE_SPAN`).
    pub(crate) fn desktop(interface: &str) -> Scope {
        let rule_start = crate::translate::IPROUTE2_RULE_INDEX;
        Scope {
            interface: interface.to_owned(),
            table: crate::translate::IPROUTE2_TABLE_INDEX,
            rule_start,
            rule_end: rule_start + 10,
        }
    }

    pub(crate) fn in_range(&self, priority: u32) -> bool {
        (self.rule_start..=self.rule_end).contains(&priority)
    }

    /// Splits the rules listed in the scope's priority range into the ones
    /// sail installs and the rest. A rule is sail's when it looks up the
    /// scope's table, jumps within the range, names the TUN interface, or
    /// is one of its two selector-free kinds: the nop the range's gotos land
    /// on and the unreachable rules of strict route; plus its DNS rule ("not
    /// dport 53 lookup main suppress_prefixlength 0"). Anything else in the
    /// range was put there by another program after sail's start and is
    /// not ours to restore.
    pub(crate) fn owned(&self, rules: &[Rule]) -> (Vec<Rule>, Vec<Rule>) {
        rules
            .iter()
            .filter(|r| self.in_range(r.priority))
            .cloned()
            .partition(|r| self.owns(r))
    }

    fn owns(&self, r: &Rule) -> bool {
        let names_tun = |name: &Option<String>| {
            !self.interface.is_empty() && name.as_deref() == Some(self.interface.as_str())
        };
        match r.action {
            Action::Table(table) if table == self.table => return true,
            Action::Goto(to) if self.in_range(to) => return true,
            Action::Nop | Action::Unreachable if r.selectorless() => return true,
            _ => {}
        }
        if names_tun(&r.iif) || names_tun(&r.oif) {
            return true;
        }
        let dns = Rule {
            invert: false,
            dport: None,
            ..r.clone()
        };
        r.invert
            && r.dport == Some((53, 53))
            && r.action == Action::Table(TABLE_MAIN)
            && r.suppress_prefixlen == Some(0)
            && dns.selectorless()
    }
}

/// The entries of `want` not in `have` (counting duplicates), in `want`'s
/// order.
pub(crate) fn missing<T: Clone + fmt::Display>(want: &[T], have: &[T]) -> Vec<T> {
    let mut count: HashMap<String, usize> = HashMap::new();
    for item in have {
        *count.entry(item.to_string()).or_default() += 1;
    }
    let mut missing = Vec::new();
    for item in want {
        match count.get_mut(&item.to_string()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => missing.push(item.clone()),
        }
    }
    missing
}

/// Rules in the order they are added back: highest priority number first,
/// so a goto's target (the range's nop) exists before the goto and is
/// never shown as unresolved.
pub(crate) fn restore_order(rules: &[Rule]) -> Vec<Rule> {
    let mut out = rules.to_vec();
    out.sort_by_key(|r| std::cmp::Reverse(r.priority));
    out
}

/// Routes in the order they are added back, those whose own prefix covers
/// their gateway (fc00::/7 via the TUN's fde2:…::2) last. The kernel checks
/// a new route's gateway in the route's table first: once such a route is
/// back, every gateway route added after it resolves its gateway through
/// it, not on-link, and is refused (EHOSTUNREACH). sail's routes have no
/// gateway; kept from Go for routes that do.
pub(crate) fn route_restore_order(routes: &[Route]) -> Vec<Route> {
    let mut out = routes.to_vec();
    out.sort_by_key(Route::covers_gateway);
    out
}

/// The rules' and routes' strings, for logs and the status.
pub(crate) fn names(rules: &[Rule], routes: &[Route]) -> Vec<String> {
    rules
        .iter()
        .map(ToString::to_string)
        .chain(routes.iter().map(|r| format!("route {r}")))
        .collect()
}
