//! Typed account-profile facts -> internal local-policy candidates. Not wired
//! to Client, probes, engine commands, mode persistence or UI yet.
//!
//! Scope comes from an explicit acknowledged node/endpoint, never from profile
//! order, names, endpoint suffixes or a default node. This is a projection, not
//! authorization or evidence that the engine accepted/ran this profile. A future
//! controller must match the applied revision and actual confirmed selection.
//! Auto execution must keep the exact endpoint pinned. Returning policy mode
//! from Manual to Auto is not an engine unpin: local-final legacy fallback can
//! otherwise cross exit IPs. No-qualified-candidate must never clear that pin.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use ppvpn_account::profile::auto_selection::{ExitMode, UnavailableReason};
use ppvpn_account::profile::{NodeView, ProfileView, ReplicaView};
use sha2::{Digest, Sha256};

use crate::auto_route::{Candidate, EgressGroup};

const MAX_CANDIDATES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct EntranceKey {
    pub node_id: String,
    pub endpoint_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisabledReason {
    SelectedNodeUnknown,
    SelectedEndpointUnknown,
    InvalidIdentity,
    Unavailable(UnavailableReason),
    NoEligibleCandidates,
    TooManyCandidates,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Disabled {
    pub reason: DisabledReason,
    /// Bounded stable codes only, no raw backend text or labels.
    pub skipped: BTreeMap<&'static str, usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateSet {
    pub revision: String,
    pub scope: EgressGroup,
    pub candidates: Vec<Candidate>,
    /// Opaque policy ids are resolved only through this explicit map.
    pub targets: BTreeMap<String, EntranceKey>,
    /// Exact host-confirmed input, retained even when its optional automatic
    /// metadata is unavailable; never substitute another eligible candidate.
    pub confirmed: EntranceKey,
    pub confirmed_candidate: Option<String>,
    pub skipped: BTreeMap<&'static str, usize>,
}

/// Both identities must be explicitly confirmed by the host. Primary scope
/// uses the authoritative committed node exit; local-final uses that exact
/// endpoint's exit. A missing/unknown endpoint is never replaced by a default.
pub(crate) fn build(
    profile: &ProfileView,
    selected_node: &str,
    selected_endpoint: Option<&str>,
) -> Result<CandidateSet, Disabled> {
    let mut skipped = BTreeMap::new();
    if !unique_identities(profile) {
        return Err(disabled(DisabledReason::InvalidIdentity, skipped));
    }
    let node = profile
        .nodes
        .iter()
        .find(|node| node.id == selected_node)
        .ok_or_else(|| disabled(DisabledReason::SelectedNodeUnknown, skipped.clone()))?;
    let endpoint = match selected_endpoint {
        Some(key) => Some(
            node.replicas
                .iter()
                .find(|replica| replica.endpoint_key == key)
                .ok_or_else(|| {
                    disabled(DisabledReason::SelectedEndpointUnknown, skipped.clone())
                })?,
        ),
        None => return Err(disabled(DisabledReason::SelectedEndpointUnknown, skipped)),
    };
    let scope =
        selected_scope(node, endpoint).map_err(|reason| disabled(reason, skipped.clone()))?;
    let selected_mode = node
        .auto_selection
        .exit_mode
        .expect("selected_scope checked mode");
    let mut result = CandidateSet {
        revision: profile.revision.clone(),
        scope,
        candidates: Vec::new(),
        targets: BTreeMap::new(),
        confirmed: EntranceKey {
            node_id: selected_node.into(),
            endpoint_key: selected_endpoint.expect("endpoint checked above").into(),
        },
        confirmed_candidate: None,
        skipped: BTreeMap::new(),
    };
    for node in &profile.nodes {
        for replica in &node.replicas {
            let facts = &node.auto_selection;
            let ingress = &replica.auto_selection;
            let reason = if let Some(reason) = facts.unavailable_reason {
                Some(reason.code())
            } else if facts.instance_id.as_deref() != Some(result.scope.instance.as_str()) {
                Some("other_instance")
            } else if facts.exit_mode != Some(selected_mode) {
                Some("different_exit_mode")
            } else if let Some(reason) = ingress.unavailable_reason {
                Some(reason.code())
            } else if selected_mode == ExitMode::Primary
                && facts
                    .primary_exit_ip
                    .map(|ip| exit_identity(selected_mode, ip))
                    != ingress.exit_ip.map(|ip| exit_identity(selected_mode, ip))
            {
                Some(UnavailableReason::MetadataConflict.code())
            } else if ingress.metadata.is_none() {
                Some(UnavailableReason::MetadataUnavailable.code())
            } else if ingress.exit_ip.is_none() {
                Some(UnavailableReason::ExitIdentityUnavailable.code())
            } else if ingress
                .exit_ip
                .is_some_and(|ip| exit_identity(selected_mode, ip) != result.scope.egress)
            {
                Some("different_exit_identity")
            } else {
                None
            };
            if let Some(reason) = reason {
                *skipped.entry(reason).or_default() += 1;
                continue;
            }
            let metadata = ingress.metadata.as_ref().unwrap();
            // The account DTO already validates these bounds. Fail closed also
            // for internally constructed inconsistent DTOs, without reinterpreting
            // decimal wire data or affecting the existing manual profile.
            if metadata.quota_multiplier_milli == 0
                || metadata.quota_multiplier_milli > 999_999
                || metadata.entry_latency_threshold_ms == 0
                || metadata.entry_latency_threshold_ms > i32::MAX as u32
                || metadata.tier_rank == 0
                || metadata.tier_rank > i32::MAX as u32
            {
                *skipped
                    .entry(UnavailableReason::InvalidMetadata.code())
                    .or_default() += 1;
                continue;
            }
            if result.candidates.len() == MAX_CANDIDATES {
                return Err(disabled(DisabledReason::TooManyCandidates, skipped));
            }
            let key = EntranceKey {
                node_id: node.id.clone(),
                endpoint_key: replica.endpoint_key.clone(),
            };
            let id = policy_id(&key);
            if result.targets.insert(id.clone(), key).is_some() {
                return Err(disabled(DisabledReason::InvalidIdentity, skipped));
            }
            if node.id == selected_node && selected_endpoint == Some(replica.endpoint_key.as_str())
            {
                result.confirmed_candidate = Some(id.clone());
            }
            result.candidates.push(Candidate {
                id,
                group: result.scope.clone(),
                quota_multiplier_milli: metadata.quota_multiplier_milli,
                latency_threshold_ms: metadata.entry_latency_threshold_ms,
                tier_rank: metadata.tier_rank as i32,
                eligible: true,
            });
        }
    }
    if result.candidates.is_empty() {
        return Err(disabled(DisabledReason::NoEligibleCandidates, skipped));
    }
    // No preference is encoded here: policy applies quality gates then rank.
    result.candidates.sort_by(|a, b| a.id.cmp(&b.id));
    result.skipped = skipped;
    Ok(result)
}

fn selected_scope(
    node: &NodeView,
    replica: Option<&ReplicaView>,
) -> Result<EgressGroup, DisabledReason> {
    let facts = &node.auto_selection;
    if let Some(reason) = facts.unavailable_reason {
        return Err(DisabledReason::Unavailable(reason));
    }
    let instance = facts
        .instance_id
        .as_ref()
        .filter(|id| !id.is_empty())
        .ok_or(DisabledReason::Unavailable(
            UnavailableReason::InstanceIdentityUnavailable,
        ))?;
    let mode = facts.exit_mode.ok_or(DisabledReason::Unavailable(
        UnavailableReason::ExitIdentityUnavailable,
    ))?;
    let exit = match mode {
        ExitMode::Primary => facts.primary_exit_ip,
        ExitMode::LocalFinal => {
            let replica = replica.ok_or(DisabledReason::SelectedEndpointUnknown)?;
            if let Some(reason) = replica.auto_selection.unavailable_reason {
                return Err(DisabledReason::Unavailable(reason));
            }
            replica.auto_selection.exit_ip
        }
    }
    .ok_or(DisabledReason::Unavailable(
        UnavailableReason::ExitIdentityUnavailable,
    ))?;
    Ok(EgressGroup {
        instance: instance.clone(),
        egress: exit_identity(mode, exit),
    })
}

fn exit_identity(mode: ExitMode, address: IpAddr) -> String {
    let address = match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        _ => address,
    };
    let mode = match mode {
        ExitMode::Primary => "primary",
        ExitMode::LocalFinal => "local_final",
    };
    format!("{mode}:{address}")
}

fn policy_id(key: &EntranceKey) -> String {
    let mut hash = Sha256::new();
    for part in [&key.node_id, &key.endpoint_key] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("entry-{:x}", hash.finalize())
}

fn unique_identities(profile: &ProfileView) -> bool {
    let mut nodes = BTreeSet::new();
    let mut endpoints = BTreeSet::new();
    profile.nodes.iter().all(|node| {
        !node.id.is_empty()
            && node.id.len() <= 128
            && nodes.insert(&node.id)
            && node.replicas.iter().all(|replica| {
                !replica.endpoint_key.is_empty()
                    && replica.endpoint_key.len() <= 128
                    && endpoints.insert(&replica.endpoint_key)
            })
    })
}

fn disabled(reason: DisabledReason, skipped: BTreeMap<&'static str, usize>) -> Disabled {
    Disabled { reason, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_route::{Config, Epoch, Health, Policy, SampleResult};
    use ppvpn_account::profile::parse_profile;
    use serde_json::{json, Value};

    const INSTANCE: &str = "11111111-1111-4111-8111-111111111111";
    const OTHER: &str = "22222222-2222-4222-8222-222222222222";

    fn ingress(id: &str, rank: u32, multiplier: &str, threshold: u32) -> Value {
        json!({"endpoint_key":id,"replica_ordinal":0,"protocol":"anytls","tier_rank":rank,
            "auto_selection_metadata":{"entry_tier_id":rank.to_string(),"entry_tier_key":"entry-key",
                "quota_multiplier":multiplier,"entry_latency_threshold_ms":threshold}})
    }
    fn node(id: &str, instance: &str, mode: &str, exit: &str, ingresses: Vec<Value>) -> Value {
        json!({"id":id,"name":"must-not-be-used-as-identity","entry_key":"not-a-price",
            "instance_id":instance,"exit_mode":mode,"exit":{"ip":exit},"ingresses":ingresses})
    }
    fn profile(nodes: Vec<Value>) -> ProfileView {
        parse_profile(&serde_json::to_vec(&json!({"schema_version":1,"revision":"revision-1",
            "nodes":nodes,"selection":{"mode":"manual","default_node_id":"not-a-confirmed-selection"}})).unwrap()).unwrap()
    }
    fn primary(id: &str, endpoint: &str, rank: u32, multiplier: &str) -> Value {
        node(
            id,
            INSTANCE,
            "primary",
            "203.0.113.10",
            vec![ingress(endpoint, rank, multiplier, 200)],
        )
    }
    /// Credential-free public contract fixture for automatic route selection.
    /// The independent full-fixture check verifies ProfileView equality.
    #[test]
    fn backend_20261115_all_six_ingresses_qualify_with_exact_public_facts() {
        let p = parse_profile(include_bytes!(
            "testdata/backend-auto-contract-public-20261115.json"
        ))
        .unwrap();
        assert_eq!(p.nodes.len(), 3);
        assert_eq!(
            p.nodes
                .iter()
                .map(|node| node.replicas.len())
                .sum::<usize>(),
            6
        );
        for node in &p.nodes {
            for ingress in &node.replicas {
                let set = build(&p, &node.id, Some(&ingress.endpoint_key)).unwrap();
                let id = set
                    .confirmed_candidate
                    .as_ref()
                    .expect("backend supplied exact eligible facts");
                let candidate = set
                    .candidates
                    .iter()
                    .find(|candidate| &candidate.id == id)
                    .unwrap();
                assert!(candidate.eligible);
                assert_eq!(candidate.quota_multiplier_milli, 750);
                assert_eq!(candidate.latency_threshold_ms, 175);
                assert_eq!(
                    candidate.tier_rank,
                    ingress.auto_selection.metadata.as_ref().unwrap().tier_rank as i32
                );
                assert_eq!(
                    set.scope.instance,
                    node.auto_selection.instance_id.as_deref().unwrap()
                );
                for target in set.targets.values() {
                    let other = p
                        .nodes
                        .iter()
                        .find(|node| node.id == target.node_id)
                        .unwrap();
                    assert_eq!(
                        other.auto_selection.instance_id,
                        node.auto_selection.instance_id
                    );
                }
            }
        }
    }
    #[test]
    fn backend_20261115_bad_optional_facts_keep_manual_profile_and_disable_auto_candidate() {
        let original: Value = serde_json::from_slice(include_bytes!(
            "testdata/backend-auto-contract-public-20261115.json"
        ))
        .unwrap();
        let mut missing = original.clone();
        missing["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("instance_id");
        let p = parse_profile(&serde_json::to_vec(&missing).unwrap()).unwrap();
        assert_eq!(p.nodes.len(), 3);
        assert!(build(
            &p,
            &p.nodes[0].id,
            Some(&p.nodes[0].replicas[0].endpoint_key)
        )
        .is_err());
        let mut noncanonical = original;
        noncanonical["nodes"][0]["ingresses"][0]["auto_selection_metadata"]["quota_multiplier"] =
            json!("1.000");
        let p = parse_profile(&serde_json::to_vec(&noncanonical).unwrap()).unwrap();
        assert_eq!(p.nodes.len(), 3);
        assert!(p.nodes[0].replicas[0].auto_selection.metadata.is_none());
        let set = build(
            &p,
            &p.nodes[0].id,
            Some(&p.nodes[0].replicas[0].endpoint_key),
        )
        .unwrap();
        assert!(set.confirmed_candidate.is_none());
    }

    fn config() -> Config {
        Config {
            max_quota_multiplier_milli: 999_999,
            fresh_for_ms: 10_000,
            fail_after: 2,
            recover_after: 3,
            min_dwell_ms: 5_000,
            min_latency_improvement_ms: 20,
            max_candidates: 256,
            max_in_flight: 2,
            max_per_round: 4,
            round_timeout_ms: 500,
            min_round_interval_ms: 1_000,
        }
    }

    #[test]
    fn primary_projection_crosses_logical_entries_but_keeps_exact_instance_and_exit() {
        let p = profile(vec![
            primary("a", "a1", 1, "1"),
            primary("b", "b1", 2, "0.5"),
            node(
                "other-instance",
                OTHER,
                "primary",
                "203.0.113.10",
                vec![ingress("c1", 1, "1", 200)],
            ),
            node(
                "other-exit",
                INSTANCE,
                "primary",
                "203.0.113.11",
                vec![ingress("d1", 1, "1", 200)],
            ),
        ]);
        let set = build(&p, "a", Some("a1")).unwrap();
        assert_eq!(set.revision, "revision-1");
        assert_eq!(set.scope.instance, INSTANCE);
        assert_eq!(set.scope.egress, "primary:203.0.113.10");
        assert_eq!(set.candidates.len(), 2);
        assert_eq!(set.skipped["other_instance"], 1);
        assert_eq!(set.skipped["different_exit_identity"], 1);
        assert_eq!(
            set.targets[set.confirmed_candidate.as_ref().unwrap()].endpoint_key,
            "a1"
        );
        assert!(set
            .targets
            .values()
            .any(|target| target.node_id == "b" && target.endpoint_key == "b1"));
    }

    #[test]
    fn local_final_uses_confirmed_endpoint_and_never_silently_changes_exit_ip() {
        let mut first = ingress("local-1", 1, "1", 200);
        first["exit_ip"] = json!("203.0.113.20");
        let mut same = ingress("same-ip", 2, "0.5", 200);
        same["exit_ip"] = json!("203.0.113.20");
        let mut different = ingress("local-2", 1, "1", 200);
        different["exit_ip"] = json!("203.0.113.21");
        let p = profile(vec![
            node(
                "a",
                INSTANCE,
                "local_final",
                "203.0.113.99",
                vec![first, different],
            ),
            node("b", INSTANCE, "local_final", "203.0.113.98", vec![same]),
            primary("primary", "p1", 1, "1"),
        ]);
        let set = build(&p, "a", Some("local-1")).unwrap();
        assert_eq!(set.scope.egress, "local_final:203.0.113.20");
        assert_eq!(set.candidates.len(), 2);
        assert_eq!(set.skipped["different_exit_identity"], 1);
        assert_eq!(set.skipped["different_exit_mode"], 1);
        let other = build(&p, "a", Some("local-2")).unwrap();
        assert_eq!(other.candidates.len(), 1);
        assert_eq!(other.scope.egress, "local_final:203.0.113.21");
    }

    #[test]
    fn unknown_or_unconfirmed_selection_never_uses_profile_default_or_first_replica() {
        let p = profile(vec![primary("a", "a1", 1, "1")]);
        assert_eq!(
            build(&p, "missing", Some("a1")).unwrap_err().reason,
            DisabledReason::SelectedNodeUnknown
        );
        assert_eq!(
            build(&p, "a", Some("missing")).unwrap_err().reason,
            DisabledReason::SelectedEndpointUnknown
        );
        assert_eq!(
            build(&p, "a", None).unwrap_err().reason,
            DisabledReason::SelectedEndpointUnknown
        );
    }

    #[test]
    fn missing_or_unavailable_metadata_does_not_remove_manual_endpoints() {
        let mut missing = ingress("missing", 1, "1", 200);
        missing
            .as_object_mut()
            .unwrap()
            .remove("auto_selection_metadata");
        let mut unsupported = ingress("unsupported", 1, "1", 200);
        unsupported
            .as_object_mut()
            .unwrap()
            .remove("auto_selection_metadata");
        unsupported["auto_selection_unavailable_reason"] = json!("unsupported_entry_policy");
        let p = profile(vec![node(
            "a",
            INSTANCE,
            "primary",
            "203.0.113.10",
            vec![missing, unsupported, ingress("usable", 2, "1", 200)],
        )]);
        let set = build(&p, "a", Some("missing")).unwrap();
        assert_eq!(
            p.nodes[0].replicas.len(),
            3,
            "all remain available to manual decoder"
        );
        assert_eq!(set.candidates.len(), 1);
        assert_eq!(set.skipped["metadata_unavailable"], 1);
        assert_eq!(set.skipped["unsupported_entry_policy"], 1);
        assert!(
            set.confirmed_candidate.is_none(),
            "never pretend another endpoint was confirmed"
        );
    }

    #[test]
    fn malformed_optional_and_unknown_reason_are_disabled_without_echoing_raw_text() {
        let secret = "untrusted raw reason with address and token";
        let mut unknown = ingress("unknown", 1, "1", 200);
        unknown
            .as_object_mut()
            .unwrap()
            .remove("auto_selection_metadata");
        unknown["auto_selection_unavailable_reason"] = json!(secret);
        let mut malformed = ingress("malformed", 1, "1", 200);
        malformed["auto_selection_metadata"] = json!(["bad"]);
        let p = profile(vec![node(
            "a",
            INSTANCE,
            "primary",
            "203.0.113.10",
            vec![unknown, malformed, ingress("ok", 2, "1", 200)],
        )]);
        let set = build(&p, "a", Some("ok")).unwrap();
        assert_eq!(set.candidates.len(), 1);
        assert_eq!(set.skipped["unknown_reason"], 1);
        assert_eq!(set.skipped["invalid_metadata"], 1);
        assert!(!format!("{:?}", set.skipped).contains(secret));
    }

    #[test]
    fn primary_explicit_exit_conflict_and_missing_facts_never_join_scope() {
        let mut conflicting = ingress("conflict", 1, "1", 200);
        conflicting["exit_ip"] = json!("203.0.113.11");
        let mut no_instance = primary("no-instance", "ni", 1, "1");
        no_instance.as_object_mut().unwrap().remove("instance_id");
        let p = profile(vec![
            node(
                "a",
                INSTANCE,
                "primary",
                "203.0.113.10",
                vec![ingress("ok", 2, "1", 200), conflicting],
            ),
            no_instance,
        ]);
        let set = build(&p, "a", Some("ok")).unwrap();
        assert_eq!(set.candidates.len(), 1);
        assert_eq!(set.skipped["metadata_conflict"], 1);
        assert_eq!(set.skipped["instance_identity_unavailable"], 1);
        assert_eq!(
            build(&p, "no-instance", Some("ni")).unwrap_err().reason,
            DisabledReason::Unavailable(UnavailableReason::InstanceIdentityUnavailable)
        );
    }

    #[test]
    fn mapped_ipv4_and_equivalent_ipv6_have_one_normalized_boundary() {
        let p = profile(vec![
            primary("a", "a1", 1, "1"),
            node(
                "b",
                INSTANCE,
                "primary",
                "::ffff:203.0.113.10",
                vec![ingress("b1", 2, "1", 200)],
            ),
        ]);
        assert_eq!(build(&p, "a", Some("a1")).unwrap().candidates.len(), 2);
        let p = profile(vec![
            node(
                "a",
                INSTANCE,
                "primary",
                "2001:db8::1",
                vec![ingress("a1", 1, "1", 200)],
            ),
            node(
                "b",
                INSTANCE,
                "primary",
                "2001:0db8:0:0:0:0:0:1",
                vec![ingress("b1", 2, "1", 200)],
            ),
        ]);
        assert_eq!(build(&p, "a", Some("a1")).unwrap().candidates.len(), 2);
    }

    #[test]
    fn exact_multiplier_and_per_entrance_threshold_are_projected_without_labels_or_sorting() {
        let p = profile(vec![node(
            "a",
            INSTANCE,
            "primary",
            "203.0.113.10",
            vec![
                ingress("a1", 9, "0.001", 123),
                ingress("a2", 1, "999.999", 987),
            ],
        )]);
        let set = build(&p, "a", Some("a1")).unwrap();
        for candidate in &set.candidates {
            let endpoint = &set.targets[&candidate.id].endpoint_key;
            if endpoint == "a1" {
                assert_eq!(
                    (
                        candidate.quota_multiplier_milli,
                        candidate.latency_threshold_ms,
                        candidate.tier_rank
                    ),
                    (1, 123, 9)
                );
            } else {
                assert_eq!(
                    (
                        candidate.quota_multiplier_milli,
                        candidate.latency_threshold_ms,
                        candidate.tier_rank
                    ),
                    (999_999, 987, 1)
                );
            }
        }
    }

    #[test]
    fn actual_decoder_adapter_and_policy_together_prefer_qualified_rank_not_multiplier_or_fastest()
    {
        let p = profile(vec![
            primary("earlier", "e1", 1, "2"),
            primary("later", "l1", 2, "0.5"),
        ]);
        let set = build(&p, "earlier", Some("e1")).unwrap();
        let mut policy = Policy::new(
            config(),
            Epoch {
                session: 1,
                network: 1,
                profile: 1,
            },
            set.scope,
            set.candidates,
        )
        .unwrap();
        policy.begin_round(0).unwrap();
        let probes = policy.plan_probes(0).start;
        assert_eq!(probes.len(), 2);
        for probe in probes {
            let target = &set.targets[&probe.candidate.id];
            let latency_ms = if target.node_id == "earlier" { 190 } else { 1 };
            assert_eq!(
                policy.finish_probe(&probe.token, Health::Available { latency_ms }, 0),
                SampleResult::Accepted
            );
        }
        let proposal = policy.recommend(0).change.unwrap();
        assert_eq!(set.targets[&proposal.candidate].node_id, "earlier");
        assert_eq!(set.targets[&proposal.candidate].endpoint_key, "e1");
    }

    #[test]
    fn candidate_specific_quality_gate_can_escalate_to_later_rank_in_combined_pipeline() {
        let mut early = primary("earlier", "e1", 1, "2");
        early["ingresses"][0]["auto_selection_metadata"]["entry_latency_threshold_ms"] = json!(100);
        let p = profile(vec![early, primary("later", "l1", 2, "0.5")]);
        let set = build(&p, "earlier", Some("e1")).unwrap();
        let mut policy = Policy::new(
            config(),
            Epoch {
                session: 1,
                network: 1,
                profile: 1,
            },
            set.scope,
            set.candidates,
        )
        .unwrap();
        policy.begin_round(0).unwrap();
        for probe in policy.plan_probes(0).start {
            let latency_ms = if set.targets[&probe.candidate.id].node_id == "earlier" {
                101
            } else {
                50
            };
            policy.finish_probe(&probe.token, Health::Available { latency_ms }, 0);
        }
        let proposal = policy.recommend(0).change.unwrap();
        assert_eq!(set.targets[&proposal.candidate].node_id, "later");
    }

    #[test]
    fn stable_opaque_ids_do_not_depend_on_profile_order_labels_or_decimal_changes() {
        let mut p = profile(vec![
            primary("a", "a1", 1, "1"),
            primary("b", "b1", 2, "0.5"),
        ]);
        let first = build(&p, "a", Some("a1")).unwrap();
        p.nodes.reverse();
        p.nodes[0].name = "changed label is not identity".into();
        p.nodes[0].replicas[0]
            .auto_selection
            .metadata
            .as_mut()
            .unwrap()
            .quota_multiplier_milli = 750;
        let second = build(&p, "a", Some("a1")).unwrap();
        assert_eq!(first.targets, second.targets);
        let x = EntranceKey {
            node_id: "a:b".into(),
            endpoint_key: "c".into(),
        };
        let y = EntranceKey {
            node_id: "a".into(),
            endpoint_key: "b:c".into(),
        };
        assert_ne!(
            policy_id(&x),
            policy_id(&y),
            "length framed hash prevents delimiter aliases"
        );
        assert!(first
            .candidates
            .iter()
            .all(|candidate| candidate.id.len() <= 128));
    }

    #[test]
    fn duplicate_manual_identities_disable_auto_instead_of_merging_routes() {
        let p = profile(vec![
            primary("a", "same", 1, "1"),
            primary("b", "same", 2, "1"),
        ]);
        assert_eq!(
            build(&p, "a", Some("same")).unwrap_err().reason,
            DisabledReason::InvalidIdentity
        );
        let p = profile(vec![primary("a", "a1", 1, "1"), primary("a", "b1", 2, "1")]);
        assert_eq!(
            build(&p, "a", Some("a1")).unwrap_err().reason,
            DisabledReason::InvalidIdentity
        );
    }

    #[test]
    fn candidate_ceiling_disables_auto_without_silent_rank_truncation() {
        let nodes = (0..257)
            .map(|i| primary(&format!("n{i}"), &format!("e{i}"), 1, "1"))
            .collect();
        let p = profile(nodes);
        assert_eq!(
            build(&p, "n0", Some("e0")).unwrap_err().reason,
            DisabledReason::TooManyCandidates
        );
        assert_eq!(p.nodes.len(), 257, "manual profile is unchanged");
    }

    #[test]
    fn all_optional_metadata_unusable_returns_explicit_disabled_reason_not_empty_auto() {
        let mut n = primary("a", "a1", 1, "1");
        n["ingresses"][0]["auto_selection_metadata"] = json!(null);
        let p = profile(vec![n]);
        let disabled = build(&p, "a", Some("a1")).unwrap_err();
        assert_eq!(disabled.reason, DisabledReason::NoEligibleCandidates);
        assert_eq!(disabled.skipped["metadata_unavailable"], 1);
    }
}
