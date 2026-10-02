//! The apply request and its validation (docs/host-integration.md, 4.1).
//! `validate_request` is everything `apply` checks before it touches the
//! running instance; it needs no instance and no network.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{codes, Error};
use crate::profile::{self, Profile};

/// Which profile rules apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RoutingMode {
    /// Every profile rule and the profile's final action.
    #[default]
    Rules,
    /// Only the baseline rules; the final action proxies the selected node.
    Global,
}

impl RoutingMode {
    /// Reads a host-supplied mode: empty means rules.
    pub fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "" | "rules" => Ok(Self::Rules),
            "global" => Ok(Self::Global),
            _ => Err(Error::invalid(
                codes::ROUTING_MODE_INVALID,
                "routing_mode",
                "routing_mode must be rules or global",
            )),
        }
    }
}

/// A host-persisted ingress pin.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Pin {
    pub node_id: String,
    pub endpoint_key: String,
}

impl Pin {
    pub fn new(node_id: impl Into<String>, endpoint_key: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            endpoint_key: endpoint_key.into(),
        }
    }
}

/// What `apply` takes: the raw profile and the host-persisted state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ApplyRequest {
    /// The profile JSON as fetched; unknown fields are ignored.
    #[serde(with = "serde_bytes_as_json")]
    pub profile: Vec<u8>,
    pub routing_mode: RoutingMode,
    /// `None`: the profile's `default_node_id`.
    pub selected_node_id: Option<String>,
    pub pins: Vec<Pin>,
    pub allowed_rule_set_hosts: Vec<String>,
}

impl ApplyRequest {
    pub fn new(profile: impl Into<Vec<u8>>) -> Self {
        Self {
            profile: profile.into(),
            ..Self::default()
        }
    }
}

/// Decodes and validates `request` at `now`, as `apply` does before it
/// changes anything: the profile as given (D3: a stale `default_node_id` is
/// rejected even when the host's selection would replace it), the allowed
/// rule set hosts, then the pins.
pub fn validate_request(request: &ApplyRequest, now: DateTime<Utc>) -> Result<Profile, Error> {
    let profile = profile::parse(&request.profile)?;
    profile::validate(&profile, now)?;
    if !request.allowed_rule_set_hosts.is_empty() {
        profile::validate_rule_set_hosts(&profile, &request.allowed_rule_set_hosts)?;
    }
    let mut pinned = HashSet::new();
    for (i, pin) in request.pins.iter().enumerate() {
        if !pinned.insert(pin.node_id.as_str()) {
            return Err(Error::invalid(
                codes::PINS_INVALID,
                format!("pins[{i}].node_id"),
                "a node may be pinned at most once",
            ));
        }
    }
    Ok(profile)
}

/// The profile travels as JSON text inside a serialised request (FFI), not as
/// a byte array.
mod serde_bytes_as_json {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).unwrap_or(serde_json::Value::Null);
        serde::Serialize::serialize(&value, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_null() {
            return Ok(Vec::new());
        }
        serde_json::to_vec(&value).map_err(serde::de::Error::custom)
    }
}
