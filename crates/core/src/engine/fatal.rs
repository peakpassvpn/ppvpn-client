//! A runtime that failed or panicked leaves the instance Fatal (#208). What
//! it had opened (the TUN, its routes and rules, the system's DNS, on
//! Windows the strict route's filters) would outlive it with the host, the
//! user offline: the runtime is stopped at once, bounded, and what that
//! stop could not undo is kept for the host's `ShutdownReport`.
//! `TunRoutingBroken` is not this: the runtime still runs (#211).

use std::sync::Mutex;

use tokio::task::JoinHandle;

use super::{cleanup, Inner};
use crate::types::Leftover;

/// The cleanup at Fatal: the task while it runs, then what it left.
#[derive(Default)]
pub(super) struct FatalCleanup {
    task: Mutex<Option<JoinHandle<()>>>,
    leftovers: Mutex<Vec<Leftover>>,
}

impl Inner {
    /// The runtime failed or panicked: stop it now, within the shutdown's
    /// limit, off the caller.
    pub(super) fn fatal_cleanup(&self) {
        let Some(inner) = self.this.upgrade() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.cancel_dns_queries();
        let task = handle.spawn(async move {
            let until = std::time::Instant::now() + cleanup::SHUTDOWN_LIMIT;
            let left = cleanup::stop_runtime(inner.runtime.as_ref(), until).await;
            for leftover in &left {
                tracing::warn!(kind = ?leftover.kind, name = %leftover.name,
                    "not cleaned up after the runtime failed");
            }
            inner.live().needs_stop = false;
            *inner
                .fatal
                .leftovers
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = left;
        });
        *self.fatal.task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
    }

    /// At shutdown: waits (until `until`) for a cleanup at Fatal still under
    /// way, and takes what it left.
    pub(super) async fn fatal_leftovers(&self, until: tokio::time::Instant) -> Vec<Leftover> {
        let task = self
            .fatal
            .task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(task) = task {
            let _ = tokio::time::timeout_at(until, task).await;
        }
        self.fatal_leftovers_now()
    }

    /// What the cleanup at Fatal left so far (the last handle's drop does
    /// not wait).
    pub(super) fn fatal_leftovers_now(&self) -> Vec<Leftover> {
        std::mem::take(
            &mut *self
                .fatal
                .leftovers
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        )
    }
}
