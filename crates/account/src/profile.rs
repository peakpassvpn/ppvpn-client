//! What a client shows of a proxy profile (`GET /me/proxy-profile`): its
//! revision, expiry, nodes with their replicas, and the default node.
//!
//! The raw bytes stay authoritative (the engine validates and runs them);
//! this lenient view ignores every field it does not read, but refuses a
//! document that is not the unified schema-1 profile.

use serde::Deserialize;

/// The only proxy-profile schema this view accepts (no negotiation).
pub const PROFILE_SCHEMA_VERSION: u64 = 1;

/// The UI view of a proxy profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileView {
    pub revision: String,
    pub expires_at: String,
    pub nodes: Vec<NodeView>,
    /// `selection.default_node_id`, when set.
    pub default_node_id: Option<String>,
}

/// One node of a [`ProfileView`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeView {
    pub id: String,
    pub name: String,
    pub entry_key: String,
    pub entry_label: Option<String>,
    pub exit_region: Option<String>,
    pub exit_country_code: Option<String>,
    pub udp: bool,
    /// Sorted by `replica_ordinal`.
    pub replicas: Vec<ReplicaView>,
}

/// One ingress (replica) of a [`NodeView`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaView {
    pub endpoint_key: String,
    pub replica_ordinal: u32,
    pub protocol: String,
    /// Optional display name of the replica (≤ 32 chars).
    pub label: Option<String>,
}

/// Why a document is not a usable profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// Not JSON (or not an object of the expected types).
    Json(String),
    /// JSON, but not the unified schema-1 shape; shown as
    /// `PROFILE_SHAPE: <detail>`.
    Shape(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(detail) => f.write_str(detail),
            Self::Shape(detail) => write!(f, "PROFILE_SHAPE: {detail}"),
        }
    }
}

#[derive(Deserialize)]
struct ProfileDoc {
    #[serde(default)]
    schema_version: Option<u64>,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    expires_at: String,
    #[serde(default)]
    nodes: Vec<ProfileNode>,
    #[serde(default)]
    selection: Option<ProfileSelection>,
}

#[derive(Deserialize)]
struct ProfileSelection {
    #[serde(default)]
    default_node_id: Option<String>,
}

#[derive(Deserialize)]
struct ProfileNode {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    entry_key: String,
    #[serde(default)]
    entry_label: Option<String>,
    #[serde(default)]
    exit: ProfileExit,
    #[serde(default)]
    capabilities: ProfileCapabilities,
    #[serde(default)]
    ingresses: Vec<ProfileIngress>,
}

#[derive(Deserialize, Default)]
struct ProfileExit {
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    country_code: Option<String>,
}

#[derive(Deserialize, Default)]
struct ProfileCapabilities {
    #[serde(default)]
    udp: bool,
}

