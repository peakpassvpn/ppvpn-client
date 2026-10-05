//! Taking a new translation while running (#150). sail's reload takes it
//! in place: it diffs the inbounds by tag (adds, removes, replaces or
//! keeps each) and leaves the other connections where they are. What only
//! a stop and a start can take, a TUN's change, is a full restart
//! (`SwitchKind::FullRestart`), as is a reload sail refuses with
//! `needs_restart` or ends with `inbound_lost`. The reasons keep the words
//! of Go's `fullRestartReasons`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::Value;

use super::{Error, Inner};
use crate::request::{ListenerChange, ListenerChangeKind, SwitchKind};
use crate::runtime::inbound_tag;
use crate::translate::{
    Translation, LOCAL_PROXY_INBOUND_TAG, SYSTEM_PROXY_INBOUND_TAG, TUN_INBOUND_TAG,
};

/// The route options the listeners use (Go: `frontOptions`).
const INTERFACE_OPTIONS: [&str; 4] = [
    "auto_detect_interface",
    "override_android_vpn",
    "default_interface",
    "default_mark",
];

/// Why `next` cannot be taken by a reload of `running` (empty: it can).
/// The system proxy listener is left out: it is toggled in place.
pub(super) fn full_restart_reasons(running: &str, next: &str) -> Vec<String> {
    let parse = |config: &str| serde_json::from_str::<Value>(config).unwrap_or_default();
    let (running, next) = (parse(running), parse(next));
    let (before, after) = (inbounds(&running), inbounds(&next));
    let tags: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let mut reasons = Vec::new();
    for tag in tags {
        if tag == SYSTEM_PROXY_INBOUND_TAG {
            continue;
        }
        let (Some(old), Some(new)) = (before.get(tag), after.get(tag)) else {
            reasons.push(format!("inbound {tag} added or removed"));
            continue;
        };
        if tag == TUN_INBOUND_TAG {
            if old != new {
                reasons.push("tun options changed".into());
            }
        } else if tag == LOCAL_PROXY_INBOUND_TAG {
            if without_users(old) != without_users(new) {
                reasons.push("local proxy listener changed".into());
            }
        } else if old != new {
            reasons.push(format!("inbound {tag} changed"));
        }
    }
    if interface_options(&running) != interface_options(&next) {
        reasons.push("interface options changed".into());
    }
    reasons
}

/// What only a stop and a start can take: the TUN's changes (sail sets
/// up a TUN only at a start). sail's reload takes every other listener
/// change in place, by tag, and the route's interface options too; its
/// `needs_restart` is the final word, this saves the try where the answer
/// is known.
pub(super) fn restart_reasons(running: &str, next: &str) -> Vec<String> {
    full_restart_reasons(running, next)
        .into_iter()
        .filter(|r| r == "tun options changed" || r == "inbound tun added or removed")
        .collect()
}

/// The listeners a reload changed, from its report.
fn listener_changes(report: &crate::runtime::ReloadReport) -> Vec<ListenerChange> {
    report
        .inbounds
        .iter()
        .filter_map(|(tag, change)| {
            let change = match change.as_str() {
                "added" => ListenerChangeKind::Added,
                "removed" => ListenerChangeKind::Removed,
                "replaced" => ListenerChangeKind::Replaced,
                _ => return None,
            };
            Some(ListenerChange {
                tag: tag.clone(),
                change,
            })
        })
        .collect()
}

