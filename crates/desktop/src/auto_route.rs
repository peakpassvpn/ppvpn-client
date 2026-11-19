//! Internal automatic entrance policy, not wired to Client, IPC, or UI yet.
//!
//! Local first-gray subset of automatic route selection: quality gate, rank preference,
//! then within-tier quality. No server baseline/ranking/reporting implementation.
//!
//! Callers provide authorized candidates, explicit consumption constraints and stable
//! instance/egress identities. No backend fields or labels are interpreted here.
//! A recommendation does not change routing: acknowledge only a successful host
//! selection, serialized with epoch/mode changes by the future adapter.
//!
//! Probe cancellation is two phase: invalidation returns tokens, but they still
//! consume the global in-flight budget until their futures actually finish or
//! the host confirms they were dropped. Thus reconnects cannot multiply probes.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EgressGroup {
    pub instance: String,
    pub egress: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub id: String,
    pub group: EgressGroup,
    /// Exact quota multiplier in thousandths. Used only for an explicit user
    /// constraint, never as a preference, price, or inferred line composition.
    pub quota_multiplier_milli: u32,
    /// Candidate's authoritative regional threshold, never a global default.
    pub latency_threshold_ms: u32,
    /// Preference only, never interpreted as price.
    pub tier_rank: i32,
    pub eligible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Epoch {
    pub session: u64,
    pub network: u64,
    pub profile: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub max_quota_multiplier_milli: u32,
    pub fresh_for_ms: u64,
    pub fail_after: u16,
    pub recover_after: u16,
    pub min_dwell_ms: u64,
    pub min_latency_improvement_ms: u32,
    pub max_candidates: usize,
    pub max_in_flight: usize,
    pub max_per_round: usize,
    pub round_timeout_ms: u64,
    pub min_round_interval_ms: u64,
}

