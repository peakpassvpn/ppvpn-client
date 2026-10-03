//! Taking a new translation while running (#150). sail's reload keeps
//! every listener as it is: it neither adds nor removes an inbound, and of
//! an existing one it replaces only the users. A change it cannot take is
//! a stop and a start (`SwitchKind::FullRestart`), as Go's
//! `fullRestartReasons`, whose whitelist and words this keeps.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::Value;

use super::{Error, Inner};
use crate::request::SwitchKind;
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
    ) -> Result<(SwitchKind, Translation), Error> {
        let reasons = full_restart_reasons(&running.json, &next.json);
        if reasons.is_empty() {
            if let Err(e) = self.runtime.reload(&next.json).await {
                return Err(self.runtime_error(&e));
            }
            self.guard_check("kernel switch");
            return Ok((SwitchKind::KernelSwitch, next));
        }
        tracing::info!(reasons = reasons.join("; "), "full restart");
        self.live().restarting = true;
        let result = self.restart(running, build).await;
        self.live().restarting = false;
        result.map(|translation| (SwitchKind::FullRestart { reasons }, translation))
    }

    async fn restart(
        self: &Arc<Self>,
        running: &Translation,
        build: Build<'_>,
    ) -> Result<Translation, Error> {
        // Before the TUN closes: sail's cleanup must not be undone.
        self.guard_stopped();
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
