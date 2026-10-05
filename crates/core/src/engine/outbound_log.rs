//! Failed connections (#179, Go: internal/outboundlog), from sail's
//! `DialFailed`: Go's debug `outbound failed` line, a direct outbound's at
//! most once per destination per [`DIRECT_LIMIT`]; and a failure through a
//! node's failover group reads the groups again at once, so the ingress
//! health in `status.nodes` follows without waiting for the next refresh.
//!
//! The chain names every hop before the dial, outermost first (`F>G>m`),
//! and each member of a failover group that fails is one `DialFailed`
//! (`more_to_try` until the connection's last): one line per ingress, as
//! Go's (sail 2eb3fe47).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::Inner;
use crate::runtime::{DialFailed, DnsExchange, Routed};
use crate::translate::{Translation, DIRECT_TAG};

/// How often a direct outbound logs a failure to the same destination
/// (Go: DirectLimit): while the network is down every direct connection
/// fails.
pub(super) const DIRECT_LIMIT: Duration = Duration::from_secs(10);

/// The destinations remembered; past it, those older than the window go.
const LIMITER_SIZE: usize = 1024;

/// Go's limiter: per key, the first failure in a window is logged, the
/// next ones only counted.
pub(super) struct Limiter {
    window: Duration,
    seen: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    logged: Instant,
    suppressed: u64,
}

impl Limiter {
    pub(super) fn new(window: Duration) -> Self {
        Limiter {
            window,
            seen: Mutex::default(),
        }
    }

    /// Whether `count` failures for `key` at `now` are logged and, if so,
    /// how many were suppressed since the last line.
    pub(super) fn allow(&self, key: &str, now: Instant, count: u64) -> Option<u64> {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = seen.get_mut(key) {
            if now.duration_since(entry.logged) < self.window {
                entry.suppressed += count;
                return None;
            }
        } else if seen.len() >= LIMITER_SIZE {
            let window = self.window;
            // Those past the window would be logged anyway: forgotten.
            seen.retain(|_, e| now.duration_since(e.logged) < window);
            if seen.len() >= LIMITER_SIZE {
                // All still in their window: a new key is logged but not
                // remembered, so the table stays bounded.
                return Some(0);
            }
        }
        let entry = seen.entry(key.to_owned()).or_insert(Entry {
            logged: now,
            suppressed: 0,
        });
        let suppressed = entry.suppressed;
        entry.logged = now;
        // A batch of several logs one line; the rest count as suppressed
        // on the next.
        entry.suppressed = count.saturating_sub(1);
        Some(suppressed)
    }

    #[cfg(test)]
    fn remembered(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter::new(DIRECT_LIMIT)
    }
}

/// What a chain is, read against the configuration it ran on.
#[derive(Debug, PartialEq, Eq)]
enum Through {
    Direct,
    /// A node: its id, the ingress when the chain names one, and whether
    /// it went through the node's failover group.
    Node {
        node_id: String,
        endpoint_key: Option<String>,
        group: bool,
    },
    Other,
}

fn through(t: &Translation, chain: &str) -> Through {
    let tags: Vec<&str> = chain.split('>').collect();
    if tags.last() == Some(&DIRECT_TAG) {
        return Through::Direct;
    }
    let group = tags
        .iter()
        .any(|tag| t.groups.values().any(|g| g == tag) || t.groups.contains_key(*tag));
    // The innermost tag that is a node's outbound.
    for tag in tags.iter().rev() {
        if let Some(node_id) = t.outbound_nodes.get(*tag) {
            return Through::Node {
                node_id: node_id.clone(),
                endpoint_key: t.ingress_keys.get(*tag).cloned(),
                group,
            };
        }
    }
    Through::Other
}

impl Inner {
    /// One `DialFailed` (from the watcher).
    pub(super) async fn on_dial_failed(&self, failed: DialFailed) {
        let through = {
            let live = self.live();
            if !live.running {
                return;
            }
            match &live.applied {
                Some(a) => through(&a.translation, &failed.chain),
                None => return,
            }
        };
        let outbound = failed.chain.rsplit('>').next().unwrap_or_default();
        match &through {
            Through::Direct => {
                let key = failed.destination.as_str();
                if let Some(suppressed) =
                    self.outbound_log
                        .allow(key, Instant::now(), failed.count.max(1))
                {
                    if suppressed > 0 {
                        tracing::debug!(stage = %failed.stage, outbound, destination = %failed.destination,
                            error = %failed.error, suppressed, "outbound failed");
                    } else {
                        tracing::debug!(stage = %failed.stage, outbound, destination = %failed.destination,
                            error = %failed.error, "outbound failed");
                    }
                }
            }
            Through::Node {
                node_id,
                endpoint_key,
                ..
            } => {
                tracing::debug!(stage = %failed.stage, node_id = %node_id,
                    endpoint_key = %endpoint_key.as_deref().unwrap_or(""), outbound,
                    destination = %failed.destination, error = %failed.error,
                    count = failed.count, more_to_try = failed.more_to_try, "outbound failed");
            }
            Through::Other => {
                tracing::debug!(stage = %failed.stage, outbound, destination = %failed.destination,
                    error = %failed.error, count = failed.count, "outbound failed");
            }
        }
        // A failover member failed: its health changed now.
        if matches!(through, Through::Node { group: true, .. }) {
            self.refresh().await;
        }
    }
}