impl Config {
    fn validate(&self) -> Result<(), Error> {
        // Hard ceilings also apply to a mistakenly generous adapter config.
        if self.max_candidates == 0
            || self.max_candidates > 256
            || self.max_in_flight == 0
            || self.max_in_flight > 4
            || self.max_per_round < self.max_in_flight
            || self.max_per_round > 16
            || self.fresh_for_ms == 0
            || self.fail_after == 0
            || self.recover_after == 0
            || self.round_timeout_ms == 0
            || self.round_timeout_ms > 30_000
            || self.min_round_interval_ms < 1_000
        {
            return Err(Error::InvalidConfig);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidConfig,
    InvalidCandidates,
    Paused,
    ManualMode,
    TooSoon,
    Exhausted,
    ScopeChangeRequiresNewSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Health {
    /// Combined candidate health and entrance quality from the adapter.
    /// latency_ms is this entrance's RTT, not total HTTP duration. Bare TCP
    /// reachability does not by itself prove end-to-end proxy availability.
    Available {
        latency_ms: u32,
    },
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProbeToken {
    serial: u64,
    round: u64,
    epoch: Epoch,
    inventory: u64,
    candidate: String,
    issued_at: u64,
    deadline: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Probe {
    pub token: ProbeToken,
    pub candidate: Candidate,
}

#[derive(Default, Debug)]
pub(crate) struct ProbePlan {
    pub start: Vec<Probe>,
    pub cancel: Vec<ProbeToken>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SampleResult {
    Accepted,
    Stale,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    Paused,
    NoQualifiedCandidate,
    InitialChoice,
    CurrentHealthy,
    CurrentUnhealthy,
    CoolingDown,
    BelowImprovement,
    RankImprovement,
    LatencyImprovement,
    ManualOverride,
    ManualUnverified,
    ManualUnhealthy,
    ManualOutsideThreshold,
    ManualIneligible,
    EgressGroupLocked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proposal {
    revision: u64,
    epoch: Epoch,
    from: Option<String>,
    pub candidate: String,
}

impl Proposal {
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decision {
    /// Actual acknowledged selection, even when it is no longer qualified.
    /// A warning does not itself disconnect or silently change the host.
    pub current: Option<String>,
    pub reason: Reason,
    pub change: Option<Proposal>,
}

#[derive(Default)]
struct Observation {
    healthy: bool,
    recovery_required: bool,
    successes: u16,
    failures: u16,
    last_success: Option<(u64, u32)>,
    last_health: Option<Health>,
}

impl Observation {
    fn record(&mut self, health: Health, now: u64, config: &Config, threshold: u32) {
        self.last_health = Some(health);
        match health {
            Health::Available { latency_ms } if latency_ms <= threshold => {
                self.failures = 0;
                self.successes = self.successes.saturating_add(1);
                self.last_success = Some((now, latency_ms));
                if !self.recovery_required || self.successes >= config.recover_after {
                    self.healthy = true;
                    self.recovery_required = false;
                }
            }
            Health::Unavailable | Health::Available { .. } => {
                // A confirmed quality downgrade also counts when connections
                // still work. One spike alone does not discard the last good
                // fresh sample; repeated breaches do, and recovery is stricter.
                self.successes = 0;
                self.failures = self.failures.saturating_add(1);
                if self.failures >= config.fail_after {
                    self.healthy = false;
                    self.recovery_required = true;
                }
            }
        }
    }

    fn latency(&self, now: u64, config: &Config, threshold: u32) -> Option<u32> {
        let (at, latency) = self.last_success?;
        let age = now.checked_sub(at)?;
        (self.healthy && age <= config.fresh_for_ms && latency <= threshold).then_some(latency)
    }
}

struct Flight {
    token: ProbeToken,
    canceled: bool,
}
struct Round {
    id: u64,
    deadline: u64,
    started_at: u64,
    started: BTreeSet<String>,
}

pub(crate) struct Policy {
    config: Config,
    epoch: Epoch,
    candidates: BTreeMap<String, Candidate>,
    observations: BTreeMap<String, Observation>,
    current: Option<String>,
    locked_group: EgressGroup,
    manual: Option<String>,
    paused: bool,
    last_switch: Option<u64>,
    last_round: Option<u64>,
    round: Option<Round>,
    flights: BTreeMap<u64, Flight>,
    cursor: usize,
    serial: u64,
    inventory: u64,
    revision: u64,
}

impl Policy {
    pub fn new(
        config: Config,
        epoch: Epoch,
        group: EgressGroup,
        candidates: Vec<Candidate>,
    ) -> Result<Self, Error> {
        config.validate()?;
        if !valid_group(&group) {
            return Err(Error::InvalidCandidates);
        }
        let candidates = inventory(&config, candidates)?;
        Ok(Self {
            config,
            epoch,
            candidates,
            observations: BTreeMap::new(),
            current: None,
            locked_group: group,
            manual: None,
            paused: false,
            last_switch: None,
            last_round: None,
            round: None,
            flights: BTreeMap::new(),
            cursor: 0,
            serial: 0,
            inventory: 0,
            revision: 0,
        })
    }

    fn invalidate(&mut self) -> Vec<ProbeToken> {
        self.revision += 1;
        self.round = None;
        self.flights
            .values_mut()
            .filter_map(|flight| {
                if flight.canceled {
                    return None;
                }
                flight.canceled = true;
                Some(flight.token.clone())
            })
            .collect()
    }

    pub fn replace_candidates(
        &mut self,
        candidates: Vec<Candidate>,
    ) -> Result<Vec<ProbeToken>, Error> {
        let candidates = inventory(&self.config, candidates)?;
        if candidates == self.candidates {
            return Ok(Vec::new());
        }
        let cancel = self.invalidate();
        self.inventory += 1;
        self.candidates = candidates;
        self.observations.clear();
        // Removed manual choices remain manual/unavailable, never auto fallback.
        Ok(cancel)
    }

    pub fn set_epoch(&mut self, epoch: Epoch) -> Vec<ProbeToken> {
        if epoch == self.epoch {
            return Vec::new();
        }
        let cancel = self.invalidate();
        if epoch.session != self.epoch.session {
            self.current = None;
            self.manual = None;
            self.last_switch = None;
        }
        self.epoch = epoch;
        self.observations.clear();
        cancel
    }

    /// An explicit new user/session selection may replace the instance/exit
    /// boundary. Preserve outstanding probe accounting across that replacement.
    pub fn reset_scope(
        &mut self,
        epoch: Epoch,
        group: EgressGroup,
    ) -> Result<Vec<ProbeToken>, Error> {
        if !valid_group(&group) {
            return Err(Error::InvalidCandidates);
        }
        if epoch.session == self.epoch.session {
            return Err(Error::ScopeChangeRequiresNewSession);
        }
        let cancel = self.set_epoch(epoch);
        self.locked_group = group;
        Ok(cancel)
    }

    pub fn set_manual(&mut self, candidate: Option<String>) -> Vec<ProbeToken> {
        if candidate == self.manual {
            return Vec::new();
        }
        let cancel = self.invalidate();
        self.manual = candidate;
        cancel
    }

    pub fn set_paused(&mut self, paused: bool) -> Vec<ProbeToken> {
        if paused == self.paused {
            return Vec::new();
        }
        let cancel = self.invalidate();
        self.paused = paused;
        self.observations.clear();
        cancel
    }

    pub fn round_interval_ms(&self) -> u64 {
        self.config.min_round_interval_ms
    }

    pub fn round_candidate_limit(&self) -> usize {
        self.config.max_per_round
    }

    pub fn round_timeout_ms(&self) -> u64 {
        self.config.round_timeout_ms
    }

    /// The rate limit survives mode, network, profile and background churn.
    pub fn begin_round(&mut self, now: u64) -> Result<Vec<ProbeToken>, Error> {
        if self.paused {
            return Err(Error::Paused);
        }
        if self.manual.is_some() {
            return Err(Error::ManualMode);
        }
        if self.last_round.is_some_and(|at| {
            now.checked_sub(at)
                .is_none_or(|age| age < self.config.min_round_interval_ms)
        }) {
            return Err(Error::TooSoon);
        }
        let deadline = now
            .checked_add(self.config.round_timeout_ms)
            .ok_or(Error::Exhausted)?;
        let serial = self.serial.checked_add(1).ok_or(Error::Exhausted)?;
        let cancel = self.invalidate();
        self.serial = serial;
        self.round = Some(Round {
            id: serial,
            deadline,
            started_at: now,
            started: BTreeSet::new(),
        });
        self.last_round = Some(now);
        Ok(cancel)
    }

    fn allowed(&self, candidate: &Candidate) -> bool {
        candidate.eligible && self.locked_group == candidate.group
    }

    pub fn plan_probes(&mut self, now: u64) -> ProbePlan {
        let mut plan = ProbePlan::default();
        if self.paused || self.manual.is_some() {
            return plan;
        }
        let Some(round) = &self.round else {
            return plan;
        };
        if now < round.started_at {
            return plan;
        }
        if now > round.deadline {
            plan.cancel = self.invalidate();
            return plan;
        }
        let ids: Vec<_> = self.candidates.keys().cloned().collect();
        if ids.is_empty() {
            return plan;
        }
        // With room for alternatives, prioritize active-path freshness. A
        // single-slot round must rotate everyone instead: always prepending
        // current there would permanently starve all alternatives.
        let mut order = Vec::with_capacity(ids.len());
        let prioritize_current = self.config.max_per_round > 1;
        if let Some(current) = self
            .current
            .as_ref()
            .filter(|id| prioritize_current && self.candidates.contains_key(*id))
        {
            order.push((current.clone(), None));
        }
        let start = self.cursor % ids.len();
        for offset in 0..ids.len() {
            let index = (start + offset) % ids.len();
            if !prioritize_current || self.current.as_ref() != Some(&ids[index]) {
                order.push((ids[index].clone(), Some((index + 1) % ids.len())));
            }
        }
        for (id, cursor) in &order {
            if self.flights.len() >= self.config.max_in_flight {
                break;
            }
            let round = self.round.as_mut().expect("round exists");
            if round.started.len() >= self.config.max_per_round {
                break;
            }
            if let Some(cursor) = cursor {
                self.cursor = *cursor;
            }
            let candidate = &self.candidates[id];
            if !self.allowed(candidate)
                || candidate.quota_multiplier_milli > self.config.max_quota_multiplier_milli
                || self.round.as_ref().unwrap().started.contains(id)
                || self.flights.values().any(|f| &f.token.candidate == id)
            {
                continue;
            }
            let Some(serial) = self.serial.checked_add(1) else {
                break;
            };
            self.serial = serial;
            let round = self.round.as_mut().unwrap();
            round.started.insert(id.clone());
            let token = ProbeToken {
                serial,
                round: round.id,
                epoch: self.epoch,
                inventory: self.inventory,
                candidate: id.clone(),
                issued_at: now,
                deadline: round.deadline,
            };
            self.flights.insert(
                serial,
                Flight {
                    token: token.clone(),
                    canceled: false,
                },
            );
            plan.start.push(Probe {
                token,
                candidate: candidate.clone(),
            });
        }
        plan
    }

    /// A cancellation acknowledgement means the actual probe future was dropped.
    pub fn acknowledge_cancel(&mut self, token: &ProbeToken) -> bool {
        if !self
            .flights
            .get(&token.serial)
            .is_some_and(|f| f.canceled && f.token == *token)
        {
            return false;
        }
        self.flights.remove(&token.serial);
        true
    }

    pub fn finish_probe(&mut self, token: &ProbeToken, health: Health, now: u64) -> SampleResult {
        if !self
            .flights
            .get(&token.serial)
            .is_some_and(|f| f.token == *token)
        {
            return SampleResult::Unknown;
        }
        let flight = self.flights.remove(&token.serial).unwrap();
        if flight.canceled
            || token.epoch != self.epoch
            || token.inventory != self.inventory
            || self.round.as_ref().is_none_or(|r| r.id != token.round)
            || now < token.issued_at
            || now > token.deadline
        {
            return SampleResult::Stale;
        }
        let threshold = self.candidates[&token.candidate].latency_threshold_ms;
        self.observations
            .entry(token.candidate.clone())
            .or_default()
            .record(health, now, &self.config, threshold);
        self.revision += 1;
        SampleResult::Accepted
    }

    fn decision(&self, candidate: Option<&Candidate>, reason: Reason) -> Decision {
        Decision {
            current: self.current.clone(),
            reason,
            change: candidate
                .filter(|c| self.current.as_deref() != Some(c.id.as_str()))
                .map(|c| Proposal {
                    revision: self.revision,
                    epoch: self.epoch,
                    from: self.current.clone(),
                    candidate: c.id.clone(),
                }),
        }
    }

    fn qualified_latency(&self, candidate: &Candidate, now: u64) -> Option<u32> {
        if !self.allowed(candidate)
            || candidate.quota_multiplier_milli > self.config.max_quota_multiplier_milli
        {
            return None;
        }
        self.observations.get(&candidate.id)?.latency(
            now,
            &self.config,
            candidate.latency_threshold_ms,
        )
    }

    pub fn recommend(&self, now: u64) -> Decision {
        if self.paused {
            return self.decision(None, Reason::Paused);
        }
        if let Some(id) = &self.manual {
            let Some(candidate) = self.candidates.get(id) else {
                return self.decision(None, Reason::ManualIneligible);
            };
            if !candidate.eligible {
                return self.decision(None, Reason::ManualIneligible);
            }
            if !self.allowed(candidate) {
                return self.decision(None, Reason::EgressGroupLocked);
            }
            // Manual override bypasses multiplier/latency/health gates, never eligibility
            // or the active exit identity. Unpin explicitly returns to Auto.
            let reason = match self.observations.get(id) {
                None => Reason::ManualUnverified,
                Some(o)
                    if matches!(o.last_health, Some(Health::Available { latency_ms })
                    if latency_ms > candidate.latency_threshold_ms) =>
                {
                    Reason::ManualOutsideThreshold
                }
                Some(o) if !o.healthy => Reason::ManualUnhealthy,
                Some(o)
                    if o.last_success.is_none_or(|(at, _)| {
                        now.checked_sub(at)
                            .is_none_or(|age| age > self.config.fresh_for_ms)
                    }) =>
                {
                    Reason::ManualUnverified
                }
                Some(o)
                    if o.latency(now, &self.config, candidate.latency_threshold_ms)
                        .is_none() =>
                {
                    Reason::ManualOutsideThreshold
                }
                _ => Reason::ManualOverride,
            };
            return self.decision(Some(candidate), reason);
        }
        let best = self
            .candidates
            .values()
            .filter_map(|c| self.qualified_latency(c, now).map(|latency| (c, latency)))
            .min_by_key(|(c, latency)| {
                (
                    c.tier_rank,
                    *latency,
                    self.current.as_deref() != Some(c.id.as_str()),
                    c.id.as_str(),
                )
            });
        let Some((best, best_latency)) = best else {
            return self.decision(None, Reason::NoQualifiedCandidate);
        };
        let Some(current) = self.current.as_ref().and_then(|id| self.candidates.get(id)) else {
            return self.decision(Some(best), Reason::InitialChoice);
        };
        if current.id == best.id {
            return self.decision(None, Reason::CurrentHealthy);
        }
        let Some(current_latency) = self.qualified_latency(current, now) else {
            return self.decision(Some(best), Reason::CurrentUnhealthy);
        };
        if self.last_switch.is_some_and(|at| {
            now.checked_sub(at)
                .is_none_or(|age| age < self.config.min_dwell_ms)
        }) {
            return self.decision(None, Reason::CoolingDown);
        }
        let reason = if best.tier_rank < current.tier_rank {
            Reason::RankImprovement
        } else if best.tier_rank == current.tier_rank
            && best_latency < current_latency
            && current_latency - best_latency >= self.config.min_latency_improvement_ms
        {
            Reason::LatencyImprovement
        } else {
            return self.decision(None, Reason::BelowImprovement);
        };
        self.decision(Some(best), reason)
    }

    /// Seed only a verified engine selection, without inventing probe success
    /// or fabricating a Manual action/selection proposal. A different current
    /// selection requires explicit reconciliation, not a late seed overwrite.
    pub fn seed_confirmed(&mut self, candidate: &str, epoch: Epoch, now: u64) -> bool {
        if epoch != self.epoch || self.paused || self.manual.is_some() {
            return false;
        }
        let Some(candidate) = self.candidates.get(candidate).filter(|c| self.allowed(c)) else {
            return false;
        };
        if let Some(current) = &self.current {
            return current == &candidate.id;
        }
        self.current = Some(candidate.id.clone());
        self.last_switch = Some(now);
        self.revision += 1;
        true
    }

    /// Called after the host applied this proposal successfully. A stale
    /// acknowledgement cannot overwrite a newer selection/mode/epoch. The host
    /// adapter must also serialize its actual apply and reconcile a stale result.
    pub fn acknowledge_selection(&mut self, proposal: &Proposal, now: u64) -> bool {
        if self.recommend(now).change.as_ref() != Some(proposal) {
            return false;
        }
        let Some(candidate) = self.candidates.get(&proposal.candidate) else {
            return false;
        };
        self.current = Some(candidate.id.clone());
        self.last_switch = Some(now);
        self.revision += 1;
        true
    }
}

fn inventory(
    config: &Config,
    candidates: Vec<Candidate>,
) -> Result<BTreeMap<String, Candidate>, Error> {
    if candidates.len() > config.max_candidates {
        return Err(Error::InvalidCandidates);
    }
    let mut result = BTreeMap::new();
    for candidate in candidates {
        if [
            &candidate.id,
            &candidate.group.instance,
            &candidate.group.egress,
        ]
        .iter()
        .any(|id| id.is_empty() || id.len() > 128)
            || candidate.quota_multiplier_milli == 0
            || candidate.quota_multiplier_milli > 999_999
            || candidate.latency_threshold_ms == 0
            || candidate.latency_threshold_ms > i32::MAX as u32
            || candidate.tier_rank <= 0
            || result.contains_key(&candidate.id)
        {
            return Err(Error::InvalidCandidates);
        }
        result.insert(candidate.id.clone(), candidate);
    }
    Ok(result)
}

fn valid_group(group: &EgressGroup) -> bool {
    [&group.instance, &group.egress]
        .iter()
        .all(|id| !id.is_empty() && id.len() <= 128)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(instance: &str, egress: &str) -> EgressGroup {
        EgressGroup {
            instance: instance.into(),
            egress: egress.into(),
        }
    }
    fn epoch() -> Epoch {
        Epoch {
            session: 1,
            network: 1,
            profile: 1,
        }
    }
    fn config() -> Config {
        Config {
            max_quota_multiplier_milli: 2_000,
            fresh_for_ms: 100_000,
            fail_after: 2,
            recover_after: 3,
            min_dwell_ms: 5_000,
            min_latency_improvement_ms: 20,
            max_candidates: 32,
            max_in_flight: 2,
            max_per_round: 4,
            round_timeout_ms: 500,
            min_round_interval_ms: 1_000,
        }
    }
    fn candidate(id: &str, multiplier: u32) -> Candidate {
        Candidate {
            id: id.into(),
            group: group("instance", "exit-set"),
            quota_multiplier_milli: multiplier,
            latency_threshold_ms: 200,
            tier_rank: 1,
            eligible: true,
        }
    }
    fn policy(candidates: Vec<Candidate>) -> Policy {
        Policy::new(config(), epoch(), group("instance", "exit-set"), candidates).unwrap()
    }
    fn available(latency_ms: u32) -> Health {
        Health::Available { latency_ms }
    }
    fn round(policy: &mut Policy, now: u64, outcomes: &[(&str, Health)]) -> Vec<String> {
        assert!(policy.begin_round(now).unwrap().is_empty());
        let mut probed = Vec::new();
        loop {
            let plan = policy.plan_probes(now);
            assert!(plan.cancel.is_empty());
            if plan.start.is_empty() {
                break;
            }
            for probe in plan.start {
                let health = outcomes
                    .iter()
                    .find(|(id, _)| *id == probe.candidate.id)
                    .map(|(_, health)| *health)
                    .unwrap_or(Health::Unavailable);
                assert_eq!(
                    policy.finish_probe(&probe.token, health, now),
                    SampleResult::Accepted
                );
                probed.push(probe.candidate.id);
            }
        }
        probed
    }
    fn select(policy: &mut Policy, now: u64) -> String {
        let proposal = policy
            .recommend(now)
            .change
            .expect("a real selection proposal");
        let id = proposal.candidate.clone();
        assert!(policy.acknowledge_selection(&proposal, now));
        id
    }

    #[test]
    fn qualified_rank_precedes_faster_and_lower_multiplier() {
        let mut earlier = candidate("earlier", 2_000);
        earlier.tier_rank = 1;
        let mut later = candidate("later", 500);
        later.tier_rank = 2;
        let mut p = policy(vec![earlier, later]);
        round(
            &mut p,
            0,
            &[("earlier", available(180)), ("later", available(10))],
        );
        assert_eq!(select(&mut p, 0), "earlier");
        round(
            &mut p,
            6_000,
            &[("earlier", available(190)), ("later", available(1))],
        );
        assert_eq!(p.recommend(6_000).reason, Reason::CurrentHealthy);
    }

    #[test]
    fn same_rank_quality_is_not_reordered_by_multiplier() {
        let mut p = policy(vec![candidate("fast", 2_000), candidate("slow", 500)]);
        round(
            &mut p,
            0,
            &[("fast", available(20)), ("slow", available(150))],
        );
        assert_eq!(select(&mut p, 0), "fast");
    }

    #[test]
    fn filters_eligibility_user_multiplier_constraint_and_each_candidates_own_threshold() {
        let mut cheap = candidate("cheap", 1_000);
        cheap.latency_threshold_ms = 100;
        let mut slow_allowed = candidate("allowed", 1_100);
        slow_allowed.latency_threshold_ms = 300;
        let mut denied = candidate("denied", 1);
        denied.eligible = false;
        let mut p = policy(vec![
            cheap,
            slow_allowed,
            denied,
            candidate("over-cost", 2_001),
        ]);
        let probed = round(
            &mut p,
            0,
            &[("cheap", available(150)), ("allowed", available(250))],
        );
        assert_eq!(probed.len(), 2);
        assert_eq!(select(&mut p, 0), "allowed");
    }

    #[test]
    fn exact_multiplier_only_filters_an_explicit_user_constraint() {
        let mut c = config();
        c.max_quota_multiplier_milli = 1_099;
        let mut earlier = candidate("earlier", 1_100);
        earlier.tier_rank = 1;
        let mut later = candidate("later", 1_099);
        later.tier_rank = 2;
        let mut p = Policy::new(
            c,
            epoch(),
            group("instance", "exit-set"),
            vec![earlier, later],
        )
        .unwrap();
        let probed = round(
            &mut p,
            0,
            &[("earlier", available(1)), ("later", available(199))],
        );
        assert_eq!(probed, ["later"]);
        assert_eq!(select(&mut p, 0), "later");
    }

    #[test]
    fn same_instance_cross_logical_entries_work_but_other_exits_and_instances_never_do() {
        let mut foreign = candidate("foreign-instance", 1);
        foreign.group.instance = "other".into();
        let mut other_exit = candidate("different-local-final", 1);
        other_exit.group.egress = "other-IP-set".into();
        let mut p = policy(vec![
            Candidate {
                tier_rank: 2,
                ..candidate("node-A/endpoint-1", 1_100)
            },
            candidate("node-B/endpoint-2", 1_000),
            foreign,
            other_exit,
        ]);
        let probed = round(
            &mut p,
            0,
            &[
                ("node-A/endpoint-1", available(10)),
                ("node-B/endpoint-2", available(20)),
            ],
        );
        assert_eq!(probed.len(), 2);
        assert_eq!(select(&mut p, 0), "node-B/endpoint-2");
        p.set_manual(Some("foreign-instance".into()));
        assert_eq!(p.recommend(0).reason, Reason::EgressGroupLocked);
        assert!(p.recommend(0).change.is_none());
        assert_eq!(
            p.reset_scope(epoch(), group("other", "exit-set")),
            Err(Error::ScopeChangeRequiresNewSession)
        );
    }

    #[test]
    fn earlier_rank_recovery_waits_for_three_successes_and_dwell() {
        let mut p = policy(vec![
            candidate("primary", 1_000),
            Candidate {
                tier_rank: 2,
                ..candidate("backup", 1_100)
            },
        ]);
        round(&mut p, 0, &[("backup", available(20))]);
        assert_eq!(select(&mut p, 0), "backup");
        round(&mut p, 1_000, &[("backup", available(20))]); // primary now failed twice
        for at in [2_000, 3_000] {
            round(
                &mut p,
                at,
                &[("primary", available(20)), ("backup", available(20))],
            );
            assert!(p.recommend(at).change.is_none());
        }
        round(
            &mut p,
            4_000,
            &[("primary", available(20)), ("backup", available(20))],
        );
        assert_eq!(p.recommend(4_000).reason, Reason::CoolingDown);
        assert_eq!(select(&mut p, 5_000), "primary");
    }

    #[test]
    fn working_but_slow_earlier_tier_escalates_and_recovers_with_hysteresis() {
        let mut earlier = candidate("earlier", 2_000);
        earlier.latency_threshold_ms = 100;
        let mut later = candidate("later", 500);
        later.tier_rank = 2;
        let mut p = policy(vec![earlier, later]);
        round(
            &mut p,
            0,
            &[("earlier", available(90)), ("later", available(10))],
        );
        assert_eq!(select(&mut p, 0), "earlier");
        round(
            &mut p,
            1_000,
            &[("earlier", available(101)), ("later", available(10))],
        );
        assert_eq!(
            p.recommend(1_000).reason,
            Reason::CurrentHealthy,
            "one spike is not a downgrade"
        );
        round(
            &mut p,
            2_000,
            &[("earlier", available(110)), ("later", available(10))],
        );
        assert_eq!(p.recommend(2_000).reason, Reason::CurrentUnhealthy);
        assert_eq!(
            select(&mut p, 2_000),
            "later",
            "quality breach, without a failed connection"
        );
        for at in [3_000, 4_000] {
            round(
                &mut p,
                at,
                &[("earlier", available(95)), ("later", available(10))],
            );
            assert!(p.recommend(at).change.is_none());
        }
        round(
            &mut p,
            5_000,
            &[("earlier", available(95)), ("later", available(10))],
        );
        assert_eq!(p.recommend(5_000).reason, Reason::CoolingDown);
        assert_eq!(p.recommend(7_000).reason, Reason::RankImprovement);
        assert_eq!(select(&mut p, 7_000), "earlier");
    }

    #[test]
    fn unhealthy_current_fails_over_inside_group_without_waiting_for_dwell() {
        let mut p = policy(vec![candidate("a", 1_000), candidate("b", 1_100)]);
        round(&mut p, 0, &[("a", available(10)), ("b", available(20))]);
        select(&mut p, 0);
        round(&mut p, 1_000, &[("b", available(20))]);
        assert_eq!(p.recommend(1_000).reason, Reason::CurrentHealthy);
        round(&mut p, 2_000, &[("b", available(20))]);
        assert_eq!(p.recommend(2_000).reason, Reason::CurrentUnhealthy);
        assert_eq!(select(&mut p, 2_000), "b");
    }

    #[test]
    fn within_tier_small_latency_improvements_do_not_flap_regardless_of_multiplier() {
        let mut p = policy(vec![candidate("a", 1_000), candidate("b", 1_000)]);
        round(&mut p, 0, &[("a", available(100))]);
        select(&mut p, 0);
        round(
            &mut p,
            1_000,
            &[("a", available(100)), ("b", available(90))],
        );
        assert_eq!(p.recommend(6_000).reason, Reason::BelowImprovement);
        round(
            &mut p,
            6_000,
            &[("a", available(100)), ("b", available(80))],
        );
        assert_eq!(p.recommend(6_000).reason, Reason::LatencyImprovement);
        assert_eq!(select(&mut p, 6_000), "b");
        // Changing a multiplier does not introduce price preference; inventory
        // changes still require fresh health, and within-tier RTT hysteresis holds.
        p.replace_candidates(vec![candidate("a", 995), candidate("b", 1_000)])
            .unwrap();
        round(
            &mut p,
            7_000,
            &[("a", available(95)), ("b", available(100))],
        );
        assert_eq!(p.recommend(12_000).reason, Reason::BelowImprovement);
    }

    #[test]
    fn manual_override_stays_selected_even_when_unhealthy_and_unpin_returns_to_auto() {
        let mut p = policy(vec![candidate("a", 1_000), candidate("b", 2_000)]);
        round(&mut p, 0, &[("a", available(10))]);
        round(&mut p, 1_000, &[("a", available(10))]);
        p.set_manual(Some("b".into()));
        assert_eq!(p.recommend(1_000).reason, Reason::ManualUnhealthy);
        assert_eq!(select(&mut p, 1_000), "b");
        assert_eq!(p.begin_round(2_000), Err(Error::ManualMode));
        assert!(p.recommend(2_000).change.is_none());
        p.set_manual(None);
        assert_eq!(p.recommend(2_000).reason, Reason::CurrentUnhealthy);
        assert_eq!(select(&mut p, 2_000), "a");
    }

    #[test]
    fn unavailable_or_ineligible_manual_target_does_not_fall_back_automatically() {
        let mut p = policy(vec![candidate("a", 1_000)]);
        round(&mut p, 0, &[("a", available(10))]);
        select(&mut p, 0);
        p.set_manual(Some("removed".into()));
        let decision = p.recommend(1);
        assert_eq!(decision.reason, Reason::ManualIneligible);
        assert_eq!(decision.current.as_deref(), Some("a"));
        assert!(decision.change.is_none());
    }

    #[test]
    fn recommendations_are_not_acknowledged_selections_and_stale_acks_cannot_win() {
        let mut p = policy(vec![candidate("a", 1_000), candidate("b", 1_100)]);
        round(&mut p, 0, &[("a", available(10)), ("b", available(20))]);
        let old = p.recommend(0).change.unwrap();
        assert_eq!(p.recommend(0).current, None);
        p.set_manual(Some("b".into()));
        assert!(!p.acknowledge_selection(&old, 0));
        assert_eq!(select(&mut p, 0), "b");
        assert!(!p.acknowledge_selection(&old, 0));
    }

    #[test]
    fn stale_health_or_backwards_time_never_qualifies_and_expired_ack_is_rejected() {
        let mut p = policy(vec![candidate("a", 1_000)]);
        round(&mut p, 1_000, &[("a", available(10))]);
        let proposal = p.recommend(1_000).change.unwrap();
        assert_eq!(p.recommend(999).reason, Reason::NoQualifiedCandidate);
        assert_eq!(p.recommend(101_001).reason, Reason::NoQualifiedCandidate);
        assert!(!p.acknowledge_selection(&proposal, 101_001));
    }

    #[test]
    fn network_epoch_cancels_old_work_without_freeing_live_resource_slots_early() {
        let mut p = policy(vec![candidate("a", 1_000), candidate("b", 1_100)]);
        p.begin_round(0).unwrap();
        let old = p.plan_probes(0).start;
        assert_eq!(old.len(), 2);
        let mut next_epoch = epoch();
        next_epoch.network += 1;
        let canceled = p.set_epoch(next_epoch);
        assert_eq!(canceled.len(), 2);
        p.begin_round(1_000).unwrap();
        assert!(
            p.plan_probes(1_000).start.is_empty(),
            "old work still uses both slots"
        );
        assert!(p.acknowledge_cancel(&canceled[0]));
        assert!(
            !p.acknowledge_cancel(&canceled[0]),
            "duplicate cancel cannot free another slot"
        );
        let new = p.plan_probes(1_000).start;
        assert_eq!(new.len(), 1);
        assert_eq!(
            p.finish_probe(&old[1].token, available(1), 1_000),
            SampleResult::Stale
        );
        assert_eq!(p.recommend(1_000).reason, Reason::NoQualifiedCandidate);
        assert_eq!(p.plan_probes(1_000).start.len(), 1);
    }

    #[test]
    fn wrong_and_duplicate_tokens_cannot_release_someone_elses_probe_slot() {
        let mut p = policy(vec![
            candidate("a", 1_000),
            candidate("b", 1_100),
            candidate("c", 1_200),
        ]);
        p.begin_round(0).unwrap();
        let probes = p.plan_probes(0).start;
        let mut forged = probes[0].token.clone();
        forged.epoch.network += 1;
        assert_eq!(
            p.finish_probe(&forged, available(1), 0),
            SampleResult::Unknown
        );
        assert!(
            !p.acknowledge_cancel(&probes[0].token),
            "active token is not canceled"
        );
        assert!(p.plan_probes(0).start.is_empty());
        assert_eq!(
            p.finish_probe(&probes[0].token, available(10), 0),
            SampleResult::Accepted
        );
        assert_eq!(
            p.finish_probe(&probes[0].token, available(1), 0),
            SampleResult::Unknown
        );
        assert_eq!(p.plan_probes(0).start.len(), 1);
        assert!(p.plan_probes(0).start.is_empty());
    }

    #[test]
    fn round_caps_rate_limit_and_rotation_prevent_probe_bursts_and_starvation() {
        let mut p = policy((0..9).map(|i| candidate(&format!("c{i}"), 1_000)).collect());
        let mut all = BTreeSet::new();
        for at in [0, 1_000, 2_000] {
            let ids = round(&mut p, at, &[]);
            assert_eq!(ids.len(), 4);
            all.extend(ids);
            assert!(p.plan_probes(at).start.is_empty());
            assert_eq!(p.begin_round(at + 999), Err(Error::TooSoon));
        }
        assert_eq!(all.len(), 9);
    }

    #[test]
    fn active_path_is_refreshed_while_alternative_probe_slots_rotate() {
        let mut p = policy((0..9).map(|i| candidate(&format!("c{i}"), 1_000)).collect());
        round(&mut p, 0, &[("c0", available(10))]);
        select(&mut p, 0);
        let mut others = BTreeSet::new();
        for at in [1_000, 2_000, 3_000] {
            let ids = round(&mut p, at, &[("c0", available(10))]);
            assert!(ids.contains(&"c0".into()));
            others.extend(ids.into_iter().filter(|id| id != "c0"));
        }
        assert_eq!(others.len(), 8);
    }

    #[test]
    fn deadline_drops_late_samples_and_requires_actual_cancellation_ack() {
        let mut p = policy(vec![candidate("a", 1_000)]);
        p.begin_round(1_000).unwrap();
        assert!(p.plan_probes(999).start.is_empty());
        let probe = p.plan_probes(1_000).start.pop().unwrap();
        let expired = p.plan_probes(1_501);
        assert_eq!(expired.cancel.len(), 1);
        assert_eq!(
            p.finish_probe(&probe.token, available(10), 1_501),
            SampleResult::Stale
        );
        assert_eq!(p.recommend(1_501).reason, Reason::NoQualifiedCandidate);
        assert!(!p.acknowledge_cancel(&expired.cancel[0]));
    }

    #[test]
    fn profile_change_discards_inflight_old_endpoint_health() {
        let mut p = policy(vec![candidate("a", 1_000)]);
        p.begin_round(0).unwrap();
        let old = p.plan_probes(0).start.pop().unwrap();
        let mut replacement = candidate("a", 500);
        replacement.latency_threshold_ms = 20;
        let canceled = p.replace_candidates(vec![replacement]).unwrap();
        assert_eq!(canceled.len(), 1);
        assert_eq!(
            p.finish_probe(&old.token, available(10), 0),
            SampleResult::Stale
        );
        assert!(p.recommend(0).change.is_none());
    }

    #[test]
    fn background_pause_cancels_probes_and_resume_requires_fresh_observations() {
        let mut p = policy(vec![candidate("a", 1_000)]);
        round(&mut p, 0, &[("a", available(10))]);
        select(&mut p, 0);
        p.begin_round(1_000).unwrap();
        let pending = p.plan_probes(1_000).start;
        assert_eq!(p.set_paused(true).len(), 1);
        assert_eq!(p.begin_round(2_000), Err(Error::Paused));
        assert_eq!(p.recommend(2_000).reason, Reason::Paused);
        p.set_paused(false);
        assert_eq!(p.recommend(2_000).reason, Reason::NoQualifiedCandidate);
        assert_eq!(
            p.finish_probe(&pending[0].token, available(1), 2_000),
            SampleResult::Stale
        );
    }

    #[test]
    fn new_session_may_change_scope_but_cannot_lose_old_probe_budget() {
        let mut p = policy(vec![candidate("a", 1_000), candidate("b", 1_000)]);
        p.set_manual(Some("a".into()));
        select(&mut p, 0);
        p.set_manual(None);
        p.begin_round(0).unwrap();
        let old = p.plan_probes(0).start;
        let mut next = epoch();
        next.session += 1;
        let cancel = p
            .reset_scope(next, group("new-instance", "exit-set"))
            .unwrap();
        assert_eq!(cancel.len(), old.len());
        assert_eq!(p.recommend(1).current, None);
        let mut candidate = candidate("new", 1_000);
        candidate.group.instance = "new-instance".into();
        p.replace_candidates(vec![candidate]).unwrap();
        p.begin_round(1_000).unwrap();
        assert!(p.plan_probes(1_000).start.is_empty());
        for token in cancel {
            assert!(p.acknowledge_cancel(&token));
        }
        let new = p.plan_probes(1_000).start;
        assert_eq!(new.len(), 1);
    }

    #[test]
    fn invalid_metadata_or_inventory_is_rejected_without_mutating_current_policy() {
        let mut p = policy(vec![candidate("a", 1_000)]);
        p.set_manual(Some("a".into()));
        select(&mut p, 0);
        for mut invalid in [
            candidate("zero-cost", 0),
            candidate("too-expensive-format", 1_000_000),
            candidate("no-threshold", 1_000),
            Candidate {
                tier_rank: 0,
                ..candidate("no-rank", 1_000)
            },
            Candidate {
                latency_threshold_ms: i32::MAX as u32 + 1,
                ..candidate("threshold-overflow", 1_000)
            },
        ] {
            if invalid.id == "no-threshold" {
                invalid.latency_threshold_ms = 0;
            }
            assert_eq!(
                p.replace_candidates(vec![invalid]),
                Err(Error::InvalidCandidates)
            );
            assert_eq!(p.recommend(0).current.as_deref(), Some("a"));
        }
        assert_eq!(
            p.replace_candidates(vec![candidate("a", 1_000), candidate("a", 1_100)]),
            Err(Error::InvalidCandidates)
        );
        assert_eq!(
            p.replace_candidates((0..33).map(|i| candidate(&i.to_string(), 1_000)).collect()),
            Err(Error::InvalidCandidates)
        );
    }

    #[test]
    fn context_churn_cannot_reset_the_global_probe_rate_limit() {
        let mut p = policy(vec![candidate("a", 1_000)]);
        round(&mut p, 0, &[("a", available(10))]);
        let mut next = epoch();
        next.network += 1;
        p.set_epoch(next);
        p.set_manual(Some("a".into()));
        p.set_manual(None);
        p.set_paused(true);
        p.set_paused(false);
        p.replace_candidates(vec![candidate("a", 900)]).unwrap();
        next.session += 1;
        p.reset_scope(next, group("instance", "exit-set")).unwrap();
        assert_eq!(p.begin_round(999), Err(Error::TooSoon));
        assert!(p.begin_round(1_000).is_ok());
    }

    fn single_probe_policy(candidates: Vec<Candidate>) -> Policy {
        let mut config = config();
        config.max_in_flight = 1;
        config.max_per_round = 1;
        Policy::new(config, epoch(), group("instance", "exit-set"), candidates).unwrap()
    }

    #[test]
    fn single_probe_rounds_cover_alternatives_with_an_existing_current_selection() {
        let mut p =
            single_probe_policy((0..5).map(|i| candidate(&format!("c{i}"), 1_000)).collect());
        p.set_manual(Some("c0".into()));
        select(&mut p, 0);
        p.set_manual(None);
        let mut seen = BTreeSet::new();
        for at in [0, 1_000, 2_000, 3_000, 4_000] {
            let ids = round(&mut p, at, &[]);
            assert_eq!(ids.len(), 1, "never increase the caller's one-probe budget");
            seen.extend(ids);
        }
        assert_eq!(
            seen.len(),
            5,
            "an existing current must not starve alternatives"
        );
    }

    #[test]
    fn single_probe_budget_still_discovers_recovery_to_an_earlier_rank() {
        let mut backup = candidate("b-backup", 500);
        backup.tier_rank = 2;
        let mut p = single_probe_policy(vec![candidate("a-earlier", 2_000), backup]);
        p.set_manual(Some("b-backup".into()));
        select(&mut p, 0);
        p.set_manual(None);
        for at in [0, 1_000, 2_000, 3_000] {
            round(&mut p, at, &[("b-backup", available(10))]);
        }
        for at in [4_000, 5_000, 6_000, 7_000] {
            round(
                &mut p,
                at,
                &[("a-earlier", available(180)), ("b-backup", available(10))],
            );
            assert!(
                p.recommend(at).change.is_none(),
                "recovery is still debounced"
            );
        }
        round(
            &mut p,
            8_000,
            &[("a-earlier", available(180)), ("b-backup", available(10))],
        );
        assert_eq!(p.recommend(8_000).reason, Reason::RankImprovement);
        assert_eq!(select(&mut p, 8_000), "a-earlier");
    }

    #[test]
    fn single_probe_rotation_does_not_treat_expired_current_health_as_fresh() {
        let mut p = single_probe_policy(vec![candidate("a-current", 1_000), candidate("b", 1_000)]);
        p.config.fresh_for_ms = 1_500;
        p.set_manual(Some("a-current".into()));
        select(&mut p, 0);
        p.set_manual(None);
        round(&mut p, 0, &[("a-current", available(10))]);
        round(&mut p, 1_000, &[("b", available(20))]);
        let decision = p.recommend(1_501);
        assert_eq!(
            decision.current.as_deref(),
            Some("a-current"),
            "recommendation is not a confirmed switch"
        );
        assert_eq!(decision.reason, Reason::CurrentUnhealthy);
        assert!(
            decision.change.is_some(),
            "only the fresh qualified alternative may be proposed"
        );
    }

    #[test]
    fn engine_seed_records_identity_without_claiming_health_or_overwriting_newer_current() {
        let mut p = policy(vec![candidate("a", 1_000), candidate("b", 1_000)]);
        assert!(p.seed_confirmed("a", epoch(), 0));
        let decision = p.recommend(0);
        assert_eq!(decision.current.as_deref(), Some("a"));
        assert_eq!(decision.reason, Reason::NoQualifiedCandidate);
        assert!(!p.seed_confirmed("b", epoch(), 1));
        let mut wrong = epoch();
        wrong.network += 1;
        assert!(!p.seed_confirmed("a", wrong, 1));
        assert!(!p.seed_confirmed("unknown", epoch(), 1));
    }

    #[test]
    fn bad_resource_configs_are_rejected_not_silently_expanded() {
        for n in [0, 5, usize::MAX] {
            let mut config = config();
            config.max_in_flight = n;
            assert!(matches!(
                Policy::new(config, epoch(), group("instance", "exit-set"), vec![]),
                Err(Error::InvalidConfig)
            ));
        }
        let mut config = config();
        config.min_round_interval_ms = 0;
        assert!(matches!(
            Policy::new(config, epoch(), group("instance", "exit-set"), vec![]),
            Err(Error::InvalidConfig)
        ));
    }
}
