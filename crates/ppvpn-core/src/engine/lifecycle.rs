//! apply, start and stop (docs/host-integration.md, 4.1 and 4.2), and the
//! watcher that follows the runtime between calls.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use chrono::Utc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::state::{Applied, Switched, TunRoutingSignal};
use super::{not_applied, now, Error, Inner};
use crate::error::codes;
use crate::event::Event;
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

/// How often the watcher reads the runtime's groups (health), traffic and
/// connections while running.
const REFRESH: Duration = Duration::from_secs(1);

impl Inner {
    pub(super) async fn apply(&self, request: ApplyRequest) -> Result<ApplyResult, Error> {
        self.admit()?;
        let _op = self.op.lock().await;
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

        // The dedupe key against the live values, select and pin included.
        let (running, unchanged) = {
            let live = self.live();
            let running = live
                .running
                .then(|| live.applied.as_ref().map(|a| a.translation.clone()))
                .flatten();
            let unchanged = live.applied.as_ref().is_some_and(|a| {
                a.profile.revision == profile.revision
                    && a.mode == request.routing_mode
                    && a.selected == selected
                    && a.pins == pins
            });
            (running, unchanged)
        };
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
            });
        }

        self.probe_host_ipv6();
        let options = self.options(request.routing_mode, &selected, &pins);
        let translation = translate::translate(&profile, &options)
            .map_err(|e| self.reload_failed(e, CANDIDATE_FAILED))?;
        let switch = if let Some(running) = &running {
            let switch = self
                .switch_to(running, &translation)
                .await
                .map_err(|error| self.reload_failed(error, RELOAD_REFUSED))?;
            if switch == SwitchKind::KernelSwitch {
                self.reassert(&translation, &selected, &pins).await;
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
            None
        };

        let revision = profile.revision.clone();
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
        })
    }

    pub(super) async fn start(self: &Arc<Self>) -> Result<(), Error> {
        self.admit()?;
        let _op = self.op.lock().await;
        self.admit()?;
        let (profile, mode, selected, pins) = {
            let live = self.live();
            if live.running {
                return Ok(());
            }
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
        self.probe_host_ipv6();
        // Translated again: select and pin may have moved since the apply.
        let mut translation =
            translate::translate(&profile, &self.options(mode, &selected, &pins))?;
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
        if let Err(e) = started {
            return Err(self.runtime_error(&e));
        }
        let run = {
            let mut live = self.live();
            live.running = true;
            live.run += 1;
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
        self.refresh().await;
        Ok(())
    }

    pub(super) async fn stop(&self) -> Result<(), Error> {
        self.admit()?;
        let _op = self.op.lock().await;
        self.admit()?;
        let running = self.live().running;
        if !running {
            return Ok(());
        }
        if let Err(e) = self.runtime.stop().await {
            return Err(self.runtime_error(&e));
        }
        self.network_stopped();
        self.local_proxy_stopped();
        let mut live = self.live();
        live.running = false;
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
    #[allow(dead_code)] // its source is not wired yet
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

/// Follows the runtime from `new` until the instance goes: group switches,
/// its state, and the groups' health every REFRESH while running. Needs a
/// tokio runtime; without one (no current handle) nothing is followed.
pub(super) fn spawn_watcher(inner: &Arc<Inner>) -> Option<JoinHandle<()>> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    let mut switches = inner.runtime.group_switches();
    let mut states = inner.runtime.states();
    let mut networks = inner.runtime.network_changes();
    let weak: Weak<Inner> = Arc::downgrade(inner);
    Some(handle.spawn(async move {
        let mut tick = tokio::time::interval(REFRESH);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut switches_open = true;
        loop {
            tokio::select! {
                switch = switches.recv(), if switches_open => match switch {
                    Some(switch) => {
                        let Some(inner) = weak.upgrade() else { return };
                        inner.on_switch(switch).await;
                    }
                    None => switches_open = false,
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
                _ = tick.tick() => {
                    let Some(inner) = weak.upgrade() else { return };
                    inner.refresh().await;
                }
            }
        }
    }))
}
