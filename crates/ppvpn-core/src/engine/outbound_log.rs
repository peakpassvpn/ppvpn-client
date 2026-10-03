//! Failed connections (#179, Go: internal/outboundlog), from sail's
//! `DialFailed`: Go's debug `outbound failed` line, a direct outbound's at
//! most once per destination per [`DIRECT_LIMIT`]; and a failure through a
//! node's failover group reads the groups again at once, so the ingress
//! health in `status.nodes` follows without waiting for the next refresh.
//!
//! Transitional: sail names a group's member in the chain only once it
//! connected, and a member that fails before another one carries the
//! connection sends nothing, so a line names the group, not the ingress
//! (rust-parity group 7).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::Inner;
use crate::runtime::{DialFailed, Routed};
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
                    count = failed.count, "outbound failed");
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

impl Inner {
    /// One routed connection (from the watcher, at debug only).
    pub(super) fn on_routed(&self, routed: &Routed) {
        let live = self.live();
        connection_line(live.applied.as_ref().map(|a| &a.translation), routed);
    }
}

#[cfg(test)]
#[path = "outbound_log_tests.rs"]
mod tests;