#[derive(Deserialize)]
struct ProfileIngress {
    #[serde(default)]
    endpoint_key: String,
    #[serde(default)]
    replica_ordinal: u32,
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    label: Option<String>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// Rejects every document that is not the unified schema-1 profile: each
/// node needs an id, an `entry_key` and at least one ingress with an
/// `endpoint_key`. Unknown fields are ignored.
fn check_shape(doc: &ProfileDoc) -> Result<(), ProfileError> {
    let shape = ProfileError::Shape;
    match doc.schema_version {
        Some(PROFILE_SCHEMA_VERSION) => {}
        Some(other) => return Err(shape(format!("schema_version {other} unsupported"))),
        None => return Err(shape("schema_version missing".into())),
    }
    if doc.revision.trim().is_empty() {
        return Err(shape("revision missing".into()));
    }
    for (index, node) in doc.nodes.iter().enumerate() {
        if node.id.trim().is_empty() {
            return Err(shape(format!("nodes[{index}].id missing")));
        }
        if node.entry_key.trim().is_empty() {
            return Err(shape(format!("nodes[{index}].entry_key missing")));
        }
        if node.ingresses.is_empty() {
            return Err(shape(format!("nodes[{index}].ingresses missing")));
        }
        if let Some(slot) = node
            .ingresses
            .iter()
            .position(|ingress| ingress.endpoint_key.trim().is_empty())
        {
            return Err(shape(format!(
                "nodes[{index}].ingresses[{slot}].endpoint_key missing"
            )));
        }
    }
    Ok(())
}

/// Parse the UI view out of raw proxy-profile bytes.
pub fn parse_profile(raw: &[u8]) -> Result<ProfileView, ProfileError> {
    let doc: ProfileDoc =
        serde_json::from_slice(raw).map_err(|error| ProfileError::Json(error.to_string()))?;
    check_shape(&doc)?;
    let nodes = doc
        .nodes
        .into_iter()
        .map(|node| {
            let mut replicas: Vec<ReplicaView> = node
                .ingresses
                .into_iter()
                .map(|ingress| ReplicaView {
                    endpoint_key: ingress.endpoint_key,
                    replica_ordinal: ingress.replica_ordinal,
                    protocol: ingress.protocol,
                    label: non_empty(ingress.label),
                })
                .collect();
            replicas.sort_by_key(|replica| replica.replica_ordinal);
            NodeView {
                id: node.id,
                name: node.name,
                entry_key: node.entry_key,
                entry_label: non_empty(node.entry_label),
                exit_region: non_empty(node.exit.region),
                exit_country_code: non_empty(node.exit.country_code),
                udp: node.capabilities.udp,
                replicas,
            }
        })
        .collect();
    Ok(ProfileView {
        revision: doc.revision,
        expires_at: doc.expires_at,
        nodes,
        default_node_id: non_empty(
            doc.selection
                .and_then(|selection| selection.default_node_id),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/proxy-profile.json");

    #[test]
    fn parses_fixture_profile_summary_and_nodes() {
        let parsed = parse_profile(FIXTURE).expect("fixture parses");
        assert_eq!(
            parsed.revision,
            "e4a97a896762108f6b8e05e95e2ce1581b8af4b35044c2898a567a32627d5876"
        );
        assert_eq!(parsed.expires_at, "2099-01-01T00:00:00Z");
        assert_eq!(parsed.nodes.len(), 3);
        assert_eq!(
            parsed.default_node_id.as_deref(),
            Some("7d7c34e4-7f38-4c0c-9a53-1f0c0c9e2b11-101")
        );

        let first = &parsed.nodes[0];
        assert_eq!(first.id, "7d7c34e4-7f38-4c0c-9a53-1f0c0c9e2b11-101");
        // As the server sends it: the flag stays in the name here.
        assert_eq!(first.name, "🇺🇸洛杉矶家宽-203.0.113.10");
        assert_eq!(first.entry_key, "cn-optimized");
        assert_eq!(first.entry_label.as_deref(), Some("三网优化"));
        assert_eq!(first.exit_region.as_deref(), Some("洛杉矶"));
        assert_eq!(first.exit_country_code.as_deref(), Some("US"));
        assert!(first.udp);
        let replicas: Vec<_> = first
            .replicas
            .iter()
            .map(|r| {
                (
                    r.endpoint_key.as_str(),
                    r.replica_ordinal,
                    r.protocol.as_str(),
                )
            })
            .collect();
        assert_eq!(replicas, [("11", 0, "shadowsocks"), ("12", 1, "vless")]);

        assert_eq!(parsed.nodes[1].entry_key, "standard");
        assert_eq!(parsed.nodes[2].exit_region.as_deref(), Some("东京"));
    }

    #[test]
    fn parses_optional_fields_and_sorts_replicas() {
        let raw = br#"{
            "schema_version": 1, "revision": "r1", "expires_at": "2030-01-01T00:00:00Z",
            "future_field": {"x": 1},
            "nodes": [{
                "id": "n1", "name": "Node", "entry_key": "standard",
                "entry_label": "Standard", "unknown": true,
                "exit": {"region": "Tokyo", "country_code": "JP"},
                "capabilities": {"tcp": true, "udp": false},
                "ingresses": [
                    {"endpoint_key": "b", "replica_ordinal": 2, "protocol": "anytls", "label": "Backup B"},
                    {"endpoint_key": "a", "replica_ordinal": 0, "protocol": "vless"}
                ]
            }]
        }"#;
        let parsed = parse_profile(raw).unwrap();
        let node = &parsed.nodes[0];
        assert_eq!(node.entry_label.as_deref(), Some("Standard"));
        assert_eq!(node.exit_country_code.as_deref(), Some("JP"));
        assert!(!node.udp);
        assert_eq!(node.replicas[0].endpoint_key, "a");
        assert_eq!(node.replicas[1].endpoint_key, "b");
        assert_eq!(node.replicas[1].label.as_deref(), Some("Backup B"));
        assert_eq!(node.replicas[0].label, None);
        assert_eq!(parsed.default_node_id, None);
    }

    #[test]
    fn rejects_the_old_profile_format() {
        let old = r#"{"schema_version":1,"revision":"r","expires_at":"2099-01-01T00:00:00Z","nodes":[{"id":"a-1","name":"x","exit":{"region":"东京"}}]}"#;
        let detail = |raw: &[u8]| match parse_profile(raw).unwrap_err() {
            error @ ProfileError::Shape(_) => error.to_string(),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(
            detail(old.as_bytes()),
            "PROFILE_SHAPE: nodes[0].entry_key missing"
        );
        assert_eq!(
            detail(br#"{"schema_version":1,"revision":"r","nodes":[{"id":"a","entry_key":"k"}]}"#),
            "PROFILE_SHAPE: nodes[0].ingresses missing"
        );
        assert_eq!(
            detail(br#"{"schema_version":1,"revision":"r","nodes":[{"id":"a","entry_key":"k","ingresses":[{"protocol":"vless"}]}]}"#),
            "PROFILE_SHAPE: nodes[0].ingresses[0].endpoint_key missing"
        );
        assert_eq!(
            detail(br#"{"schema_version":2,"revision":"r","nodes":[]}"#),
            "PROFILE_SHAPE: schema_version 2 unsupported"
        );
        assert_eq!(
            detail(br#"{"revision":"r","nodes":[]}"#),
            "PROFILE_SHAPE: schema_version missing"
        );
    }

    #[test]
    fn rejects_non_json_profile() {
        assert!(matches!(
            parse_profile(b"<html>"),
            Err(ProfileError::Json(_))
        ));
    }
}
