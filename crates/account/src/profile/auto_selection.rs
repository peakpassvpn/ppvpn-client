//! Optional schema-1 facts for automatic ingress choice. This module does
//! not select, probe or authorize a route. Bad optional data must not turn
//! an otherwise usable manual profile into a decoding failure.

use std::net::IpAddr;

use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitMode {
    Primary,
    LocalFinal,
}

/// Stable, non-sensitive reason codes only; never echo arbitrary wire data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    UnsupportedEntryPolicy,
    LatencyThresholdUnavailable,
    InstanceIdentityUnavailable,
    ExitIdentityUnavailable,
    TierIdentityUnavailable,
    BillingMultiplierUnavailable,
    AppliedBindingUnavailable,
    MetadataConflict,
    MetadataUnavailable,
    InvalidMetadata,
    UnknownReason,
}

impl UnavailableReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::UnsupportedEntryPolicy => "unsupported_entry_policy",
            Self::LatencyThresholdUnavailable => "latency_threshold_unavailable",
            Self::InstanceIdentityUnavailable => "instance_identity_unavailable",
            Self::ExitIdentityUnavailable => "exit_identity_unavailable",
            Self::TierIdentityUnavailable => "tier_identity_unavailable",
            Self::BillingMultiplierUnavailable => "billing_multiplier_unavailable",
            Self::AppliedBindingUnavailable => "applied_binding_unavailable",
            Self::MetadataConflict => "metadata_conflict",
            Self::MetadataUnavailable => "metadata_unavailable",
            Self::InvalidMetadata => "invalid_metadata",
            Self::UnknownReason => "unknown_reason",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeFacts {
    pub instance_id: Option<String>,
    pub exit_mode: Option<ExitMode>,
    /// The normalized committed primary exit; not an observed egress probe.
    pub primary_exit_ip: Option<IpAddr>,
    pub unavailable_reason: Option<UnavailableReason>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    /// Opaque stable identity. Consumers must not infer ordering from it.
    pub entry_tier_id: String,
    pub entry_tier_key: String,
    pub tier_rank: u32,
    pub quota_multiplier: String,
    /// Exact thousandths, under the agreed numeric(6,3) contract.
    pub quota_multiplier_milli: u32,
    /// This candidate's entry-region threshold, not a global threshold.
    pub entry_latency_threshold_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngressFacts {
    pub metadata: Option<Metadata>,
    /// Always group with the node's instance id and exit mode as well.
    pub exit_ip: Option<IpAddr>,
    pub unavailable_reason: Option<UnavailableReason>,
}

fn ip(value: &Value) -> Option<IpAddr> {
    let address: IpAddr = value.as_str()?.parse().ok()?;
    // IPv4-mapped IPv6 must not form a second identity for the same IPv4.
    Some(match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        _ => address,
    })
}

pub(super) fn node_facts(instance: &Value, mode: &Value, exit_ip: &Value) -> NodeFacts {
    let instance_id = instance.as_str().filter(|s| s.len() == 36).and_then(|s| {
        Uuid::parse_str(s)
            .ok()
            .filter(|id| !id.is_nil())
            .map(|id| id.to_string())
    });
    let exit_mode = match mode.as_str() {
        Some("primary") => Some(ExitMode::Primary),
        Some("local_final") => Some(ExitMode::LocalFinal),
        _ => None,
    };
    let primary_exit_ip = (exit_mode == Some(ExitMode::Primary))
        .then(|| ip(exit_ip))
        .flatten();
    let unavailable_reason = if instance_id.is_none() {
        Some(UnavailableReason::InstanceIdentityUnavailable)
    } else if exit_mode.is_none()
        || (exit_mode == Some(ExitMode::Primary) && primary_exit_ip.is_none())
    {
        Some(UnavailableReason::ExitIdentityUnavailable)
    } else {
        None
    };
    NodeFacts {
        instance_id,
        exit_mode,
        primary_exit_ip,
        unavailable_reason,
    }
}

