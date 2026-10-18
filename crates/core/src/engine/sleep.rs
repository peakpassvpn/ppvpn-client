//! The host's word on system sleep (docs/host-integration.md, 休眠与唤醒):
//! `sleep` and `wake` go to the running sail (embed `Instance::sleep` and
//! `wake`, libbox's `Pause` and `Wake`). From the first `sleep` of a run,
//! sail leaves its own detection of sleeps to the host, so a wake the host
//! misses (it quit or crashed meanwhile, or the system did not sleep after
//! all) would leave sail's timed checks waiting. A lease covers that: a
//! `sleep` not followed by a `wake` within [`LEASE`] of time awake wakes
//! sail by itself. Tokio's clock does not run while the system sleeps
//! (macOS, Linux), so the lease ends that long after the real wake; where
//! it does run, it ends at the wake.

use std::sync::Mutex;
use std::time::Duration;

use tokio::task::JoinHandle;

use super::Inner;

/// How long a `sleep` holds without a `wake`, in time awake.
pub(super) const LEASE: Duration = Duration::from_secs(60);

/// The lease of the last `sleep`, while it holds.
#[derive(Default)]
pub(super) struct SleepLease(Mutex<Option<JoinHandle<()>>>);

impl SleepLease {
    fn replace(&self, task: Option<JoinHandle<()>>) {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = std::mem::replace(&mut *slot, task) {
            old.abort();
        }
    }

    /// Whether a lease holds (tests).
    #[cfg(test)]
    pub(super) fn holds(&self) -> bool {
        let slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_ref().is_some_and(|task| !task.is_finished())
    }
}

impl Inner {
    pub(super) fn host_sleep(&self) {
        if !self.live().running {
            return;
        }
        if let Err(error) = self.runtime.sleep() {
            tracing::debug!(%error, "sleep not told");
            return;
        }
        tracing::info!("the system goes to sleep");
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = self.this.clone();
        self.sleep_lease.replace(Some(handle.spawn(async move {
            tokio::time::sleep(LEASE).await;
            let Some(inner) = weak.upgrade() else { return };
            tracing::warn!("no wake from the host since its sleep; waking");
            inner.tell_wake();
        })));
    }

    pub(super) fn host_wake(&self) {
        self.sleep_lease.replace(None);
        self.tell_wake();
    }

    fn tell_wake(&self) {
        // Not running, or no sleep told to this run: nothing to wake.
        match self.runtime.wake() {
            Ok(()) => tracing::info!("the system woke"),
            Err(error) => tracing::debug!(%error, "wake not told"),
        }
    }
}
