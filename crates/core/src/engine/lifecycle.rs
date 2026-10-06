//! apply, start and stop (docs/host-integration.md, 4.1 and 4.2), and the
//! watcher that follows the runtime between calls.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Instant;

use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;

use super::state::{Applied, Switched, TunRoutingSignal};
use super::{not_applied, now, shut_down, Error, Inner};
use crate::config::Role;
use crate::error::codes;
use crate::event::Event;
use crate::profile::Profile;
use crate::request::{
    validate_request, ApplyRequest, ApplyResult, ClearedPin, PinClearReason, SwitchKind,
};
use crate::runtime::{GroupSwitch, RuntimeState};
use crate::status::{FatalReason, TunRouting};
use crate::translate::{self, Translation, SELECTED_TAG};

/// What `ReloadFailed` says when the candidate profile is the problem (Go's
/// words, kept for the golden).
const CANDIDATE_FAILED: &str = "candidate validation or build failed";
/// What it says when the runtime refused the new configuration.
const RELOAD_REFUSED: &str = "the runtime refused the new configuration";

/// Go's `ingress tls` line (tlsdebug.go `logIngressTLS`): at debug level,
/// one per REALITY or TLS ingress of an applied profile, with fingerprints
/// of the values the core will use, so they can be compared with the
/// server's without logging them: the first 10 hex digits of the SHA-256 of
/// the public key and short ID strings exactly as received, their lengths
/// and the key's encoding, plus the server name, uTLS fingerprint and flow.
fn log_ingress_tls(profile: &Profile) {
    for node in &profile.nodes {
        for ingress in &node.ingresses {
            let Some(tls) = &ingress.tls else {
                continue;
            };
            let flow = ingress
                .credentials
                .vless
                .as_ref()
                .map_or("", |vless| vless.flow.as_str());
            match &tls.reality {
                Some(reality) => tracing::debug!(
                    node_id = %node.id,
                    endpoint_key = %ingress.endpoint_key,
                    protocol = %ingress.protocol,
                    server_name = %tls.server_name,
                    public_key_sha256 = %short_digest(&reality.public_key),
                    public_key_len = reality.public_key.len(),
                    public_key_encoding = base64_flavor(&reality.public_key),
                    short_id_sha256 = %short_digest(&reality.short_id),
                    short_id_len = reality.short_id.len(),
                    fingerprint = "chrome",
                    flow,
                    "ingress tls"
                ),
                None => tracing::debug!(
                    node_id = %node.id,
                    endpoint_key = %ingress.endpoint_key,
                    protocol = %ingress.protocol,
                    server_name = %tls.server_name,
                    insecure = tls.insecure,
                    flow,
                    "ingress tls"
                ),
            }
        }
    }
}

fn short_digest(value: &str) -> String {
    let sum = Sha256::digest(value.as_bytes());
    sum[..5].iter().map(|b| format!("{b:02x}")).collect()
}

/// A base64 string's padding and alphabet from the characters it uses:
/// "unpadded url" is base64 raw-url, the form REALITY keys use; a key
/// without '-', '_', '+' or '/' fits either alphabet.
pub(super) fn base64_flavor(value: &str) -> &'static str {
    let padded = value.ends_with('=');
    match (
        padded,
        value.contains(['+', '/']),
        value.contains(['-', '_']),
    ) {
        (true, true, _) => "padded std",
        (true, false, true) => "padded url",
        (true, false, false) => "padded url-or-std",
        (false, true, _) => "unpadded std",
        (false, false, true) => "unpadded url",
        (false, false, false) => "unpadded url-or-std",
    }
}