fn backend_reason(value: &Value) -> Option<UnavailableReason> {
    if value.is_null() {
        return None;
    }
    Some(match value.as_str() {
        Some("unsupported_entry_policy") => UnavailableReason::UnsupportedEntryPolicy,
        Some("latency_threshold_unavailable") => UnavailableReason::LatencyThresholdUnavailable,
        Some("instance_identity_unavailable") => UnavailableReason::InstanceIdentityUnavailable,
        Some("exit_identity_unavailable") => UnavailableReason::ExitIdentityUnavailable,
        Some("tier_identity_unavailable") => UnavailableReason::TierIdentityUnavailable,
        Some("billing_multiplier_unavailable") => UnavailableReason::BillingMultiplierUnavailable,
        Some("applied_binding_unavailable") => UnavailableReason::AppliedBindingUnavailable,
        Some("metadata_conflict") => UnavailableReason::MetadataConflict,
        Some(_) => UnavailableReason::UnknownReason,
        None => UnavailableReason::InvalidMetadata,
    })
}

/// Canonical positive decimal, without floating point or guessed defaults.
fn quota_multiplier_milli(value: &str) -> Option<u32> {
    if value.len() > 7 {
        return None;
    }
    let (whole, fraction) = value
        .split_once('.')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
    {
        return None;
    }
    let units: u32 = whole.parse().ok()?;
    if units > 999 {
        return None;
    }
    let fractional = if let Some(fraction) = fraction {
        if fraction.is_empty()
            || fraction.len() > 3
            || fraction.ends_with('0')
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        fraction.parse::<u32>().ok()? * 10u32.pow(3 - fraction.len() as u32)
    } else {
        0
    };
    let value = units * 1000 + fractional;
    (value > 0).then_some(value)
}

fn metadata(value: &Value, rank: &Value) -> Option<Metadata> {
    let value = value.as_object()?;
    let tier = value.get("entry_tier_id")?.as_str()?;
    if tier.is_empty()
        || tier.starts_with('0')
        || tier.len() > 19
        || !tier.bytes().all(|b| b.is_ascii_digit())
        || tier.parse::<u64>().ok()? > i64::MAX as u64
    {
        return None;
    }
    let key = value.get("entry_tier_key")?.as_str()?;
    if key.is_empty()
        || key.len() > 64
        || !key.as_bytes()[0].is_ascii_lowercase() && !key.as_bytes()[0].is_ascii_digit()
        || !key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    let rank = rank.as_u64().filter(|n| *n > 0 && *n <= i32::MAX as u64)? as u32;
    let threshold = value
        .get("entry_latency_threshold_ms")?
        .as_u64()
        .filter(|n| *n > 0 && *n <= i32::MAX as u64)? as u32;
    let multiplier = value.get("quota_multiplier")?.as_str()?;
    let quota_multiplier_milli = quota_multiplier_milli(multiplier)?;
    Some(Metadata {
        entry_tier_id: tier.into(),
        entry_tier_key: key.into(),
        tier_rank: rank,
        quota_multiplier: multiplier.into(),
        quota_multiplier_milli,
        entry_latency_threshold_ms: threshold,
    })
}

