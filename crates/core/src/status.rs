//! The status snapshot and the state machine (docs/host-integration.md,
//! section 5). Field names are Core API v1's `get-status`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::request::RoutingMode;

/// The authoritative snapshot ([`crate::Engine::status`]); never blocked by
/// lifecycle calls. Read it, then follow events.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Status {
    #[serde(flatten)]
    pub state: EngineState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_mode: Option<RoutingMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_node_id: Option<String>,
    pub node_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_ingress: Option<IngressStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_sets: Vec<RuleSetStatus>,
    pub system_proxy: SystemProxyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_proxy: Option<LocalProxyStatus>,
    pub draining_kernels: u32,
    /// TUN instances (Linux, macOS, Windows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_routing: Option<TunRouting>,
    /// macOS only: the routes of other VPNs that the TUN took over, a line
    /// each ("route 128.0.0.0/1 via 192.0.2.1 on utun4"); sail puts them
    /// back when it stops. Empty elsewhere and while not running.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced_routes: Vec<String>,
    /// Log lines dropped because the sink blocked (section 10).
    pub dropped_log_lines: u64,
}

/// The state machine (section 5). Serialised tagged, snake_case:
/// `{"state":"degraded","reasons":[{"kind":"ingress_unavailable","node_id":"jp"}]}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EngineState {
    /// No profile applied (or shut down).
    #[default]
    Stopped,
    /// Applied, not started.
    Configured,
    Running,
    /// Healing on its own: show the reasons; do not recreate the instance.
    Degraded {
        reasons: Vec<DegradedReason>,
    },
    /// Cannot heal: drop and recreate the instance.
    Fatal {
        reason: FatalReason,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DegradedReason {
    /// Offline; probes fail at once; back to running when the network is.
    NoDefaultInterface,
    /// Every ingress of the node is down; failover keeps trying.
    IngressUnavailable { node_id: String },
    /// The pinned ingress is down; a pin does not fail over.
    PinnedLineDown {
        node_id: String,
        endpoint_key: String,
    },
    /// The TUN's routing was deleted and is being put back.
    TunRoutingRestoring,
    /// The routing guard did not start; routing is as installed.
    TunRoutingUnguarded,
    /// Another program changed the TUN's routes or address (macOS,
    /// Windows); sail reports it and does not put it back. Traffic may go
    /// around the TUN until the instance starts again.
    TunRoutingBroken { missing: Vec<String> },
    /// The rule set is unavailable; its rules are degraded.
    RuleSetUnavailable { rule_set_id: String },
    /// No DNS servers on the default interface: direct names get SERVFAIL.
    LocalDnsUnavailable,
    /// The local proxy port cannot be listened on; retried with backoff.
    LocalProxyUnavailable,
    /// Another VPN took the default route.
    DefaultRouteOverridden,
    /// The applied profile is past its `expires_at`; forwarding goes on.
    ProfileExpired { expires_at: DateTime<Utc> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum FatalReason {
    /// The TUN's routing was deleted and could not be put back.
    TunRoutingBroken { missing: Vec<String> },
    /// The TUN device is gone and could not be recreated.
    TunDeviceLost,
    /// A panic was caught at the API boundary.
    Panic,
    /// A kernel failed to start and the previous one could not be restored.
    KernelUnrecoverable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TunRouting {
    Ok,
    Restoring,
    Unguarded,
    /// Another program changed what the TUN's routing set up, and nothing
    /// puts it back (macOS, Windows): until the next start.
    Broken,
}

/// The ingress a node is actually using (`get-status.selected_ingress`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IngressStatus {
    pub endpoint_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub previous_endpoint_key: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub switched_at: Option<DateTime<Utc>>,
}

/// A node's pin and ingress health (`get-status.nodes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NodeStatus {
    pub node_id: String,
    pub pinned_endpoint_key: Option<String>,
    pub ingresses: Vec<IngressHealth>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IngressHealth {
    pub endpoint_key: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check_at: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RuleSetStatus {
    pub id: String,
    /// `ready`, `stale` or `unavailable`.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Consecutive failed downloads while not ready; omitted when zero.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub failures: u32,
    /// When a set that is not ready is retried; none when ready, or when
    /// nothing changes before the next apply (host not pinned, no storage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<DateTime<Utc>>,
}

/// The unauthenticated loopback listener for OS proxy settings (7891).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SystemProxyStatus {
    pub available: bool,
    pub enabled: bool,
    pub listening: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub listen: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<String>,
}

/// The shared local proxy (no credentials).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LocalProxyStatus {
    pub listen: String,
    pub port: u16,
    pub listening: bool,
    /// Set when this instance's `new` replaced the persisted prefix and
    /// password, and why: clients holding the old credential must read it
    /// again. Only `new` resets; it stays set for the instance's lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_reset: Option<CredentialsResetReason>,
}

/// Why `new` replaced the local proxy credentials
/// (`status.local_proxy.credentials_reset`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CredentialsResetReason {
    /// The state file could not be used (not JSON, unknown version, invalid
    /// values): rebuilt.
    Corrupt,
    /// Other users could read the state file: its secret is no longer one.
    InsecurePermissions,
}

fn is_zero(port: &u16) -> bool {
    *port == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As Go's get-status: failures and next_retry_at only when they say
    /// something.
    #[test]
    fn rule_set_status_omits_quiet_fields() {
        let ready = RuleSetStatus {
            id: "cn".into(),
            state: "ready".into(),
            updated_at: None,
            error: String::new(),
            failures: 0,
            next_retry_at: None,
        };
        assert_eq!(
            serde_json::to_value(&ready).unwrap(),
            serde_json::json!({"id": "cn", "state": "ready"})
        );
        let retrying = RuleSetStatus {
            state: "unavailable".into(),
            error: "RULE_SET_DOWNLOAD_FAILED".into(),
            failures: 3,
            next_retry_at: Some("2026-10-03T08:00:20Z".parse().unwrap()),
            ..ready
        };
        assert_eq!(
            serde_json::to_value(&retrying).unwrap(),
            serde_json::json!({
                "id": "cn",
                "state": "unavailable",
                "error": "RULE_SET_DOWNLOAD_FAILED",
                "failures": 3,
                "next_retry_at": "2026-10-03T08:00:20Z",
            })
        );
    }
}