impl Inner {
    /// Applies `request`, then one info line with how long each phase took
    /// (Go's `apply timing`); an apply that changes nothing writes none.
    pub(super) async fn apply(
        self: &Arc<Self>,
        request: ApplyRequest,
    ) -> Result<ApplyResult, Error> {
        let mut timer = PhaseTimer::new();
        let mut sets = (0, 0, 0);
        let result = self.apply_phases(request, &mut timer, &mut sets).await;
        if !matches!(&result, Ok(r) if !r.applied) {
            let (ready, stale, unavailable) = sets;
            tracing::info!(
                outcome = outcome(&result),
                tun = self.config.role == Role::Tun,
                rule_sets_ready = ready,
                rule_sets_stale = stale,
                rule_sets_unavailable = unavailable,
                validate_ms = timer.ms("validate"),
                rule_sets_ms = timer.ms("rule_sets"),
                wait_ms = timer.ms("wait"),
                host_ipv6_ms = timer.ms("host_ipv6"),
                build_ms = timer.ms("build"),
                check_ms = timer.ms("check"),
                kernel_switch_ms = timer.ms("kernel_switch"),
                full_restart_ms = timer.ms("full_restart"),
                total_ms = timer.total_ms(),
                "apply timing"
            );
        }
        result
    }

    /// `apply`'s work, its phases marked on `timer`; `sets` gets the rule
    /// sets by state (ready, stale, unavailable).
    async fn apply_phases(
        self: &Arc<Self>,
        request: ApplyRequest,
        timer: &mut PhaseTimer,
        sets: &mut (usize, usize, usize),
    ) -> Result<ApplyResult, Error> {
        self.admit()?;
        // D3: the profile as given, before any selection is carried over.
        let profile = validate_request(&request, Utc::now())
            .map_err(|e| self.reload_failed(e, CANDIDATE_FAILED))?;

        // The host's selection if the new profile has it, else the default.
        let (selected, selection_reset) = match request.selected_node_id.as_deref() {
            Some(id) if profile.nodes.iter().any(|n| n.id == id) => (id.to_owned(), false),
            requested => (
                profile.selection.default_node_id.clone(),
                requested.is_some(),
            ),
        };
        // Pins the new profile still has; the others are cleared.
        let mut pins = BTreeMap::new();
        let mut cleared_pins = Vec::new();
        for pin in &request.pins {
            let reason = match profile.nodes.iter().find(|n| n.id == pin.node_id) {
                None => Some(PinClearReason::NodeRemoved),
                Some(node)
                    if !node
                        .ingresses
                        .iter()
                        .any(|i| i.endpoint_key == pin.endpoint_key) =>
                {
                    Some(PinClearReason::IngressRemoved)
                }
                Some(_) => None,
            };
            match reason {
                Some(reason) => cleared_pins.push(ClearedPin {
                    node_id: pin.node_id.clone(),
                    endpoint_key: pin.endpoint_key.clone(),
                    reason,
                }),
                None => {
                    pins.insert(pin.node_id.clone(), pin.endpoint_key.clone());
                }
            }
        }

        // The dedupe key against the live values, select and pin included,
        // and the hosts rule sets may come from (a new list may let a set be
        // fetched).
        let same_hosts = self.rule_set_hosts() == request.allowed_rule_set_hosts;
        let unchanged = same_hosts
            && self.live().applied.as_ref().is_some_and(|a| {
                a.profile.revision == profile.revision
                    && a.mode == request.routing_mode
                    && a.selected == selected
                    && a.pins == pins
            });
        if unchanged {
            // Nothing done, nothing sent; the result still tells the host
            // what its persisted selection and pins should be.
            return Ok(ApplyResult {
                applied: false,
                revision: profile.revision,
                selected_node_id: selected,
                selection_reset,
                cleared_pins,
                switch: None,
                listeners: Vec::new(),
            });
        }

        timer.mark("validate");
        // Rule sets never fail an apply: a set that cannot be had degrades
        // its rules. Their downloads (up to PREPARE_TIMEOUT) run before the
        // operation lock, so that a shutdown neither waits for them nor
        // loses its budget to them; it cancels them.
        let Some(rule_sets) = self
            .prepare_rule_sets(
                &profile,
                request.routing_mode,
                &request.allowed_rule_set_hosts,
            )
            .await
        else {
            return Err(shut_down());
        };
        timer.mark("rule_sets");
        *sets = rule_sets.counts();
        // Another lifecycle call may hold the lock.
        let _op = self.op.lock().await;
        self.admit()?;
        timer.mark("wait");
        let running = {
            let live = self.live();
            live.running
                .then(|| live.applied.as_ref().map(|a| a.translation.clone()))
                .flatten()
        };
        self.probe_host_ipv6();
        timer.mark("host_ipv6");
        let files = rule_sets.files();
        let build = || {
            let mut options = self.options(request.routing_mode, &selected, &pins);
            options.rule_sets = files.clone();
            translate::translate(&profile, &options)
        };
        let mut translation = build().map_err(|e| self.reload_failed(e, CANDIDATE_FAILED))?;
        timer.mark("build");
        let mut listeners = Vec::new();
        let switch = if let Some(running) = &running {
            let switched = self
                .switch_to(running, translation, &build)
                .await
                .map_err(|error| self.reload_failed(error, RELOAD_REFUSED))?;
            translation = switched.translation;
            listeners = switched.listeners;
            let switch = switched.kind;
            if switch == SwitchKind::KernelSwitch {
                self.kernel_switched(&profile.revision, switched.closed, switched.kept);
                self.reassert(&translation, &selected, &pins).await;
                timer.mark("kernel_switch");
            } else {
                timer.mark("full_restart");
            }
            Some(switch)
        } else {
            // Not running: kept for start, so it must load.
            let json = translation.json.clone();
            tokio::task::spawn_blocking(move || translate::check(&json))
                .await
                .map_err(|e| {
                    Error::new(codes::CORE_OPERATION_FAILED, false, format!("check: {e}"))
                })?
                .map_err(|e| self.reload_failed(e, CANDIDATE_FAILED))?;
            timer.mark("check");
            None
        };

        let revision = profile.revision.clone();
        self.log.span().in_scope(|| log_ingress_tls(&profile));
        {
            let mut live = self.live();
            live.applied = Some(Applied {
                profile,
                mode: request.routing_mode,
                selected: selected.clone(),
                pins,
                translation,
            });
            self.settle(&mut live);
            self.publish(Event::ProfileApplied {
                at: now(),
                revision: revision.clone(),
            });
            for pin in &cleared_pins {
                self.publish(Event::NodeIngressPinCleared {
                    at: now(),
                    revision: revision.clone(),
                    node_id: pin.node_id.clone(),
                    endpoint_key: pin.endpoint_key.clone(),
                    reason: pin.reason,
                });
            }
        }
        self.activate_rule_sets(rule_sets, request.allowed_rule_set_hosts);
        // A pin the request added, or one it cleared.
        self.sync_pinned_checks();
        if running.is_some() {
            self.refresh().await;
        }
        Ok(ApplyResult {
            applied: true,
            revision,
            selected_node_id: selected,
            selection_reset,
            cleared_pins,
            switch,
            listeners,
        })
    }

