//! Staged execution journal, not a live Client/transport controller yet.
//!
//! An owner adapter must hold one mutation lease shared with Manual choices,
//! profile apply, stop/restart and persistence while driving these effects.
//! Each view must be read from the real engine, not synthesized from RPC OK.
//! Existing separate RPCs are not server-side atomic; this journal does not
//! claim otherwise. Context must be checked again at actual effect dispatch.
//! No unpin effect exists: incomplete restoration is explicitly reconciliation.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::auto_route::{Epoch, Policy, Proposal};
use crate::auto_route_profile::{CandidateSet, EntranceKey};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    pub epoch: Epoch,
    /// Local intent generation for Manual/Auto, stop and owner requests.
    pub request_epoch: u64,
    pub revision: String,
    pub owner: String,
    pub owner_generation: u64,
    pub runtime_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EngineView {
    pub context: Context,
    pub running: bool,
    pub selected: EntranceKey,
    pub pins: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Manual,
    Automatic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Preferences {
    pub mode: Mode,
    pub selected_node: String,
    pub pins: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    PinExact(EntranceKey),
    SelectNode(String),
    ReadEngine,
    /// Adapter must atomically save the whole choice under the same context
    /// and merge unrelated settings, then read it back for comparison.
    Save(Preferences),
    ReadPreferences,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Effect {
    pub id: u32,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    Applied,
    Engine(EngineView),
    Preferences(Preferences),
    /// Authoritative completion with failure, not a transport timeout.
    Failed,
    /// Timeout/drop: the remote mutation or file write may still complete.
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rejected {
    ContextMismatch,
    EngineNotRunning,
    SelectionMismatch,
    TargetUnknown,
    SavedAutomaticChoiceMismatch,
    UnexpectedReply,
    NoEligibleSeed,
    Busy,
    ReconciliationRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    EngineAndStorageConfirmed {
        selected: EntranceKey,
        context: Context,
    },
    RolledBack {
        observed: EngineView,
    },
    /// Last view is observation only, never a fabricated final state.
    NeedsReconcile {
        observed: Option<EngineView>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Fence,
    VerifyFence,
    PinTarget,
    VerifyPin,
    Select,
    VerifyTarget,
    Save,
    VerifySaved,
    VerifyFinal,
    RollbackRead,
    RollbackPin,
    RollbackVerifyPin,
    RollbackSelect,
    RollbackVerify,
    RestoreSaved,
    VerifyRestored,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Recovery {
    pub context: Context,
    pub observed: Option<EngineView>,
}

#[derive(Default)]
struct JournalState {
    active: bool,
    recovery: Option<Recovery>,
}

/// Retained by the owner outside the cancellable transaction future. In-memory
/// only: crash/restart recovery still requires real-engine/settings inspection.
#[derive(Clone, Default)]
pub(crate) struct Journal(Arc<Mutex<JournalState>>);

impl Journal {
    pub fn recovery(&self) -> Option<Recovery> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recovery
            .clone()
    }
    fn reserve(&self) -> Result<(), Rejected> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.active {
            return Err(Rejected::Busy);
        }
        if state.recovery.is_some() {
            return Err(Rejected::ReconciliationRequired);
        }
        state.active = true;
        Ok(())
    }
}

pub(crate) struct Transaction {
    context: Context,
    journal: Journal,
    mutated: bool,
    finalized: bool,
    target_id: String,
    before: EngineView,
    previous_preferences: Preferences,
    target: EntranceKey,
    desired: Option<Preferences>,
    observed: Option<EngineView>,
    phase: Phase,
    pending: Option<Effect>,
    serial: u32,
    save_attempted: bool,
    outcome: Option<Outcome>,
}

impl Transaction {
    /// CandidateSet must itself have been built from this actual confirmed
    /// selection. Only a known eligible target is accepted. Capability/probe
    /// gates and current policy-proposal validation belong to the outer driver.
    pub fn begin(
        context: Context,
        set: &CandidateSet,
        actual: EngineView,
        previous_preferences: Preferences,
        target_id: &str,
        journal: Journal,
    ) -> Result<Self, Rejected> {
        validate_context(&context, &actual)?;
        if set.revision != context.revision {
            return Err(Rejected::ContextMismatch);
        }
        if set.confirmed != actual.selected {
            return Err(Rejected::SelectionMismatch);
        }
        let candidate = set
            .candidates
            .iter()
            .find(|c| c.id == target_id && c.eligible && c.group == set.scope)
            .ok_or(Rejected::TargetUnknown)?;
        let target = set
            .targets
            .get(&candidate.id)
            .cloned()
            .ok_or(Rejected::TargetUnknown)?;
        if previous_preferences.mode == Mode::Automatic
            && (previous_preferences.selected_node != actual.selected.node_id
                || previous_preferences.pins.get(&actual.selected.node_id)
                    != Some(&actual.selected.endpoint_key))
        {
            return Err(Rejected::SavedAutomaticChoiceMismatch);
        }
        journal.reserve()?;
        Ok(Self {
            context,
            journal,
            mutated: false,
            finalized: false,
            target_id: target_id.into(),
            before: actual.clone(),
            previous_preferences,
            target,
            desired: None,
            observed: Some(actual),
            phase: Phase::Fence,
            pending: None,
            serial: 0,
            save_attempted: false,
            outcome: None,
        })
    }

    /// Returns an effect exactly once. While it awaits completion, another
    /// call never redispatches it; transport cancellation is not completion.
    pub fn next(&mut self, current: &Context) -> Option<Effect> {
        if self.finalized {
            return None;
        }
        if *current != self.context {
            self.reconcile();
            return None;
        }
        if self.pending.is_some() {
            return None;
        }
        let action = match self.phase {
            Phase::Fence | Phase::RollbackPin => Action::PinExact(self.before.selected.clone()),
            Phase::PinTarget => Action::PinExact(self.target.clone()),
            Phase::Select => Action::SelectNode(self.target.node_id.clone()),
            Phase::RollbackSelect => Action::SelectNode(self.before.selected.node_id.clone()),
            Phase::VerifyFence
            | Phase::VerifyPin
            | Phase::VerifyTarget
            | Phase::VerifyFinal
            | Phase::RollbackRead
            | Phase::RollbackVerifyPin
            | Phase::RollbackVerify => Action::ReadEngine,
            Phase::Save => {
                self.save_attempted = true;
                Action::Save(self.desired.clone().expect("desired prepared"))
            }
            Phase::RestoreSaved => Action::Save(self.previous_preferences.clone()),
            Phase::VerifySaved | Phase::VerifyRestored => Action::ReadPreferences,
            Phase::Done => return None,
        };
        if matches!(
            action,
            Action::PinExact(_) | Action::SelectNode(_) | Action::Save(_)
        ) {
            self.mutated = true;
            self.mark_recovery();
        }
        self.serial += 1;
        let effect = Effect {
            id: self.serial,
            action,
        };
        self.pending = Some(effect.clone());
        Some(effect)
    }

    pub fn accept(&mut self, id: u32, reply: Reply, current: &Context) -> Result<(), Rejected> {
        if self.finalized {
            return Err(Rejected::UnexpectedReply);
        }
        if *current != self.context {
            self.reconcile();
            return Ok(());
        }
        let Some(effect) = self.pending.as_ref().filter(|effect| effect.id == id) else {
            return Err(Rejected::UnexpectedReply);
        };
        let correct_kind = matches!(
            (&effect.action, &reply),
            (_, Reply::Failed | Reply::Uncertain)
                | (
                    Action::PinExact(_) | Action::SelectNode(_) | Action::Save(_),
                    Reply::Applied
                )
                | (Action::ReadEngine, Reply::Engine(_))
                | (Action::ReadPreferences, Reply::Preferences(_))
        );
        if !correct_kind {
            return Err(Rejected::UnexpectedReply);
        }
        self.pending = None;
        if reply == Reply::Uncertain {
            self.reconcile();
            return Ok(());
        }
        if let Reply::Engine(view) = &reply {
            if validate_context(&self.context, view).is_err() {
                self.observed = Some(view.clone());
                self.reconcile();
                return Ok(());
            }
            self.observed = Some(view.clone());
            if self.mutated {
                self.mark_recovery();
            }
        }
        if reply == Reply::Failed {
            self.fail();
            return Ok(());
        }
        self.phase = match (self.phase, reply) {
            (Phase::Fence, Reply::Applied) => Phase::VerifyFence,
            (Phase::VerifyFence, Reply::Engine(view)) if pinned(&view, &self.before.selected) => {
                if self.target == self.before.selected {
                    self.prepare_save(&view);
                    Phase::Save
                } else {
                    Phase::PinTarget
                }
            }
            (Phase::PinTarget, Reply::Applied) => Phase::VerifyPin,
            (Phase::VerifyPin, Reply::Engine(view))
                if view.pins.get(&self.target.node_id) == Some(&self.target.endpoint_key)
                    && view.selected
                        == if self.target.node_id == self.before.selected.node_id {
                            self.target.clone()
                        } else {
                            self.before.selected.clone()
                        } =>
            {
                if self.target.node_id == self.before.selected.node_id {
                    self.prepare_save(&view);
                    Phase::Save
                } else {
                    Phase::Select
                }
            }
            (Phase::Select, Reply::Applied) => Phase::VerifyTarget,
            (Phase::VerifyTarget, Reply::Engine(view)) if pinned(&view, &self.target) => {
                self.prepare_save(&view);
                Phase::Save
            }
            (Phase::Save, Reply::Applied) => Phase::VerifySaved,
            (Phase::VerifySaved, Reply::Preferences(saved))
                if Some(&saved) == self.desired.as_ref() =>
            {
                Phase::VerifyFinal
            }
            (Phase::VerifyFinal, Reply::Engine(view)) if pinned(&view, &self.target) => {
                self.outcome = Some(Outcome::EngineAndStorageConfirmed {
                    selected: self.target.clone(),
                    context: self.context.clone(),
                });
                Phase::Done
            }
            // On failure, inspect before rollback: never overwrite a different
            // owner's or a third party's currently selected route.
            (Phase::RollbackRead, Reply::Engine(view))
                if view.selected == self.before.selected || view.selected == self.target =>
            {
                Phase::RollbackPin
            }
            (Phase::RollbackPin, Reply::Applied) => Phase::RollbackVerifyPin,
            (Phase::RollbackVerifyPin, Reply::Engine(view))
                if view.pins.get(&self.before.selected.node_id)
                    == Some(&self.before.selected.endpoint_key)
                    && (view.selected == self.before.selected || view.selected == self.target) =>
            {
                Phase::RollbackSelect
            }
            (Phase::RollbackSelect, Reply::Applied) => Phase::RollbackVerify,
            (Phase::RollbackVerify, Reply::Engine(view))
                if pinned(&view, &self.before.selected) =>
            {
                if self.save_attempted {
                    Phase::RestoreSaved
                } else {
                    self.finish_rollback();
                    Phase::Done
                }
            }
            (Phase::RestoreSaved, Reply::Applied) => Phase::VerifyRestored,
            (Phase::VerifyRestored, Reply::Preferences(saved))
                if saved == self.previous_preferences =>
            {
                self.finish_rollback();
                Phase::Done
            }
            _ => {
                self.fail();
                self.phase
            }
        };
        Ok(())
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    /// Called only after the journal verified actual engine and storage state.
    /// Policy expiry/staleness still prevents ACK; do not forge health to force it.
    pub fn acknowledge_policy(
        &mut self,
        policy: &mut Policy,
        proposal: &Proposal,
        current: &Context,
        now: u64,
    ) -> bool {
        if self.finalized
            || *current != self.context
            || proposal.candidate != self.target_id
            || !matches!(
                self.outcome,
                Some(Outcome::EngineAndStorageConfirmed { .. })
            )
            || !policy.acknowledge_selection(proposal, now)
        {
            if !self.finalized {
                self.reconcile();
            }
            return false;
        }
        self.finish_clean();
        true
    }

    fn mark_recovery(&self) {
        let mut state = self
            .journal
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.recovery = Some(Recovery {
            context: self.context.clone(),
            observed: self.observed.clone(),
        });
    }

    fn finish_clean(&mut self) {
        let mut state = self
            .journal
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active = false;
        state.recovery = None;
        self.finalized = true;
    }

    fn prepare_save(&mut self, view: &EngineView) {
        let mut pins = self.previous_preferences.pins.clone();
        pins.extend(view.pins.clone());
        self.desired = Some(Preferences {
            mode: Mode::Automatic,
            selected_node: self.target.node_id.clone(),
            pins,
        });
    }

    fn fail(&mut self) {
        if matches!(
            self.phase,
            Phase::RollbackRead
                | Phase::RollbackPin
                | Phase::RollbackVerifyPin
                | Phase::RollbackSelect
                | Phase::RollbackVerify
                | Phase::RestoreSaved
                | Phase::VerifyRestored
        ) {
            self.reconcile();
        } else {
            self.phase = Phase::RollbackRead;
        }
    }

    fn finish_rollback(&mut self) {
        let Some(view) = &self.observed else {
            self.reconcile();
            return;
        };
        if view.pins == self.before.pins {
            self.outcome = Some(Outcome::RolledBack {
                observed: view.clone(),
            });
            self.finish_clean();
        } else {
            self.reconcile();
        }
    }

    fn reconcile(&mut self) {
        self.outcome = Some(Outcome::NeedsReconcile {
            observed: self.observed.clone(),
        });
        self.phase = Phase::Done;
        self.pending = None;
        if self.mutated {
            self.mark_recovery();
        }
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        if self.mutated {
            self.mark_recovery();
        }
        self.journal
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active = false;
    }
}

/// Only mirrors a current real-engine identity. It does not make the candidate
/// healthy, fabricate a successful probe, or ACK an unexecuted recommendation.
pub(crate) fn seed(
    policy: &mut Policy,
    set: &CandidateSet,
    actual: &EngineView,
    current: &Context,
    now: u64,
) -> Result<(), Rejected> {
    validate_context(current, actual)?;
    if set.revision != current.revision || set.confirmed != actual.selected {
        return Err(Rejected::SelectionMismatch);
    }
    let id = set
        .confirmed_candidate
        .as_ref()
        .ok_or(Rejected::NoEligibleSeed)?;
    if set.targets.get(id) != Some(&actual.selected)
        || !policy.seed_confirmed(id, current.epoch, now)
    {
        return Err(Rejected::NoEligibleSeed);
    }
    Ok(())
}

fn pinned(view: &EngineView, target: &EntranceKey) -> bool {
    &view.selected == target && view.pins.get(&target.node_id) == Some(&target.endpoint_key)
}

fn validate_context(context: &Context, view: &EngineView) -> Result<(), Rejected> {
    if context != &view.context
        || context.revision.is_empty()
        || context.owner.is_empty()
        || context.owner_generation == 0
        || context.runtime_id == 0
    {
        return Err(Rejected::ContextMismatch);
    }
    if !view.running {
        return Err(Rejected::EngineNotRunning);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::auto_route::{Config, Health, Reason};
    use crate::auto_route_profile::build;
    use ppvpn_account::profile::parse_profile;
    use serde_json::json;

    fn key(node: &str, endpoint: &str) -> EntranceKey {
        EntranceKey {
            node_id: node.into(),
            endpoint_key: endpoint.into(),
        }
    }
    fn context() -> Context {
        Context {
            epoch: Epoch {
                session: 1,
                network: 1,
                profile: 1,
            },
            request_epoch: 1,
            revision: "r1".into(),
            owner: "owner-1".into(),
            owner_generation: 1,
            runtime_id: 1,
        }
    }
    pub(crate) fn fixture() -> (CandidateSet, EngineView, Preferences, Policy, Proposal) {
        let ingress = |id: &str, ordinal: u32| {
            json!({"endpoint_key":id,"replica_ordinal":ordinal,"protocol":"anytls","tier_rank":1,
            "auto_selection_metadata":{"entry_tier_id":"1","entry_tier_key":"tier","quota_multiplier":"1","entry_latency_threshold_ms":200}})
        };
        let node = |id: &str, ingresses| {
            json!({"id":id,"name":"not-an-identity","entry_key":"entry","instance_id":"11111111-1111-4111-8111-111111111111",
            "exit_mode":"primary","exit":{"ip":"203.0.113.10"},"ingresses":ingresses})
        };
        let profile=parse_profile(&serde_json::to_vec(&json!({"schema_version":1,"revision":"r1",
            "nodes":[node("a",vec![ingress("a1",0),ingress("a2",1)]),node("b",vec![ingress("b1",0)])]})).unwrap()).unwrap();
        let set = build(&profile, "a", Some("a1")).unwrap();
        let actual = EngineView {
            context: context(),
            running: true,
            selected: key("a", "a1"),
            pins: BTreeMap::from([("a".into(), "a1".into()), ("b".into(), "b1".into())]),
        };
        let saved = Preferences {
            mode: Mode::Automatic,
            selected_node: "a".into(),
            pins: actual.pins.clone(),
        };
        let config = Config {
            max_quota_multiplier_milli: 999_999,
            fresh_for_ms: 10_000,
            fail_after: 2,
            recover_after: 3,
            min_dwell_ms: 0,
            min_latency_improvement_ms: 20,
            max_candidates: 256,
            max_in_flight: 2,
            max_per_round: 4,
            round_timeout_ms: 500,
            min_round_interval_ms: 1_000,
        };
        let mut policy = Policy::new(
            config,
            context().epoch,
            set.scope.clone(),
            set.candidates.clone(),
        )
        .unwrap();
        seed(&mut policy, &set, &actual, &context(), 0).unwrap();
        policy.begin_round(0).unwrap();
        loop {
            let probes = policy.plan_probes(0).start;
            if probes.is_empty() {
                break;
            }
            for probe in probes {
                let target = &set.targets[&probe.candidate.id];
                let latency_ms = if target.node_id == "b" { 20 } else { 100 };
                policy.finish_probe(&probe.token, Health::Available { latency_ms }, 0);
            }
        }
        let proposal = policy.recommend(0).change.unwrap();
        (set, actual, saved, policy, proposal)
    }
    fn transaction(
        set: &CandidateSet,
        view: &EngineView,
        saved: &Preferences,
        target: &str,
        journal: Journal,
    ) -> Transaction {
        Transaction::begin(context(), set, view.clone(), saved.clone(), target, journal).unwrap()
    }
    // Unit effect interpreter, not a CoreTransport, real engine or L2 test.
    fn reply(action: &Action, engine: &mut EngineView, saved: &mut Preferences) -> Reply {
        match action {
            Action::PinExact(target) => {
                engine
                    .pins
                    .insert(target.node_id.clone(), target.endpoint_key.clone());
                if engine.selected.node_id == target.node_id {
                    engine.selected = target.clone();
                }
                Reply::Applied
            }
            Action::SelectNode(node) => {
                engine.selected = key(
                    node,
                    engine.pins.get(node).expect("always pin before select"),
                );
                Reply::Applied
            }
            Action::ReadEngine => Reply::Engine(engine.clone()),
            Action::Save(choice) => {
                *saved = choice.clone();
                Reply::Applied
            }
            Action::ReadPreferences => Reply::Preferences(saved.clone()),
        }
    }
    fn drive(
        tx: &mut Transaction,
        engine: &mut EngineView,
        saved: &mut Preferences,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        for _ in 0..30 {
            let Some(effect) = tx.next(&context()) else {
                return actions;
            };
            let response = reply(&effect.action, engine, saved);
            actions.push(effect.action);
            tx.accept(effect.id, response, &context()).unwrap();
        }
        panic!("transaction must end without an unbounded retry");
    }

    #[test]
    fn confirmed_save_preserves_pins_outside_current_engine_profile() {
        let (set, mut engine, mut saved, _policy, proposal) = fixture();
        saved
            .pins
            .insert("other-team-node".into(), "saved-endpoint".into());
        let mut tx = transaction(
            &set,
            &engine,
            &saved,
            &proposal.candidate,
            Journal::default(),
        );
        drive(&mut tx, &mut engine, &mut saved);
        assert_eq!(saved.pins["other-team-node"], "saved-endpoint");
        assert_eq!(saved.selected_node, "b");
    }

    #[test]
    fn exact_pin_then_select_then_readback_and_persistence_precede_policy_ack() {
        let (set, mut engine, mut saved, mut policy, proposal) = fixture();
        let journal = Journal::default();
        let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
        let actions = drive(&mut tx, &mut engine, &mut saved);
        assert_eq!(actions[0], Action::PinExact(key("a", "a1")));
        assert_eq!(actions[2], Action::PinExact(key("b", "b1")));
        assert_eq!(actions[4], Action::SelectNode("b".into()));
        assert!(matches!(actions[5], Action::ReadEngine));
        assert!(matches!(actions[6], Action::Save(_)));
        assert_eq!(saved.selected_node, "b");
        assert!(matches!(
            tx.outcome(),
            Some(Outcome::EngineAndStorageConfirmed { .. })
        ));
        assert_eq!(policy.recommend(0).current, set.confirmed_candidate);
        assert!(
            journal.recovery().is_some(),
            "no premature clean journal before policy ACK"
        );
        assert!(tx.acknowledge_policy(&mut policy, &proposal, &context(), 0));
        assert!(journal.recovery().is_none());
        assert_eq!(
            policy.recommend(0).current.as_deref(),
            Some(proposal.candidate.as_str())
        );
    }

    #[test]
    fn same_node_switch_confirms_exact_endpoint_without_a_select_node_call() {
        let (set, mut engine, mut saved, _, _) = fixture();
        let id = set
            .targets
            .iter()
            .find(|(_, k)| **k == key("a", "a2"))
            .unwrap()
            .0;
        let mut tx = transaction(&set, &engine, &saved, id, Journal::default());
        let actions = drive(&mut tx, &mut engine, &mut saved);
        assert!(!actions.iter().any(|a| matches!(a, Action::SelectNode(_))));
        assert_eq!(engine.selected, key("a", "a2"));
        assert_eq!(saved.pins["a"], "a2");
    }

    #[test]
    fn rpc_ok_without_engine_confirmation_never_persists_or_reports_success() {
        let (set, mut engine, mut saved, _, proposal) = fixture();
        let mut tx = transaction(
            &set,
            &engine,
            &saved,
            &proposal.candidate,
            Journal::default(),
        );
        let fence = tx.next(&context()).unwrap();
        tx.accept(fence.id, Reply::Applied, &context()).unwrap();
        assert!(tx.outcome().is_none());
        let read = tx.next(&context()).unwrap();
        engine.pins.remove("a");
        tx.accept(read.id, Reply::Engine(engine.clone()), &context())
            .unwrap();
        let actions = drive(&mut tx, &mut engine, &mut saved);
        assert!(!actions.iter().any(|a| matches!(a, Action::Save(_))));
        assert!(!matches!(
            tx.outcome(),
            Some(Outcome::EngineAndStorageConfirmed { .. })
        ));
    }

    #[test]
    fn select_rpc_ok_with_unchanged_engine_never_persists_the_proposed_node() {
        let (set, mut engine, mut saved, _, proposal) = fixture();
        let mut tx = transaction(
            &set,
            &engine,
            &saved,
            &proposal.candidate,
            Journal::default(),
        );
        loop {
            let effect = tx.next(&context()).unwrap();
            if matches!(effect.action, Action::SelectNode(_)) {
                tx.accept(effect.id, Reply::Applied, &context()).unwrap();
                break;
            }
            let response = reply(&effect.action, &mut engine, &mut saved);
            tx.accept(effect.id, response, &context()).unwrap();
        }
        let actions = drive(&mut tx, &mut engine, &mut saved);
        assert!(!actions
            .iter()
            .any(|action| matches!(action, Action::Save(_))));
        assert_eq!(saved.selected_node, "a");
        assert!(matches!(tx.outcome(), Some(Outcome::RolledBack { .. })));
    }

    #[test]
    fn definitive_select_failure_rolls_back_and_does_not_ack_policy() {
        let (set, mut engine, mut saved, policy, proposal) = fixture();
        let original = saved.clone();
        let journal = Journal::default();
        let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
        loop {
            let e = tx.next(&context()).unwrap();
            if matches!(e.action, Action::SelectNode(_)) {
                tx.accept(e.id, Reply::Failed, &context()).unwrap();
                break;
            }
            let r = reply(&e.action, &mut engine, &mut saved);
            tx.accept(e.id, r, &context()).unwrap();
        }
        drive(&mut tx, &mut engine, &mut saved);
        assert!(matches!(tx.outcome(), Some(Outcome::RolledBack { .. })));
        assert!(journal.recovery().is_none());
        assert_eq!(engine.selected, key("a", "a1"));
        assert_eq!(saved, original);
        assert_eq!(policy.recommend(0).current, set.confirmed_candidate);
    }

    // A completed failed Select can still have changed the active route. A
    // rollback must confirm its exact pin before selecting the original node.
    fn rollback_pin_readback_case(old_pin: Option<&str>, read_fails: bool, third_party: bool) {
        let (set, mut engine, mut saved, _, proposal) = fixture();
        let journal = Journal::default();
        let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
        loop {
            let effect = tx.next(&context()).unwrap();
            let response = reply(&effect.action, &mut engine, &mut saved);
            if matches!(effect.action, Action::SelectNode(_)) {
                tx.accept(effect.id, Reply::Failed, &context()).unwrap();
                break;
            }
            tx.accept(effect.id, response, &context()).unwrap();
        }
        match old_pin {
            Some(pin) => {
                engine.pins.insert("a".into(), pin.into());
            }
            None => {
                engine.pins.remove("a");
            }
        }
        let read = tx.next(&context()).unwrap();
        assert_eq!(read.action, Action::ReadEngine);
        tx.accept(read.id, Reply::Engine(engine.clone()), &context())
            .unwrap();
        let pin = tx.next(&context()).unwrap();
        assert_eq!(pin.action, Action::PinExact(key("a", "a1")));
        // Simulate RPC Applied without the promised change taking effect.
        tx.accept(pin.id, Reply::Applied, &context()).unwrap();
        let verification = tx.next(&context()).unwrap();
        assert_eq!(
            verification.action,
            Action::ReadEngine,
            "rollback must not select before actual exact-pin readback"
        );
        if third_party {
            engine.selected = key("third-party", "unrelated");
        }
        let response = if read_fails {
            Reply::Failed
        } else {
            Reply::Engine(engine.clone())
        };
        tx.accept(verification.id, response, &context()).unwrap();
        assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
        assert!(
            journal.recovery().is_some(),
            "unsafe rollback is still dirty"
        );
        assert!(
            tx.next(&context()).is_none(),
            "no rollback Select or new write"
        );
    }

    #[test]
    fn rollback_missing_pin_with_rpc_applied_does_not_select_legacy_fallback() {
        rollback_pin_readback_case(None, false, false);
    }

    #[test]
    fn rollback_wrong_pin_with_rpc_applied_does_not_select_the_wrong_exit() {
        rollback_pin_readback_case(Some("wrong-exit"), false, false);
    }

    #[test]
    fn rollback_pin_read_failure_keeps_dirty_without_selecting() {
        rollback_pin_readback_case(Some("a1"), true, false);
    }

    #[test]
    fn rollback_pin_readback_cannot_overwrite_a_third_party_selection() {
        rollback_pin_readback_case(Some("a1"), false, true);
    }

    #[test]
    fn dirty_unused_target_pin_is_not_called_a_complete_rollback() {
        let (set, mut engine, mut saved, _, proposal) = fixture();
        engine.pins.remove("b");
        saved.pins.remove("b");
        let journal = Journal::default();
        let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
        loop {
            let e = tx.next(&context()).unwrap();
            if matches!(e.action, Action::SelectNode(_)) {
                tx.accept(e.id, Reply::Failed, &context()).unwrap();
                break;
            }
            let r = reply(&e.action, &mut engine, &mut saved);
            tx.accept(e.id, r, &context()).unwrap();
        }
        drive(&mut tx, &mut engine, &mut saved);
        assert_eq!(engine.selected, key("a", "a1"));
        assert_eq!(engine.pins["b"], "b1");
        assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
        assert!(journal.recovery().is_some());
    }

    #[test]
    fn persistence_failure_restores_previous_choice_before_clean_rollback() {
        let (set, mut engine, mut saved, policy, proposal) = fixture();
        let original = saved.clone();
        let mut tx = transaction(
            &set,
            &engine,
            &saved,
            &proposal.candidate,
            Journal::default(),
        );
        loop {
            let e = tx.next(&context()).unwrap();
            if matches!(e.action, Action::Save(_)) {
                tx.accept(e.id, Reply::Failed, &context()).unwrap();
                break;
            }
            let r = reply(&e.action, &mut engine, &mut saved);
            tx.accept(e.id, r, &context()).unwrap();
        }
        let actions = drive(&mut tx, &mut engine, &mut saved);
        assert!(actions
            .iter()
            .any(|a| matches!(a,Action::Save(choice)if *choice==original)));
        assert_eq!(saved, original);
        assert!(matches!(tx.outcome(), Some(Outcome::RolledBack { .. })));
        assert_eq!(policy.recommend(0).current, set.confirmed_candidate);
    }

    #[test]
    fn failed_persistence_restore_leaves_an_explicit_storage_engine_mismatch() {
        let (set, mut engine, mut saved, _, proposal) = fixture();
        let original = saved.clone();
        let journal = Journal::default();
        let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
        loop {
            let effect = tx.next(&context()).unwrap();
            let response = reply(&effect.action, &mut engine, &mut saved);
            if matches!(effect.action, Action::ReadPreferences) {
                // The write completed, but verification failed. Its restoration
                // must be confirmed too, never assumed from attempted rollback.
                tx.accept(effect.id, Reply::Preferences(original.clone()), &context())
                    .unwrap();
                break;
            }
            tx.accept(effect.id, response, &context()).unwrap();
        }
        loop {
            let effect = tx.next(&context()).unwrap();
            if matches!(effect.action, Action::Save(_)) {
                tx.accept(effect.id, Reply::Failed, &context()).unwrap();
                break;
            }
            let response = reply(&effect.action, &mut engine, &mut saved);
            tx.accept(effect.id, response, &context()).unwrap();
        }
        assert_eq!(engine.selected, key("a", "a1"));
        assert_eq!(saved.selected_node, "b");
        assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
        assert!(journal.recovery().is_some());
        assert!(tx.next(&context()).is_none());
    }

    #[test]
    fn changed_owner_in_actual_engine_reply_blocks_writes_even_if_local_epoch_has_not_updated() {
        let (set, engine, saved, _, proposal) = fixture();
        let mut tx = transaction(
            &set,
            &engine,
            &saved,
            &proposal.candidate,
            Journal::default(),
        );
        let effect = tx.next(&context()).unwrap();
        tx.accept(effect.id, Reply::Applied, &context()).unwrap();
        let effect = tx.next(&context()).unwrap();
        let mut changed = engine;
        changed.context.owner = "replacement-owner".into();
        tx.accept(effect.id, Reply::Engine(changed), &context())
            .unwrap();
        assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
        assert!(tx.next(&context()).is_none());
    }

    #[test]
    fn uncertain_remote_completion_stops_without_a_blind_rollback_or_new_write() {
        for stop_at in [0, 2, 4, 6] {
            let (set, mut engine, mut saved, _, proposal) = fixture();
            let journal = Journal::default();
            let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
            for index in 0..=stop_at {
                let e = tx.next(&context()).unwrap();
                if index == stop_at {
                    tx.accept(e.id, Reply::Uncertain, &context()).unwrap();
                } else {
                    let r = reply(&e.action, &mut engine, &mut saved);
                    tx.accept(e.id, r, &context()).unwrap();
                }
            }
            assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
            assert!(tx.next(&context()).is_none());
            assert!(journal.recovery().is_some());
        }
    }

    #[test]
    fn cancellation_after_pin_select_or_persist_dispatch_retains_recovery_marker() {
        for stop_at in [0, 2, 4, 6] {
            let (set, mut engine, mut saved, _, proposal) = fixture();
            let journal = Journal::default();
            let initial = engine.clone();
            let original = saved.clone();
            let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
            for index in 0..=stop_at {
                let e = tx.next(&context()).unwrap();
                if index == stop_at {
                    break;
                }
                let r = reply(&e.action, &mut engine, &mut saved);
                tx.accept(e.id, r, &context()).unwrap();
            }
            drop(tx);
            assert!(journal.recovery().is_some());
            assert!(matches!(
                Transaction::begin(
                    context(),
                    &set,
                    initial,
                    original,
                    &proposal.candidate,
                    journal
                ),
                Err(Rejected::ReconciliationRequired)
            ));
        }
    }

    #[test]
    fn cancel_before_dispatch_releases_reservation_without_claiming_a_mutation() {
        let (set, engine, saved, _, proposal) = fixture();
        let journal = Journal::default();
        let tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
        assert!(matches!(
            Transaction::begin(
                context(),
                &set,
                engine.clone(),
                saved.clone(),
                &proposal.candidate,
                journal.clone()
            ),
            Err(Rejected::Busy)
        ));
        drop(tx);
        assert!(journal.recovery().is_none());
        assert!(
            Transaction::begin(context(), &set, engine, saved, &proposal.candidate, journal)
                .is_ok()
        );
    }

    #[test]
    fn epoch_owner_revision_and_runtime_changes_stop_every_pending_stage() {
        for stop_at in 0..9 {
            for dimension in 0..6 {
                let (set, mut engine, mut saved, _, proposal) = fixture();
                let journal = Journal::default();
                let mut tx =
                    transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
                for index in 0..=stop_at {
                    let e = tx.next(&context()).unwrap();
                    if index == stop_at {
                        break;
                    }
                    let r = reply(&e.action, &mut engine, &mut saved);
                    tx.accept(e.id, r, &context()).unwrap();
                }
                let mut changed = context();
                match dimension {
                    0 => changed.epoch.network += 1,
                    1 => changed.revision = "r2".into(),
                    2 => changed.owner = "new-owner".into(),
                    3 => changed.owner_generation += 1,
                    4 => changed.runtime_id += 1,
                    _ => changed.request_epoch += 1,
                }
                assert!(tx.next(&changed).is_none());
                assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
                assert!(journal.recovery().is_some());
            }
        }
    }

    #[test]
    fn changed_context_at_async_completion_is_not_accepted_even_before_next_dispatch() {
        let (set, engine, saved, _, proposal) = fixture();
        let mut tx = transaction(
            &set,
            &engine,
            &saved,
            &proposal.candidate,
            Journal::default(),
        );
        let e = tx.next(&context()).unwrap();
        let mut changed = context();
        changed.epoch.profile += 1;
        tx.accept(e.id, Reply::Applied, &changed).unwrap();
        assert!(tx.next(&changed).is_none());
        assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
    }

    #[test]
    fn stopped_or_changed_engine_view_is_never_confirmed() {
        let (set, engine, saved, _, proposal) = fixture();
        let mut tx = transaction(
            &set,
            &engine,
            &saved,
            &proposal.candidate,
            Journal::default(),
        );
        let e = tx.next(&context()).unwrap();
        tx.accept(e.id, Reply::Applied, &context()).unwrap();
        let e = tx.next(&context()).unwrap();
        let mut stopped = engine.clone();
        stopped.running = false;
        tx.accept(e.id, Reply::Engine(stopped), &context()).unwrap();
        assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
        assert!(tx.next(&context()).is_none());
    }

    #[test]
    fn duplicate_out_of_order_and_wrong_kind_replies_never_advance_or_redispatch() {
        let (set, engine, saved, _, proposal) = fixture();
        let mut tx = transaction(
            &set,
            &engine,
            &saved,
            &proposal.candidate,
            Journal::default(),
        );
        let e = tx.next(&context()).unwrap();
        assert!(tx.next(&context()).is_none());
        assert_eq!(
            tx.accept(e.id + 1, Reply::Applied, &context()),
            Err(Rejected::UnexpectedReply)
        );
        assert_eq!(
            tx.accept(e.id, Reply::Engine(engine), &context()),
            Err(Rejected::UnexpectedReply)
        );
        tx.accept(e.id, Reply::Applied, &context()).unwrap();
        assert_eq!(
            tx.accept(e.id, Reply::Applied, &context()),
            Err(Rejected::UnexpectedReply)
        );
        assert!(matches!(
            tx.next(&context()).unwrap().action,
            Action::ReadEngine
        ));
    }

    #[test]
    fn no_seed_without_matching_actual_engine_context_and_confirmed_endpoint() {
        let (set, actual, _, mut policy, _) = fixture();
        let mut wrong = actual.clone();
        wrong.selected = key("b", "b1");
        assert_eq!(
            seed(&mut policy, &set, &wrong, &context(), 0),
            Err(Rejected::SelectionMismatch)
        );
        wrong = actual.clone();
        wrong.context.revision = "other".into();
        assert_eq!(
            seed(&mut policy, &set, &wrong, &context(), 0),
            Err(Rejected::ContextMismatch)
        );
    }

    #[test]
    fn seed_does_not_fabricate_freshness_or_a_successful_probe() {
        let (set, actual, _, mut policy, _) = fixture();
        let mut newer = context();
        newer.epoch.network += 1;
        policy.set_epoch(newer.epoch);
        let mut actual = actual;
        actual.context = newer.clone();
        seed(&mut policy, &set, &actual, &newer, 1_000).unwrap();
        assert_eq!(policy.recommend(1_000).reason, Reason::NoQualifiedCandidate);
        assert_eq!(policy.recommend(1_000).current, set.confirmed_candidate);
    }

    #[test]
    fn invalid_initial_profile_selection_or_saved_auto_state_never_dispatches() {
        let (set, engine, saved, _, proposal) = fixture();
        let mut wrong = engine.clone();
        wrong.context.revision = "r2".into();
        assert!(matches!(
            Transaction::begin(
                context(),
                &set,
                wrong,
                saved.clone(),
                &proposal.candidate,
                Journal::default()
            ),
            Err(Rejected::ContextMismatch)
        ));
        let mut wrong = saved.clone();
        wrong.selected_node = "b".into();
        assert!(matches!(
            Transaction::begin(
                context(),
                &set,
                engine.clone(),
                wrong,
                &proposal.candidate,
                Journal::default()
            ),
            Err(Rejected::SavedAutomaticChoiceMismatch)
        ));
        assert!(matches!(
            Transaction::begin(
                context(),
                &set,
                engine,
                saved,
                "missing",
                Journal::default()
            ),
            Err(Rejected::TargetUnknown)
        ));
    }

    #[test]
    fn expired_policy_ack_leaves_engine_and_storage_truth_as_reconciliation() {
        let (set, mut engine, mut saved, mut policy, proposal) = fixture();
        let journal = Journal::default();
        let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
        drive(&mut tx, &mut engine, &mut saved);
        assert!(!tx.acknowledge_policy(&mut policy, &proposal, &context(), 10_001));
        assert!(journal.recovery().is_some());
        assert!(matches!(tx.outcome(), Some(Outcome::NeedsReconcile { .. })));
        assert_eq!(engine.selected, key("b", "b1"));
        assert_eq!(saved.selected_node, "b");
        assert_eq!(policy.recommend(10_001).current, set.confirmed_candidate);
    }

    #[test]
    fn successful_persistence_without_policy_ack_is_dirty_on_drop() {
        let (set, mut engine, mut saved, _, proposal) = fixture();
        let journal = Journal::default();
        let mut tx = transaction(&set, &engine, &saved, &proposal.candidate, journal.clone());
        drive(&mut tx, &mut engine, &mut saved);
        drop(tx);
        assert!(journal.recovery().is_some());
    }
}