fn inbounds(config: &Value) -> BTreeMap<String, &Value> {
    config["inbounds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|inbound| Some((inbound_tag(inbound)?, inbound)))
        .collect()
}

fn without_users(inbound: &Value) -> Value {
    let mut inbound = inbound.clone();
    if let Some(object) = inbound.as_object_mut() {
        object.remove("users");
    }
    inbound
}

fn interface_options(config: &Value) -> Vec<&Value> {
    INTERFACE_OPTIONS
        .iter()
        .map(|key| &config["route"][key])
        .collect()
}

/// How the running configuration was replaced, and with what.
pub(super) struct Switched {
    pub kind: SwitchKind,
    /// The configuration now running.
    pub translation: Translation,
    /// The listeners a hot switch changed (none for a full restart).
    pub listeners: Vec<ListenerChange>,
    /// A hot switch's connections: closed (their node is gone) and kept.
    /// A full restart closes them all and counts none.
    pub closed: u32,
    pub kept: u32,
}

/// The connections a switch from `running` to `next` closes (Go's
/// `closeOnSwitch`): those through a node `next` no longer has. A
/// connection's node is the one its chain recorded when it was routed:
/// node and ingress tags are made from the node's id (translate's
/// `node_tag`), so a tag names the same node in every build. A connection
/// through no node (direct, a rejected one) and one on a node that stays,
/// whatever its ingress, rule or selection now, is kept: sail's reload
/// leaves it on the outbound it was routed through. A removed local proxy
/// user's connections are among these (its node went); sail itself closes
/// those of a listener or user it removes.
pub(super) fn close_on_switch(
    running: &Translation,
    next: &Translation,
    connections: &[crate::runtime::RuntimeConnection],
) -> Vec<u64> {
    let remaining: BTreeSet<&String> = next.outbound_nodes.values().collect();
    connections
        .iter()
        .filter(|c| {
            c.chain
                .iter()
                .find_map(|tag| {
                    next.outbound_nodes
                        .get(tag)
                        .or_else(|| running.outbound_nodes.get(tag))
                })
                .is_some_and(|node| !remaining.contains(node))
        })
        .map(|c| c.id)
        .collect()
}

/// Builds the configuration again from the current inputs (the listeners'
/// ports and whether the local proxy is left out may have changed).
pub(super) type Build<'a> = &'a (dyn Fn() -> Result<Translation, Error> + Sync);

/// Whether `translation` carries the shared local proxy listener.
fn has_local_proxy(translation: &Translation) -> bool {
    serde_json::from_str::<Value>(&translation.json)
        .ok()
        .is_some_and(|config| inbounds(&config).contains_key(LOCAL_PROXY_INBOUND_TAG))
}

impl Inner {
    /// Switches the running runtime from `running` to `next`: a reload when
    /// it can take it, else a stop and a start, which builds again with
    /// `build` once the old listeners are closed (as `start` does). A start
    /// that fails puts `running` back; if that fails too, the instance is
    /// stopped. Returns the configuration now running. Called under the
    /// operation lock.
    pub(super) async fn switch_to(
        self: &Arc<Self>,
        running: &Translation,
        next: Translation,
        build: Build<'_>,
    ) -> Result<Switched, Error> {
        let mut reasons = restart_reasons(&running.json, &next.json);
        if reasons.is_empty() {
            // Before the reload: sail closes a removed user's connections
            // itself, and they still count as closed.
            let connections = self.runtime.connections().await.unwrap_or_default();
            match self.runtime.reload(&next.json).await {
                Ok(report) => {
                    self.guard_check("kernel switch");
                    let doomed = close_on_switch(running, &next, &connections);
                    for id in &doomed {
                        if let Err(e) = self.runtime.close_connection(*id).await {
                            tracing::warn!(id, error = %e, "cannot close a connection of a removed node");
                        }
                    }
                    // Kept: those still open, less the closed. sail has
                    // already closed a removed or replaced listener's own.
                    let kept = self
                        .runtime
                        .connections()
                        .await
                        .unwrap_or_default()
                        .iter()
                        .filter(|c| !doomed.contains(&c.id))
                        .count();
                    let closed = u32::try_from(doomed.len()).unwrap_or(u32::MAX);
                    let kept = u32::try_from(kept).unwrap_or(u32::MAX);
                    return Ok(Switched {
                        kind: SwitchKind::KernelSwitch,
                        translation: next,
                        listeners: listener_changes(&report),
                        closed,
                        kept,
                    });
                }
                // sail has the final word: what it cannot take in place is
                // a restart, and a listener that could not move (it
                // listens no more) is one too.
                Err(e) if e.code == "needs_restart" || e.code == "inbound_lost" => {
                    tracing::warn!(error = %e, "the reload needs a restart");
                    reasons.push(e.code.clone());
                }
                Err(e) => return Err(self.runtime_error(&e)),
            }
        }
        tracing::info!(reasons = reasons.join("; "), "full restart");
        self.live().restarting = true;
        let result = self.restart(running, build).await;
        self.live().restarting = false;
        result.map(|translation| Switched {
            kind: SwitchKind::FullRestart { reasons },
            translation,
            listeners: Vec::new(),
            closed: 0,
            kept: 0,
        })
    }

    async fn restart(
        self: &Arc<Self>,
        running: &Translation,
        build: Build<'_>,
    ) -> Result<Translation, Error> {
        // Before the TUN closes: sail's cleanup must not be undone.
        self.guard_stopped();
        self.cancel_dns_queries();
        if let Err(e) = self.runtime.stop().await {
            let error = self.runtime_error(&e);
            // Still running: the TUN stays, and so does its guard.
            self.guard_restarted();
            return Err(error);
        }
        // The old run's timers go with it; a re-probe that was waiting is
        // armed again in the new run.
        let reprobe = self.network_stopped();
        self.local_proxy_stopped();
        let started = self.start_next(build).await;
        let (translation, error) = match started {
            Ok(translation) => (translation, None),
            Err(error) => {
                if !self.live().running {
                    // Panicked: Fatal, nothing to put back.
                    return Err(error);
                }
                tracing::warn!(error = %error, "full restart failed, putting the running configuration back");
                self.set_local_proxy_left_out(!has_local_proxy(running));
                match self.runtime.start(&running.json).await {
                    Ok(()) => (running.clone(), Some(error)),
                    Err(e) => {
                        let back = self.runtime_error(&e);
                        tracing::error!(error = %back, "cannot put the running configuration back");
                        self.restart_failed();
                        return Err(error);
                    }
                }
            }
        };
        self.kernel_started();
        let retry = {
            // A new run of sail: what was read of the old one goes. A local
            // proxy listener still left out is retried in the new run.
            let mut live = self.live();
            live.run += 1;
            live.clear_runtime();
            live.local_proxy_unavailable = self.local_proxy_left_out();
            self.settle(&mut live);
            live.local_proxy_unavailable.then_some(live.run)
        };
        self.network_started();
        if reprobe {
            self.arm_reprobe();
        }
        // What sail installed for the new TUN.
        self.guard_started();
        self.sync_pinned_checks();
        if let Some(run) = retry {
            self.retry_local_proxy(run);
        }
        match error {
            None => Ok(translation),
            Some(error) => Err(error),
        }
    }

    /// The start half of a restart, as `start`: the listeners' ports are
    /// checked again now that the old ones are closed, and a start that
    /// fails with the shared local proxy listener goes on without it.
    async fn start_next(&self, build: Build<'_>) -> Result<Translation, Error> {
        self.prepare_listeners()?;
        let mut translation = build()?;
        let mut started = self.runtime.start(&translation.json).await;
        if let Err(e) = &started {
            if e.code != "panicked" && self.leave_out_local_proxy() {
                tracing::warn!(error = %e, "start failed with the local proxy listener, starting without it");
                translation = build()?;
                started = self.runtime.start(&translation.json).await;
            }
        }
        match started {
            Ok(()) => Ok(translation),
            Err(e) => Err(self.runtime_error(&e)),
        }
    }

    /// A kernel of its own started (start, restart): the next switch's
    /// `previous`.
    pub(super) fn kernel_started(&self) {
        self.kernel_gen.fetch_add(1, Ordering::SeqCst);
    }

    /// A reload switched kernels for `revision` (Go's `kernel switched`
    /// line and `KernelSwitched`), closing `closed` connections and keeping
    /// `kept` ([`close_on_switch`]). No kernel drains: sail reloads in
    /// place, so `draining_kernels` is 0.
    pub(super) fn kernel_switched(&self, revision: &str, closed: u32, kept: u32) {
        let gen = self.kernel_gen.fetch_add(1, Ordering::SeqCst) + 1;
        tracing::info!(
            gen,
            previous = gen - 1,
            closed_connections = closed,
            kept_connections = kept,
            draining_kernels = 0u32,
            "kernel switched"
        );
        self.publish(crate::event::Event::KernelSwitched {
            at: super::now(),
            revision: revision.into(),
            closed_connections: closed,
            kept_connections: kept,
            draining_kernels: 0,
        });
    }

    /// Neither configuration starts again: the instance is stopped.
    fn restart_failed(&self) {
        let mut live = self.live();
        if !live.running {
            // Already marked (a panic is Fatal).
            return;
        }
        live.running = false;
        live.run += 1;
        live.clear_runtime();
        self.settle(&mut live);
        self.publish(crate::event::Event::CoreStopped { at: super::now() });
    }
}

#[cfg(test)]
#[path = "switch_tests.rs"]
mod tests;