    /// Starts the applied profile, then one info line with how long each
    /// phase took (Go's `start timing`); none when already running.
    pub(super) async fn start(self: &Arc<Self>) -> Result<(), Error> {
        self.admit()?;
        let _op = self.op.lock().await;
        self.admit()?;
        if self.live().running {
            return Ok(());
        }
        let mut timer = PhaseTimer::new();
        let result = self.start_phases(&mut timer).await;
        tracing::info!(
            outcome = outcome(&result),
            tun = self.config.role == Role::Tun,
            local_proxy_ms = timer.ms("local_proxy"),
            host_ipv6_ms = timer.ms("host_ipv6"),
            build_ms = timer.ms("build"),
            engine_start_ms = timer.ms("engine_start"),
            total_ms = timer.total_ms(),
            "start timing"
        );
        result
    }

    /// `start`'s work under the operation lock, not running, its phases
    /// marked on `timer`.
    async fn start_phases(self: &Arc<Self>, timer: &mut PhaseTimer) -> Result<(), Error> {
        let (profile, mode, selected, pins) = {
            let live = self.live();
            let Some(a) = &live.applied else {
                return Err(not_applied());
            };
            (
                a.profile.clone(),
                a.mode,
                a.selected.clone(),
                a.pins.clone(),
            )
        };
        // The listeners' ports may have been taken while stopped.
        self.prepare_listeners()?;
        timer.mark("local_proxy");
        self.probe_host_ipv6();
        timer.mark("host_ipv6");
        // Translated again: select and pin may have moved since the apply.
        let mut translation =
            translate::translate(&profile, &self.options(mode, &selected, &pins))?;
        timer.mark("build");
        self.live().needs_stop = true;
        let mut started = self.runtime.start(&translation.json).await;
        if let Err(e) = &started {
            // The shared listener may have been taken since its port was
            // checked: the run goes without it (section 4.6).
            if e.code != "panicked" && self.leave_out_local_proxy() {
                tracing::warn!(error = %e, "start failed with the local proxy listener, starting without it");
                translation =
                    translate::translate(&profile, &self.options(mode, &selected, &pins))?;
                started = self.runtime.start(&translation.json).await;
            }
        }
        // sail's start: parse, outbounds, DNS, routing, inbounds (the TUN
        // and its routes included); not divided further.
        timer.mark("engine_start");
        if let Err(e) = started {
            return Err(self.runtime_error(&e));
        }
        self.kernel_started();
        let replaced_routes = self.runtime.replaced_routes();
        let run = {
            let mut live = self.live();
            live.running = true;
            live.run += 1;
            live.replaced_routes = replaced_routes;
            live.local_proxy_unavailable = self.local_proxy_left_out();
            if let Some(applied) = live.applied.as_mut() {
                applied.translation = translation;
            }
            self.settle(&mut live);
            self.publish(Event::CoreStarted { at: now() });
            live.local_proxy_unavailable.then_some(live.run)
        };
        if let Some(run) = run {
            self.retry_local_proxy(run);
        }
        self.network_started();
        self.guard_started();
        self.sync_pinned_checks();
        self.refresh().await;
        Ok(())
    }

