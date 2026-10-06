//! The Linux desktop TUN's routing guard (tunrules, Go 0.5.20): started
//! right after sail started the TUN, asked to check after a kernel switch
//! or a network change, and stopped before the TUN closes, so that sail's
//! own cleanup is never undone. Its verdicts are the Engine's
//! `TunRoutingSignal`s (`Status::tun_routing`, the state, the events).
//!
//! Elsewhere (macOS; Windows once sail reports it) sail itself tells when
//! another program changed the TUN's routes or address (its
//! `SystemChanged`). sail does not put them back: the routing is broken
//! (Degraded, not Fatal) until the next start; the host decides whether
//! to rebuild.

use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;

use super::state::TunRoutingSignal;
use super::Inner;
use crate::config::{Platform, Role};
use crate::runtime::SystemChange;
use crate::translate;
use crate::tunrules::{self, TunRoutingStatus};

/// The running guard and the task that follows its verdicts.
#[derive(Default)]
pub(super) struct RoutingGuard {
    run: Mutex<Option<(tunrules::Guard, JoinHandle<()>)>>,
    /// Tests: off unless a test turns it on (the fake runtime installs no
    /// routing, so a real guard reports `Unguarded`).
    #[cfg(test)]
    pub(super) enabled: std::sync::atomic::AtomicBool,
}

impl RoutingGuard {
    fn take(&self) -> Option<(tunrules::Guard, JoinHandle<()>)> {
        self.run.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// At the last handle's drop: stopped before the TUN's cleanup.
    pub(super) fn stop(&self) {
        if let Some((guard, follower)) = self.take() {
            follower.abort();
            guard.stop();
        }
    }
}

/// The signal a verdict is (`Ok`: none, nothing to report).
pub(super) fn signal(status: TunRoutingStatus) -> Option<TunRoutingSignal> {
    match status {
        TunRoutingStatus::Ok => None,
        TunRoutingStatus::Restoring { .. } => Some(TunRoutingSignal::Restoring),
        TunRoutingStatus::Restored { missing } => Some(TunRoutingSignal::Restored { missing }),
        TunRoutingStatus::Unguarded { .. } => Some(TunRoutingSignal::Unguarded),
        TunRoutingStatus::Broken { missing, error } => {
            Some(TunRoutingSignal::Broken { missing, error })
        }
    }
}

impl Inner {
    /// Only the Linux desktop TUN routes by policy rules.
    fn guards_routing(&self) -> bool {
        cfg!(target_os = "linux")
            && self.config.role == Role::Tun
            && self.config.platform == Platform::Linux
    }

    /// After sail started the TUN: snapshots what it installed and keeps it
    /// in place until [`guard_stopped`](Self::guard_stopped).
    pub(super) fn guard_started(self: &Arc<Self>) {
        if !self.guards_routing() {
            return;
        }
        #[cfg(test)]
        if !self
            .routing
            .enabled
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.guard_stopped();
        let scope = tunrules::Scope::desktop(translate::interface_name(self.config.platform));
        let _entered = handle.enter();
        let (guard, mut statuses) = tunrules::Guard::start(scope);
        let weak = Arc::downgrade(self);
        let follower = handle.spawn(async move {
            loop {
                let status = statuses.borrow_and_update().clone();
                if let Some(signal) = signal(status) {
                    let Some(inner) = weak.upgrade() else { return };
                    if !inner.live().running {
                        return;
                    }
                    inner.on_tun_routing(signal);
                }
                if statuses.changed().await.is_err() {
                    return;
                }
            }
        });
        *self.routing.run.lock().unwrap_or_else(|e| e.into_inner()) = Some((guard, follower));
    }

    /// Before the TUN closes (stop, a full restart, shutdown) or once the
    /// runtime failed: on return the routing is no longer touched.
    pub(super) fn guard_stopped(&self) {
        self.routing.stop();
    }

    /// After a stop that failed: the TUN still runs (unless the runtime
    /// panicked), so it is guarded again, from a new snapshot.
    pub(super) fn guard_restarted(self: &Arc<Self>) {
        if self.live().running {
            self.guard_started();
        }
    }

    /// Asks the guard for a check now (`reason` is logged with it).
    pub(super) fn guard_check(&self, reason: &str) {
        if let Some((guard, _)) = self
            .routing
            .run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            guard.check(reason);
        }
    }

    /// Another program changed what sail set up for the TUN, while
    /// running: `TunRoutingBroken`, `tun_routing` broken and Degraded
    /// until the next start. Not Fatal: sail puts nothing back here, and
    /// another VPN changing a route would otherwise end the instance while
    /// traffic goes around the TUN all the same. Whether and how often to
    /// rebuild is the host's (host-integration 4.4).
    pub(super) fn on_system_change(&self, change: SystemChange) {
        if !self.live().running {
            return;
        }
        tracing::warn!(
            kind = %change.kind,
            resource = %change.resource,
            "tun routing changed by another program"
        );
        self.on_tun_routing(TunRoutingSignal::Changed {
            missing: vec![change.resource],
            error: format!("{} changed by another program", change.kind),
        });
    }

    /// Whether a guard runs (tests).
    #[cfg(test)]
    pub(super) fn guard_running(&self) -> bool {
        self.routing
            .run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
