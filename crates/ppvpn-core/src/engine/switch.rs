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

impl Inner {
    /// Switches the running runtime from `running` to `next`: a reload when
    /// it can take it, else a stop and a start. A start that fails puts
    /// `running` back; if that fails too, the instance is stopped. Called
    /// under the operation lock.
    pub(super) async fn switch_to(
        self: &Arc<Self>,
        running: &Translation,
        next: &Translation,
    ) -> Result<SwitchKind, Error> {
        let reasons = full_restart_reasons(&running.json, &next.json);
        if reasons.is_empty() {
            if let Err(e) = self.runtime.reload(&next.json).await {
                return Err(self.runtime_error(&e));
            }
            self.guard_check("kernel switch");
            return Ok(SwitchKind::KernelSwitch);
        }
        tracing::info!(reasons = reasons.join("; "), "full restart");
        self.live().restarting = true;
        let result = self.restart(running, next).await;
        self.live().restarting = false;
        result.map(|()| SwitchKind::FullRestart { reasons })
    }

    async fn restart(
        self: &Arc<Self>,
        running: &Translation,
        next: &Translation,
    ) -> Result<(), Error> {
        // Before the TUN closes: sail's cleanup must not be undone.
        self.guard_stopped();
        if let Err(e) = self.runtime.stop().await {
            let error = self.runtime_error(&e);
            // Still running: the TUN stays, and so does its guard.
            self.guard_restarted();
            return Err(error);
        }
        self.network_stopped();
        let started = self.runtime.start(&next.json).await;
        let error = match started {
            Ok(()) => None,
            Err(e) => {
                let error = self.runtime_error(&e);
                if !self.live().running {
                    // Panicked: Fatal, nothing to put back.
                    return Err(error);
                }
                tracing::warn!(error = %error, "full restart failed, putting the running configuration back");
                Some(match self.runtime.start(&running.json).await {
                    Ok(()) => error,
                    Err(e) => {
                        let back = self.runtime_error(&e);
                        tracing::error!(error = %back, "cannot put the running configuration back");
                        self.restart_failed();
                        return Err(error);
                    }
                })
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
        // What sail installed for the new TUN.
        self.guard_started();
        if let Some(run) = retry {
            self.retry_local_proxy(run);
        }
        match error {
            None => Ok(()),
            Some(error) => Err(error),
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