    pub(super) async fn stop(self: &Arc<Self>) -> Result<(), Error> {
        self.admit()?;
        let _op = self.op.lock().await;
        self.admit()?;
        let running = self.live().running;
        if !running {
            return Ok(());
        }
        // Before the TUN closes: sail's cleanup must not be undone.
        self.guard_stopped();
        self.stop_pinned_checks();
        self.cancel_dns_queries();
        if let Err(e) = self.runtime.stop().await {
            let error = self.runtime_error(&e);
            // Still running: the TUN stays, and so does its guard.
            self.guard_restarted();
            self.sync_pinned_checks();
            return Err(error);
        }
        self.network_stopped();
        self.local_proxy_stopped();
        let mut live = self.live();
        live.running = false;
        live.needs_stop = false;
        live.run += 1;
        live.clear_runtime();
        self.settle(&mut live);
        self.publish(Event::CoreStopped { at: now() });
        Ok(())
    }

    /// Sends `ReloadFailed` for `error` and returns it.
    fn reload_failed(&self, error: Error, message: &str) -> Error {
        self.publish(Event::ReloadFailed {
            at: now(),
            code: error.code.into(),
            message: message.into(),
        });
        error
    }

    /// After a reload: sail keeps a group's selection across a reload, so
    /// the selection and the pins are set again from what was applied.
    async fn reassert(&self, t: &Translation, selected: &str, pins: &BTreeMap<String, String>) {
        if let Some(tag) = t.node_tags.get(selected) {
            if let Err(e) = self.runtime.select(SELECTED_TAG, tag).await {
                tracing::warn!(error = %e, "cannot select the node after a reload");
            }
        }
        for (node_tag, auto) in &t.groups {
            let pinned = t
                .outbound_nodes
                .get(node_tag)
                .and_then(|node| pins.get(node))
                .and_then(|key| {
                    t.members
                        .get(node_tag)?
                        .iter()
                        .find(|m| t.ingress_keys.get(*m) == Some(key))
                });
            let member = pinned.unwrap_or(auto);
            if let Err(e) = self.runtime.select(node_tag, member).await {
                tracing::warn!(error = %e, "cannot set a pin after a reload");
            }
        }
    }

