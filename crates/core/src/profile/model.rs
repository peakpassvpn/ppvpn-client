//! The proxy profile (schema 1) as the backend sends it. Field names are the
//! JSON contract (docs/backend-profile.md); unknown fields are ignored. Go's
//! `profile` package is the reference: absent and `null` both read as the
//! zero value.

use chrono::{DateTime, FixedOffset};
use serde::Deserialize;

/// The only accepted profile format; there is no negotiation.
pub const CURRENT_SCHEMA_VERSION: i64 = 1;

/// The most ingresses one logical node may have (failover fan-out).
pub const MAX_INGRESSES_PER_NODE: usize = 64;

/// The most characters an ingress label may have.
pub const MAX_INGRESS_LABEL_LENGTH: usize = 32;

/// The most rule sets one profile may declare.
pub const MAX_RULE_SETS: usize = 32;

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Profile {
    pub schema_version: i64,
    pub revision: String,
    pub generated_at: Option<DateTime<FixedOffset>>,
    pub expires_at: Option<DateTime<FixedOffset>>,
    pub nodes: Vec<Node>,
    pub selection: Selection,
    pub routing: Routing,
}

/// A logical node: one exit identity reachable through one or more
/// ingresses. Selection, routing, local proxy users and probes address it by
/// `id`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Node {
    pub id: String,
    pub name: String,
    pub entry_key: String,
    pub entry_label: String,
    pub exit: Exit,
    pub capabilities: Capabilities,
    pub ingresses: Vec<Ingress>,
}

/// One way to reach a node's exit. Array order is failover order:
/// `ingresses[0]` is the primary, the rest are backups tried in order.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Ingress {
    pub role: String,
    pub endpoint_key: String,
    /// A display name only; `None` when absent.
    pub label: Option<String>,
    pub replica_ordinal: i64,
    pub protocol: String,
    pub endpoint: Endpoint,
    pub credentials: Credentials,
    pub tls: Option<Tls>,
    pub transport: Option<Transport>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Endpoint {
    pub domain: String,
    pub ip: String,
    pub port: u16,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Exit {
    pub ip: String,
    pub region: String,
    pub country_code: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
#[non_exhaustive]
pub struct Capabilities {
    pub tcp: bool,
    pub udp: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Selection {
    pub mode: String,
    pub default_node_id: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Routing {
    pub rule_sets: Vec<RuleSet>,
    pub rules: Vec<RoutingRule>,
    #[serde(rename = "final")]
    pub final_action: RoutingAction,
}

/// A downloadable sing-box binary rule set (`.srs`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct RuleSet {
    pub id: String,
    pub url: String,
    pub sha256: String,
    pub update_interval_seconds: i64,
}

/// Evaluated in array order.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct RoutingRule {
    pub id: String,
    #[serde(rename = "match")]
    pub matcher: RoutingMatch,
    pub action: RoutingAction,
    /// Also applies in the global routing mode.
    pub baseline: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct RoutingMatch {
    pub domains: Vec<String>,
    pub domain_suffixes: Vec<String>,
    pub ip_cidrs: Vec<String>,
    pub ip_is_private: bool,
    pub protocols: Vec<String>,
    pub ports: Vec<u16>,
    pub port_ranges: Vec<String>,
    pub rule_set_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct RoutingAction {
    #[serde(rename = "type")]
    pub kind: String,
    pub target: String,
    pub node_id: String,
}

/// A tagged union: exactly one member, matching the ingress protocol.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Credentials {
    pub shadowsocks: Option<ShadowsocksCredentials>,
    pub vless: Option<VlessCredentials>,
    pub anytls: Option<AnyTlsCredentials>,
}

/// SIP022: `identity_keys` are the server's iPSKs (outermost first), absent
/// for single-user SS2022; `user_key` is this user's uPSK.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct ShadowsocksCredentials {
    pub method: String,
    pub identity_keys: Vec<String>,
    pub user_key: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct VlessCredentials {
    pub uuid: String,
    pub flow: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct AnyTlsCredentials {
    pub password: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Tls {
    pub server_name: String,
    pub alpn: Vec<String>,
    pub insecure: bool,
    pub reality: Option<Reality>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Reality {
    pub public_key: String,
    pub short_id: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
#[non_exhaustive]
pub struct Transport {
    #[serde(rename = "type")]
    pub kind: String,
}
