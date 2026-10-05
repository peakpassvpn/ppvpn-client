//! The profile's rule sets in the Engine (contract 4.1, Go: the core's
//! rulesets wiring): an apply prepares them (downloads within
//! `rulesets::PREPARE_TIMEOUT`, never failing for them), the translation
//! names their local copies, and the refresh task reports each state change
//! (`RuleSetChanged`, `Degraded{RuleSetUnavailable}` while running) and asks
//! for a rebuild when the sets the configuration can use change.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Notify;

use super::{now, Error, Inner};
use crate::config::{EngineConfig, Role};
use crate::error::codes;
use crate::event::Event;
use crate::profile::{Profile, RuleSet};
use crate::request::RoutingMode;
use crate::rulesets::{self, Dial, DirectDial, Manager, Snapshot, State, Stream};
use crate::runtime::Target;
use crate::status::RuleSetStatus;
use crate::translate::{self, RuleSetFile, DIRECT_TAG};

/// Bounds a download's connection through the runtime (as a plain one).
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// The start of `shutdown`, for the waits that must give way to it.
#[derive(Default)]
pub(super) struct Closing {
    closed: AtomicBool,
    notify: Notify,
}

impl Closing {
    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// What the configuration uses of the rule sets: the local copies of the
/// last snapshot made current, and the hosts their URLs are pinned to.
#[derive(Default)]
pub(super) struct RuleSetInputs {
    files: HashMap<String, RuleSetFile>,
    allowed_hosts: Vec<String>,
}

/// The manager of an instance: its cache in `<state_dir>/rule-sets`, its
/// downloads direct, its reports to `inner`.
pub(super) fn manager(config: &EngineConfig, inner: Weak<Inner>) -> Manager {
    let on_state = inner.clone();
    let on_rebuild = inner.clone();
    Manager::new(rulesets::Options {
        dir: Some(rule_set_dir(config)),
        dial: Arc::new(EngineDial { inner }),
        on_state: Some(Arc::new(move |status: rulesets::Status| {
            if let Some(inner) = on_state.upgrade() {
                inner.on_rule_set(status);
            }
        })),
        on_rebuild: Some(Arc::new(move || {
            let Some(inner) = on_rebuild.upgrade() else {
                return;
            };
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move { inner.rebuild_rule_sets().await });
            }
        })),
        ..rulesets::Options::default()
    })
}

fn rule_set_dir(config: &EngineConfig) -> std::path::PathBuf {
    config.state_dir.join("rule-sets")
}

/// A download's connection. While a TUN instance runs it goes through the
/// running configuration's direct outbound, which is bound to the physical
/// interface, so it can neither enter the tunnel nor reach a node.
/// Otherwise no tunnel of this instance exists and a plain socket is
/// direct (Go: `dialRuleSet`).
struct EngineDial {
    inner: Weak<Inner>,
}

#[async_trait]
impl Dial for EngineDial {
    async fn dial(&self, host: &str, port: u16) -> io::Result<Box<dyn Stream>> {
        let runtime = self
            .inner
            .upgrade()
            .filter(|inner| inner.config.role == Role::Tun && inner.live().running)
            .map(|inner| inner.runtime.clone());
        let Some(runtime) = runtime else {
            return DirectDial.dial(host, port).await;
        };
        let to = match host.parse() {
            Ok(ip) => Target::Addr(std::net::SocketAddr::new(ip, port)),
            Err(_) => Target::Domain(host.to_owned(), port),
        };
        let stream = runtime
            .dial_tcp(DIRECT_TAG, to, DIAL_TIMEOUT)
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Box::new(stream))
    }
}

/// The sets the configuration of `profile` in `mode` names.
pub(super) fn sets_of(profile: &Profile, mode: RoutingMode) -> Vec<RuleSet> {
    translate::effective_routing(profile, mode).rule_sets
}

/// A rebuild of an expired profile fails (contract 4.1): the configuration
/// in use stays, and the host learns it from `ReloadFailed`.
pub(super) fn expired(profile: &Profile) -> Option<Error> {
    let expires = profile.expires_at?;
    (Utc::now() >= expires).then(|| {
        Error::new(
            codes::PROFILE_EXPIRED,
            false,
            "the applied profile has expired",
        )
    })
}

impl Inner {
    /// A rebuild the Engine started itself failed: logged, `ReloadFailed`.
    pub(super) fn rebuild_failed(&self, error: Error, message: &str) {
        tracing::warn!(error = %error, "{message}");
        self.publish(Event::ReloadFailed {
            at: now(),
            code: error.code.into(),
            message: message.into(),
        });
    }

    fn rule_set_inputs(&self) -> std::sync::MutexGuard<'_, RuleSetInputs> {
        self.rule_set_inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// `Options.rule_sets`: the local copies in use.
    pub(super) fn rule_set_files(&self) -> HashMap<String, RuleSetFile> {
        self.rule_set_inputs().files.clone()
    }