pub(super) fn ingress_facts(
    node: &NodeFacts,
    rank: &Value,
    exit_ip: &Value,
    raw_metadata: &Value,
    reason: &Value,
) -> IngressFacts {
    let unavailable = |reason| IngressFacts {
        metadata: None,
        exit_ip: None,
        unavailable_reason: Some(reason),
    };
    if let Some(reason) = backend_reason(reason) {
        return unavailable(if raw_metadata.is_null() {
            reason
        } else {
            UnavailableReason::MetadataConflict
        });
    }
    if let Some(reason) = node.unavailable_reason {
        return unavailable(reason);
    }
    let chosen_exit = match node.exit_mode {
        Some(ExitMode::Primary) => {
            if !exit_ip.is_null() && ip(exit_ip) != node.primary_exit_ip {
                return unavailable(UnavailableReason::MetadataConflict);
            }
            node.primary_exit_ip
        }
        Some(ExitMode::LocalFinal) => ip(exit_ip),
        None => None,
    };
    let Some(chosen_exit) = chosen_exit else {
        return unavailable(UnavailableReason::ExitIdentityUnavailable);
    };
    if raw_metadata.is_null() {
        return unavailable(UnavailableReason::MetadataUnavailable);
    }
    match metadata(raw_metadata, rank) {
        Some(metadata) => IngressFacts {
            metadata: Some(metadata),
            exit_ip: Some(chosen_exit),
            unavailable_reason: None,
        },
        None => unavailable(UnavailableReason::InvalidMetadata),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{parse_profile, ProfileView};
    use serde_json::json;

    fn document() -> Value {
        let mut doc: Value =
            serde_json::from_slice(include_bytes!("../../tests/fixtures/proxy-profile.json"))
                .unwrap();
        let node = &mut doc["nodes"][0];
        node["instance_id"] = json!("7d7c34e4-7f38-4c0c-9a53-1f0c0c9e2b11");
        node["exit_mode"] = json!("primary");
        node["exit"]["ip"] = json!("203.0.113.10");
        for ingress in node["ingresses"].as_array_mut().unwrap() {
            ingress["tier_rank"] = json!(2);
            ingress["auto_selection_metadata"] = json!({
                "entry_tier_id": "42", "entry_tier_key": "standard",
                "quota_multiplier": "0.75", "entry_latency_threshold_ms": 170
            });
        }
        doc
    }

    fn parse(doc: &Value) -> ProfileView {
        parse_profile(&serde_json::to_vec(doc).unwrap())
            .expect("manual profile view must remain decodable")
    }

    #[test]
    fn optional_contract_projects_authoritative_identity_and_exact_multiplier() {
        let p = parse(&document());
        let node = &p.nodes[0];
        assert_eq!(
            node.auto_selection.instance_id.as_deref(),
            Some("7d7c34e4-7f38-4c0c-9a53-1f0c0c9e2b11")
        );
        assert_eq!(node.auto_selection.exit_mode, Some(ExitMode::Primary));
        for r in &node.replicas {
            let m = r.auto_selection.metadata.as_ref().unwrap();
            assert_eq!((&*m.entry_tier_id, &*m.entry_tier_key), ("42", "standard"));
            assert_eq!(m.quota_multiplier_milli, 750);
            assert_eq!(m.tier_rank, 2);
            assert_eq!(m.entry_latency_threshold_ms, 170);
            assert_eq!(
                r.auto_selection.exit_ip,
                Some("203.0.113.10".parse().unwrap())
            );
            assert_eq!(r.auto_selection.unavailable_reason, None);
        }
    }

    #[test]
    fn old_manual_profile_needs_no_automatic_defaults() {
        let doc: Value =
            serde_json::from_slice(include_bytes!("../../tests/fixtures/proxy-profile.json"))
                .unwrap();
        let p = parse(&doc);
        for n in p.nodes {
            assert!(n.auto_selection.instance_id.is_none());
            for r in n.replicas {
                assert!(r.auto_selection.metadata.is_none());
            }
        }
    }

    #[test]
    fn malformed_optional_fields_never_reject_manual_document() {
        let paths = [
            "/nodes/0/instance_id",
            "/nodes/0/exit_mode",
            "/nodes/0/ingresses/0/tier_rank",
            "/nodes/0/ingresses/0/auto_selection_metadata",
            "/nodes/0/ingresses/0/auto_selection_metadata/entry_tier_id",
            "/nodes/0/ingresses/0/auto_selection_metadata/entry_tier_key",
            "/nodes/0/ingresses/0/auto_selection_metadata/quota_multiplier",
            "/nodes/0/ingresses/0/auto_selection_metadata/entry_latency_threshold_ms",
        ];
        for path in paths {
            for bad in [
                Value::Null,
                json!([]),
                json!({}),
                json!(false),
                json!(-1),
                json!(1.5),
                json!(""),
            ] {
                let mut doc = document();
                *doc.pointer_mut(path).unwrap() = bad;
                let p = parse(&doc);
                assert!(
                    p.nodes[0].replicas[0].auto_selection.metadata.is_none(),
                    "{path}"
                );
                assert_eq!(p.nodes[0].id, doc["nodes"][0]["id"].as_str().unwrap());
                assert_eq!(
                    p.nodes[0].replicas.len(),
                    doc["nodes"][0]["ingresses"].as_array().unwrap().len()
                );
            }
        }
    }

    #[test]
    fn decimal_bounds_and_canonical_syntax_are_exact() {
        for (s, n) in [
            ("0.001", 1),
            ("0.01", 10),
            ("0.5", 500),
            ("0.75", 750),
            ("1", 1000),
            ("1.001", 1001),
            ("999.999", 999999),
        ] {
            assert_eq!(quota_multiplier_milli(s), Some(n), "{s}");
        }
        for s in [
            "",
            "0",
            "0.0",
            "-1",
            "+1",
            "01",
            "00.5",
            "1e0",
            "1.",
            ".5",
            "1.000",
            "0.0001",
            "1000",
            "1000.001",
            "999999999999999999",
            " 1",
            "1 ",
            "１",
        ] {
            assert_eq!(quota_multiplier_milli(s), None, "{s}");
        }
    }

    #[test]
    fn each_candidate_keeps_its_own_threshold_and_rank_is_not_multiplier() {
        let mut doc = document();
        let mut second = doc["nodes"][0]["ingresses"][0].clone();
        second["endpoint_key"] = json!("another-allowed-endpoint");
        second["replica_ordinal"] = json!(99);
        second["tier_rank"] = json!(1);
        second["auto_selection_metadata"]["entry_latency_threshold_ms"] = json!(300);
        second["auto_selection_metadata"]["quota_multiplier"] = json!("1.5");
        doc["nodes"][0]["ingresses"]
            .as_array_mut()
            .unwrap()
            .push(second);
        let p = parse(&doc);
        let first = p.nodes[0].replicas[0]
            .auto_selection
            .metadata
            .as_ref()
            .unwrap();
        let second = p.nodes[0]
            .replicas
            .last()
            .unwrap()
            .auto_selection
            .metadata
            .as_ref()
            .unwrap();
        assert_eq!(
            (
                first.entry_latency_threshold_ms,
                second.entry_latency_threshold_ms
            ),
            (170, 300)
        );
        assert_eq!((first.tier_rank, second.tier_rank), (2, 1));
        assert_eq!(
            (first.quota_multiplier_milli, second.quota_multiplier_milli),
            (750, 1500)
        );
    }

    #[test]
    fn invalid_identity_and_threshold_disable_without_numeric_coercion() {
        for (field, values) in [
            (
                "entry_tier_id",
                vec![
                    json!(42),
                    json!("0"),
                    json!("01"),
                    json!("-1"),
                    json!("9223372036854775808"),
                ],
            ),
            (
                "entry_tier_key",
                vec![
                    json!("UPPER"),
                    json!("-prefix"),
                    json!("bad/key"),
                    json!("x".repeat(65)),
                ],
            ),
            (
                "entry_latency_threshold_ms",
                vec![
                    json!(0),
                    json!(-1),
                    json!(1.5),
                    json!("170"),
                    json!(2147483648u64),
                ],
            ),
        ] {
            for value in values {
                let mut doc = document();
                doc["nodes"][0]["ingresses"][0]["auto_selection_metadata"][field] = value;
                let p = parse(&doc);
                assert_eq!(
                    p.nodes[0].replicas[0].auto_selection.unavailable_reason,
                    Some(UnavailableReason::InvalidMetadata)
                );
            }
        }
        let mut doc = document();
        doc["nodes"][0]["ingresses"][0]["auto_selection_metadata"]["entry_latency_threshold_ms"] =
            json!(i32::MAX);
        assert!(parse(&doc).nodes[0].replicas[0]
            .auto_selection
            .metadata
            .is_some());
    }

    #[test]
    fn local_final_preserves_distinct_exit_identities_and_requires_each_exit() {
        let mut doc = document();
        doc["nodes"][0]["exit_mode"] = json!("local_final");
        let first = &mut doc["nodes"][0]["ingresses"][0];
        first["exit_ip"] = json!("2001:db8::1");
        let mut second = first.clone();
        second["endpoint_key"] = json!("different-exit");
        second["replica_ordinal"] = json!(99);
        second["exit_ip"] = json!("2001:db8::2");
        doc["nodes"][0]["ingresses"]
            .as_array_mut()
            .unwrap()
            .push(second);
        let p = parse(&doc);
        assert_ne!(
            p.nodes[0].replicas[0].auto_selection.exit_ip,
            p.nodes[0].replicas.last().unwrap().auto_selection.exit_ip
        );
        doc["nodes"][0]["ingresses"][0]["exit_ip"] = Value::Null;
        assert_eq!(
            parse(&doc).nodes[0].replicas[0]
                .auto_selection
                .unavailable_reason,
            Some(UnavailableReason::ExitIdentityUnavailable)
        );
    }

    #[test]
    fn primary_exit_conflict_is_not_a_second_automatic_group() {
        let mut doc = document();
        doc["nodes"][0]["ingresses"][0]["exit_ip"] = json!("203.0.113.11");
        assert_eq!(
            parse(&doc).nodes[0].replicas[0]
                .auto_selection
                .unavailable_reason,
            Some(UnavailableReason::MetadataConflict)
        );
        doc["nodes"][0]["ingresses"][0]["exit_ip"] = json!("::ffff:203.0.113.10");
        assert_eq!(
            parse(&doc).nodes[0].replicas[0].auto_selection.exit_ip,
            Some("203.0.113.10".parse().unwrap())
        );
    }

    #[test]
    fn backend_reasons_and_conflicts_fail_closed_without_echoing_wire_data() {
        let mut doc = document();
        let first = &mut doc["nodes"][0]["ingresses"][0];
        first["auto_selection_metadata"] = Value::Null;
        first["auto_selection_unavailable_reason"] = json!("latency_threshold_unavailable");
        assert_eq!(
            parse(&doc).nodes[0].replicas[0]
                .auto_selection
                .unavailable_reason,
            Some(UnavailableReason::LatencyThresholdUnavailable)
        );
        doc["nodes"][0]["ingresses"][0]["auto_selection_unavailable_reason"] =
            json!("untrusted-secret-data");
        let p = parse(&doc);
        assert_eq!(
            p.nodes[0].replicas[0].auto_selection.unavailable_reason,
            Some(UnavailableReason::UnknownReason)
        );
        assert!(!format!("{:?}", p.nodes[0].replicas[0].auto_selection)
            .contains("untrusted-secret-data"));
        let mut doc = document();
        doc["nodes"][0]["ingresses"][0]["auto_selection_unavailable_reason"] =
            json!("unsupported_entry_policy");
        assert_eq!(
            parse(&doc).nodes[0].replicas[0]
                .auto_selection
                .unavailable_reason,
            Some(UnavailableReason::MetadataConflict)
        );
    }

    #[test]
    fn unknown_metadata_fields_are_ignored_but_existing_schema_validation_remains() {
        let mut doc = document();
        doc["nodes"][0]["ingresses"][0]["auto_selection_metadata"]["future"] =
            json!({"any":"value"});
        assert!(parse(&doc).nodes[0].replicas[0]
            .auto_selection
            .metadata
            .is_some());
        doc["schema_version"] = json!(2);
        assert!(parse_profile(&serde_json::to_vec(&doc).unwrap()).is_err());
        let mut doc = document();
        doc["nodes"][0]["ingresses"][0]["endpoint_key"] = json!("");
        assert!(parse_profile(&serde_json::to_vec(&doc).unwrap()).is_err());
    }
}