/// The `rule`, `target` and `target_kind` of a connection line.
fn rule_and_target<'a>(t: Option<&Translation>, r: &'a Routed) -> (String, &'a str, &'static str) {
    let rule = match r.rule {
        None => "final".to_owned(),
        Some(i) => t
            .and_then(|t| t.rule_ids.get(i).cloned())
            .unwrap_or_else(|| i.to_string()),
    };
    let target = r.request_destination.as_deref().unwrap_or(&r.destination);
    let host = target.rsplit_once(':').map_or(target, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let kind = if host.parse::<std::net::IpAddr>().is_ok() {
        "ip"
    } else {
        "domain"
    };
    (rule, target, kind)
}

/// Go's `connection` line (telemetry.go `logRouted`): one per routed
/// connection, at debug level, with Go's keys. `rule` is the profile rule's
/// id (or the engine's own rule, or `final`); `target` is what the outbound
/// was asked to reach, `target_kind` whether that is a domain. The domain
/// is logged (contract section 10: debug logs names), credentials never.
pub(super) fn connection_line(t: Option<&Translation>, r: &Routed) {
    let (rule, target, target_kind) = rule_and_target(t, r);
    let id = r.id.map(|id| id.to_string()).unwrap_or_default();
    // Go logs the outbound the rules named (its tag; a node's selector or
    // group, or direct): the chain's first, outermost hop. The member that
    // carried it is the chain's last.
    let outbound = r.chain.first().map(String::as_str).unwrap_or("");
    tracing::debug!(
        id = %id,
        inbound = %r.inbound,
        network = %r.network,
        destination = %r.destination,
        route_domain = %r.domain.as_deref().unwrap_or(""),
        protocol = %r.protocol.as_deref().unwrap_or(""),
        rule = %rule,
        outbound,
        target,
        target_kind,
        action = %r.action,
        error = %r.error.as_deref().unwrap_or(""),
        "connection"
    );
}

/// The logical server and the resolver of an exchange: a sequential
/// server's member (what sail names) maps back to its server, as the
/// translation recorded it, with the member's resolver as `upstream`.
fn dns_server<'a>(t: Option<&'a Translation>, e: &'a DnsExchange) -> (&'a str, Option<&'a str>) {
    let tag = e.server.as_deref().unwrap_or("");
    match t.and_then(|t| t.dns_members.get(tag)) {
        Some((server, upstream)) => (server.as_str(), Some(upstream.as_str())),
        None => (tag, None),
    }
}

/// Whether an exchange of sail's gets a `dns` line: sent upstream, and not
/// one of the engine's dns-local listener (it logs its own, with the
/// resolver it asked).
fn logs_dns(t: Option<&Translation>, e: &DnsExchange) -> bool {
    let own_listener = t.is_none_or(|t| t.dns_local_listener);
    e.source == "exchanged"
        && !(own_listener && dns_server(t, e).0 == crate::translate::DNS_LOCAL_TAG)
}

/// Go's dnstransport `dns` line for one of sail's exchanges (at debug):
/// those sent upstream only, as Go (answers from the cache or a rule have
/// none). `name` with its final dot, `server` the logical server and
/// `upstream` the resolver its member asked, `attempt` for a sequential
/// server, `rcode` in miekg's words.
pub(super) fn dns_line(t: Option<&Translation>, e: &DnsExchange) {
    if !logs_dns(t, e) {
        return;
    }
    let name = format!("{}.", e.name);
    let (server, upstream) = dns_server(t, e);
    let ms = e.duration_ms.unwrap_or(0);
    // `upstream` and `attempt` only where they apply (absent when None).
    match e.rcode.filter(|_| e.error.is_none()) {
        Some(rcode) => tracing::debug!(
            name = %name,
            "type" = %e.qtype,
            server,
            upstream,
            attempt = e.attempt,
            rcode = %crate::localdns::rcode_name(rcode),
            answers = e.answers_total,
            ms,
            "dns"
        ),
        None => tracing::debug!(
            name = %name,
            "type" = %e.qtype,
            server,
            upstream,
            attempt = e.attempt,
            error = %e.error.as_deref().unwrap_or("no answer"),
            ms,
            "dns"
        ),
    }
}

impl Inner {
    /// One of sail's DNS exchanges (from the watcher, at debug only).
    pub(super) fn on_dns_exchange(&self, exchange: &DnsExchange) {
        let live = self.live();
        dns_line(live.applied.as_ref().map(|a| &a.translation), exchange);
    }

    /// One routed connection (from the watcher, at debug only).
    pub(super) fn on_routed(&self, routed: &Routed) {
        let live = self.live();
        connection_line(live.applied.as_ref().map(|a| &a.translation), routed);
    }
}

#[cfg(test)]
#[path = "outbound_log_tests.rs"]
mod tests;
