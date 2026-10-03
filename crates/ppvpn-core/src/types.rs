//! Values the queries, probes and local proxy calls return
//! (docs/host-integration.md, section 4). Field names are Core API v1's.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `list-nodes` / `get-selected-node`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NodeInfo {
    pub id: String,
    pub name: String,
    pub entry_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entry_label: String,
    /// The primary ingress's protocol.
    pub protocol: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub region: String,
    pub tcp: bool,
    pub udp: bool,
    pub ingresses: Vec<IngressInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IngressInfo {
    pub endpoint_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub replica_ordinal: i64,
    pub role: String,
    pub protocol: String,
}

/// Cumulative bytes, direction as the client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Traffic {
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub measured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Connection {
    pub id: String,
    pub node_id: String,
    pub network: String,
    pub destination: String,
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub started_at: DateTime<Utc>,
}

/// What [`crate::Engine::version`] reports; hosts show it in diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VersionInfo {
    pub core_version: String,
    pub sail_version: String,
    pub sail_commit: String,
    pub profile_schema_version: u32,
    pub local_proxy_contract_version: u32,
}

/// What [`crate::Engine::shutdown`] could not clean up within its 10 s; the
/// next `new` sweeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ShutdownReport {
    pub leftovers: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProbeMethod {
    #[default]
    Tcp,
    Icmp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProbeEntrancesRequest {
    /// Empty: every node.
    #[serde(default)]
    pub node_ids: Vec<String>,
    #[serde(default)]
    pub method: ProbeMethod,
    pub timeout_ms: u64,
    pub concurrency: u32,
}

impl ProbeEntrancesRequest {
    pub fn new(method: ProbeMethod, timeout_ms: u64, concurrency: u32) -> Self {
        Self {
            node_ids: Vec::new(),
            method,
            timeout_ms,
            concurrency,
        }
    }
    pub fn with_node_ids(mut self, node_ids: Vec<String>) -> Self {
        self.node_ids = node_ids;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EntranceResult {
    pub node_id: String,
    pub method: ProbeMethod,
    pub success: bool,
    pub latency_ms: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    pub endpoint_key: String,
    pub ingress_role: String,
    pub ingresses: Vec<IngressProbeResult>,
    pub measured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IngressProbeResult {
    pub endpoint_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub replica_ordinal: i64,
    pub role: String,
    pub success: bool,
    pub latency_ms: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProbeAvailabilityRequest {
    pub node_id: String,
    /// The URL to fetch through the node's local proxy user.
    pub target: String,
    pub timeout_ms: u64,
}

impl ProbeAvailabilityRequest {
    pub fn new(node_id: impl Into<String>, target: impl Into<String>, timeout_ms: u64) -> Self {
        Self {
            node_id: node_id.into(),
            target: target.into(),
            timeout_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AvailabilityResult {
    pub node_id: String,
    pub total_ms: i64,
    pub success: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub http_status: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    pub measured_at: DateTime<Utc>,
}

/// `node` (a per-node user) or `routed` (the routed user, bare prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LocalProxyKind {
    Node,
    Routed,
}

/// A local proxy endpoint without its secret: what a WebView may see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LocalProxyMetadata {
    pub kind: LocalProxyKind,
    /// Empty for the routed user.
    pub node_id: String,
    pub listen: String,
    pub port: u16,
    pub protocols: Vec<String>,
    pub auth_required: bool,
}

/// A local proxy credential: native credential panels only. Its `Debug`
/// leaves the username and password out, so a log line never carries them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LocalProxyCredential {
    pub kind: LocalProxyKind,
    pub node_id: String,
    pub listen: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for LocalProxyCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProxyCredential")
            .field("kind", &self.kind)
            .field("node_id", &self.node_id)
            .field("listen", &self.listen)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

fn is_zero(value: &u16) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_debugs_without_its_secrets() {
        let secret = format!("s{}", std::process::id());
        let credential = LocalProxyCredential {
            kind: LocalProxyKind::Node,
            node_id: "jp".into(),
            listen: "127.0.0.1".into(),
            port: 7890,
            username: format!("u{secret}"),
            password: secret.clone(),
        };
        let shown = format!("{credential:?}");
        assert!(
            !shown.contains(&secret),
            "the debug output carries a secret"
        );
        assert!(shown.contains("jp") && shown.contains("7890"));
    }
}