    /// The hosts of the snapshot in use (`ApplyRequest::allowed_rule_set_hosts`).
    pub(super) fn rule_set_hosts(&self) -> Vec<String> {
        self.rule_set_inputs().allowed_hosts.clone()
    }

    /// The snapshot an apply translates with (downloads included); None
    /// when `shutdown` began meanwhile (the downloads are dropped).
    pub(super) async fn prepare_rule_sets(
        &self,
        profile: &Profile,
        mode: RoutingMode,
        allowed_hosts: &[String],
    ) -> Option<Snapshot> {
        let closed = self.closing.notify.notified();
        if self.closing.is_closed() {
            return None;
        }
        let sets = sets_of(profile, mode);
        if !sets.is_empty() {
            // The cache's directory, made on first use; a failure shows as
            // the sets' storage error.
            let dir = rule_set_dir(&self.config);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!(error = %e, "rule set cache directory");
            }
        }
        let prepare = self
            .rule_sets
            .prepare(&sets, allowed_hosts, Some(rulesets::PREPARE_TIMEOUT));
        tokio::select! {
            snapshot = prepare => Some(snapshot),
            () = closed => None,
        }
    }

    /// After the translation made from `snapshot` took effect: it is what
    /// the next ones use, and its sets are refreshed from now on.
    pub(super) fn activate_rule_sets(&self, snapshot: Snapshot, allowed_hosts: Vec<String>) {
        *self.rule_set_inputs() = RuleSetInputs {
            files: snapshot.files(),
            allowed_hosts,
        };
        self.rule_sets.activate(snapshot);
        self.settle_rule_sets();
    }

    /// `status.rule_sets`.
    pub(super) fn rule_set_statuses(&self) -> Vec<RuleSetStatus> {
        self.rule_sets
            .statuses()
            .iter()
            .map(rulesets::Status::public)
            .collect()
    }

    /// One set's state changed: logged, `RuleSetChanged`, and the state.
    fn on_rule_set(&self, status: rulesets::Status) {
        tracing::info!(
            id = %status.id,
            state = status.state.as_str(),
            error = status.error,
            failures = status.failures,
            next_retry_at = ?status.next_retry_at,
            "rule set"
        );
        self.publish(status.event(now()));
        self.settle_rule_sets();
    }

    /// `Degraded{RuleSetUnavailable}` for each set without a copy.
    fn settle_rule_sets(&self) {
        let unavailable: Vec<String> = self
            .rule_sets
            .statuses()
            .into_iter()
            .filter(|s| s.state == State::Unavailable)
            .map(|s| s.id)
            .collect();
        let mut live = self.live();
        if live.rule_sets_unavailable != unavailable {
            live.rule_sets_unavailable = unavailable;
            self.settle(&mut live);
        }
    }

    /// The refresh task changed which sets can be used: the configuration
    /// is built again from the cached copies (no downloads), as an apply of
    /// the same profile (Go: `rebuildForRuleSets`).
    async fn rebuild_rule_sets(self: Arc<Self>) {
        let _op = self.op.lock().await;
        let (profile, mode, selected, pins, running) = {
            let live = self.live();
            if live.shut_down {
                return;
            }
            let Some(a) = &live.applied else {
                return;
            };
            (
                a.profile.clone(),
                a.mode,
                a.selected.clone(),
                a.pins.clone(),
                live.running.then(|| a.translation.clone()),
            )
        };
        if let Some(error) = expired(&profile) {
            self.rebuild_failed(error, "rule set rebuild failed");
            return;
        }
        let allowed_hosts = self.rule_set_inputs().allowed_hosts.clone();
        let snapshot = self
            .rule_sets
            .prepare(&sets_of(&profile, mode), &allowed_hosts, None)
            .await;
        let files = snapshot.files();
        let build = || {
            let mut options = self.options(mode, &selected, &pins);
            options.rule_sets = files.clone();
            translate::translate(&profile, &options)
        };
        let result = match build() {
            Ok(translation) => match &running {
                Some(running) => {
                    self.switch_to(running, translation, &build)
                        .await
                        .map(|switched| {
                            if switched.kind == crate::request::SwitchKind::KernelSwitch {
                                self.kernel_switched(
                                    &profile.revision,
                                    switched.closed,
                                    switched.kept,
                                );
                            }
                            switched.translation
                        })
                }
                None => Ok(translation),
            },
            Err(e) => Err(e),
        };
        let translation = match result {
            Ok(translation) => translation,
            Err(error) => {
                self.rebuild_failed(error, "rule set rebuild failed");
                return;
            }
        };
        if let Some(applied) = self.live().applied.as_mut() {
            applied.translation = translation;
        }
        self.activate_rule_sets(snapshot, allowed_hosts);
        if running.is_some() {
            self.refresh().await;
        }
    }
}

#[cfg(test)]
#[path = "rule_sets_tests.rs"]
mod tests;