    /// Reads the runtime's groups, traffic and connections into the
    /// snapshot (the queries are synchronous and never wait on sail), unless
    /// the run it read them from is over.
    pub(super) async fn refresh(&self) {
        let run = {
            let live = self.live();
            if !live.running {
                return;
            }
            live.run
        };
        let groups = self.runtime.groups().await;
        let traffic = self.runtime.traffic().await;
        let connections = self.runtime.connections().await;
        let mut live = self.live();
        if !live.running || live.run != run {
            return;
        }
        if let Ok(groups) = groups {
            live.groups = groups.into_iter().map(|g| (g.tag.clone(), g)).collect();
        }
        if let Ok(traffic) = traffic {
            live.traffic = traffic;
            live.traffic_at = Some(now());
        }
        if let Ok(connections) = connections {
            live.connections = connections;
        }
    }

    /// A group moved: a node's fallback group failing over (not pinned) is
    /// `NodeIngressSwitched`.
    async fn on_switch(&self, switch: GroupSwitch) {
        self.refresh().await;
        let mut live = self.live();
        if !live.running {
            return;
        }
        let Some(applied) = &live.applied else {
            return;
        };
        let t = &applied.translation;
        let Some(node_id) = t
            .groups
            .iter()
            .find(|(_, auto)| **auto == switch.group)
            .and_then(|(node_tag, _)| t.outbound_nodes.get(node_tag))
            .cloned()
        else {
            return;
        };
        if applied.pins.contains_key(&node_id) {
            return;
        }
        let (Some(from), Some(to)) = (
            t.ingress_keys.get(&switch.from).cloned(),
            t.ingress_keys.get(&switch.to).cloned(),
        ) else {
            return;
        };
        let at = now();
        live.switched.insert(
            node_id.clone(),
            Switched {
                previous_endpoint_key: from.clone(),
                at,
            },
        );
        self.publish(Event::NodeIngressSwitched {
            at,
            node_id,
            endpoint_key: to,
            previous_endpoint_key: from,
        });
    }

    /// The runtime ended on its own while running: nothing brings it back
    /// but a new instance (Fatal).
    fn on_runtime_state(&self, state: RuntimeState) {
        let RuntimeState::Failed { code, .. } = state else {
            return;
        };
        let mut live = self.live();
        if !live.running || live.fatal.is_some() || (live.restarting && code != "panicked") {
            return;
        }
        live.running = false;
        live.run += 1;
        live.clear_runtime();
        live.fatal = Some(if code == "panicked" {
            FatalReason::Panic
        } else {
            FatalReason::KernelUnrecoverable
        });
        self.settle(&mut live);
        drop(live);
        // The TUN is gone with the runtime.
        self.guard_stopped();
        self.stop_pinned_checks();
        // What it had opened goes now, not at the host's shutdown.
        self.fatal_cleanup();
    }

    /// The default interface changed (`None`: none). Offline while running
    /// is `Degraded{NoDefaultInterface}`; `NetworkChanged` is sent while
    /// running, as Go's. Its source is sail's network watch (network.rs).
    pub(super) fn on_network(&self, interface: Option<(&str, u32)>) {
        let mut live = self.live();
        live.offline = interface.is_none();
        if live.running {
            let (name, index) = interface.unwrap_or_default();
            self.publish(Event::NetworkChanged {
                at: now(),
                has_default_interface: interface.is_some(),
                interface_name: name.into(),
                interface_index: index,
            });
        }
        self.settle(&mut live);
    }
}

impl Inner {
    /// The TUN routing guard's report: Restoring and Unguarded degrade,
    /// Restored ends both (`TunRoutingRestored`), Broken is Fatal
    /// (`TunRoutingBroken`).
    pub(super) fn on_tun_routing(&self, signal: TunRoutingSignal) {
        let mut live = self.live();
        match signal {
            TunRoutingSignal::Restoring => live.tun_routing = Some(TunRouting::Restoring),
            TunRoutingSignal::Unguarded => live.tun_routing = Some(TunRouting::Unguarded),
            TunRoutingSignal::Restored { missing } => {
                live.tun_routing = None;
                self.publish(Event::TunRoutingRestored { at: now(), missing });
            }
            TunRoutingSignal::Broken { missing, error } => {
                self.publish(Event::TunRoutingBroken {
                    at: now(),
                    missing: missing.clone(),
                    error,
                });
                if live.fatal.is_none() {
                    live.fatal = Some(FatalReason::TunRoutingBroken { missing });
                }
            }
        }
        self.settle(&mut live);
    }
}

/// How long each phase of an apply or a start took, for its one info line
/// (Go's phaseTimer).
struct PhaseTimer {
    started: Instant,
    last: Instant,
    phases: Vec<(&'static str, u64)>,
}

impl PhaseTimer {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last: now,
            phases: Vec::new(),
        }
    }

    /// Ends the phase that began at the previous mark (or at creation).
    fn mark(&mut self, phase: &'static str) {
        let now = Instant::now();
        self.phases.push((phase, millis(now - self.last)));
        self.last = now;
    }

    /// A phase's milliseconds; None (no field) for a phase that did not
    /// end.
    fn ms(&self, phase: &str) -> Option<u64> {
        self.phases
            .iter()
            .find(|(name, _)| *name == phase)
            .map(|(_, ms)| *ms)
    }

    fn total_ms(&self) -> u64 {
        millis(self.started.elapsed())
    }
}

fn millis(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// A timing line's `outcome`.
fn outcome<T>(result: &Result<T, Error>) -> &'static str {
    if result.is_ok() {
        "ok"
    } else {
        "failed"
    }
}

/// Follows the runtime from `new` until the instance goes: group switches,
/// failed dials, changes of the TUN's routing by others, its state and the
/// network, as they happen; no timer of its own (the figures are read while
/// the host reads them, reads.rs). Needs a tokio runtime; without one (no
/// current handle) nothing is followed.
pub(super) fn spawn_watcher(inner: &Arc<Inner>) -> Option<JoinHandle<()>> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    let mut switches = inner.runtime.group_switches();
    let mut states = inner.runtime.states();
    let mut networks = inner.runtime.network_changes();
    let mut failures = inner.runtime.dial_failures();
    let mut systems = inner.runtime.system_changes();
    // Routed connections and DNS exchanges only at debug: sail builds them
    // only for a subscriber, and only the debug lines use them.
    let debug = inner.config.log.level == crate::config::LogLevel::Debug;
    let mut routes = debug.then(|| inner.runtime.routes());
    let mut exchanges = debug.then(|| inner.runtime.dns_exchanges());
    let weak: Weak<Inner> = Arc::downgrade(inner);
    Some(handle.spawn(async move {
        let mut switches_open = true;
        let mut failures_open = true;
        let mut systems_open = true;
        loop {
            tokio::select! {
                switch = switches.recv(), if switches_open => match switch {
                    Some(switch) => {
                        let Some(inner) = weak.upgrade() else { return };
                        inner.on_switch(switch).await;
                    }
                    None => switches_open = false,
                },
                failed = failures.recv(), if failures_open => match failed {
                    Some(failed) => {
                        let Some(inner) = weak.upgrade() else { return };
                        inner.on_dial_failed(failed).await;
                    }
                    None => failures_open = false,
                },
                changed = systems.recv(), if systems_open => match changed {
                    Some(changed) => {
                        let Some(inner) = weak.upgrade() else { return };
                        inner.on_system_change(changed);
                    }
                    None => systems_open = false,
                },
                routed = async {
                    match routes.as_mut() {
                        Some(routes) => routes.recv().await,
                        None => std::future::pending().await,
                    }
                } => match routed {
                    Some(routed) => {
                        let Some(inner) = weak.upgrade() else { return };
                        inner.on_routed(&routed);
                    }
                    None => routes = None,
                },
                exchanged = async {
                    match exchanges.as_mut() {
                        Some(exchanges) => exchanges.recv().await,
                        None => std::future::pending().await,
                    }
                } => match exchanged {
                    Some(exchanged) => {
                        let Some(inner) = weak.upgrade() else { return };
                        inner.on_dns_exchange(&exchanged);
                    }
                    None => exchanges = None,
                },
                changed = networks.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let change = networks.borrow_and_update().clone();
                    let Some(inner) = weak.upgrade() else { return };
                    if let Some(change) = change {
                        inner.on_network_change(change);
                    }
                }
                changed = states.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let state = states.borrow_and_update().clone();
                    let Some(inner) = weak.upgrade() else { return };
                    inner.on_runtime_state(state);
                }
            }
        }
    }))
}
