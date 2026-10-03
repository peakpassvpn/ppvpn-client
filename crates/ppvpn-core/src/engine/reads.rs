//! The runtime's figures (groups and their health, traffic, connections)
//! are read while the host reads them, not on a timer of their own: an
//! idle instance wakes for nothing (#45, idle power). The first host read
//! starts a refresher that reads the runtime every [`READ_REFRESH`] while
//! the host keeps reading, and ends [`READ_IDLE`] after its last read; a
//! read returns what was last read (`Traffic::measured_at` says when).
//! Group switches and failed dials refresh at once on their own (events).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::time::Instant;

use super::Inner;

/// How often the runtime is read while the host reads.
pub(super) const READ_REFRESH: Duration = Duration::from_secs(1);
/// How long after the host's last read the refresher goes on.
pub(super) const READ_IDLE: Duration = Duration::from_secs(10);

/// The host's reads and the refresher they keep going.
pub(super) struct Reads {
    /// The runtime the refresher runs on (the one the instance was made
    /// on): a host may read from a thread of its own.
    handle: Option<Handle>,
    last: Mutex<Option<Instant>>,
    active: AtomicBool,
}

impl Reads {
    pub(super) fn new() -> Self {
        Reads {
            handle: Handle::try_current().ok(),
            last: Mutex::default(),
            active: AtomicBool::new(false),
        }
    }

    fn last(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.last.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn idle(&self) -> bool {
        self.last().is_none_or(|at| at.elapsed() >= READ_IDLE)
    }
}

impl Inner {
    /// A host read (`status`, `traffic`, `connections`): keeps the
    /// runtime's figures fresh while the host goes on reading.
    pub(super) fn host_read(self: &Arc<Self>) {
        *self.reads.last() = Some(Instant::now());
        if !self.live().running || self.reads.active.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(handle) = &self.reads.handle else {
            self.reads.active.store(false, Ordering::SeqCst);
            return;
        };
        let weak: Weak<Inner> = Arc::downgrade(self);
        handle.spawn(async move {
            loop {
                let Some(inner) = weak.upgrade() else { return };
                inner.refresh().await;
                if inner.reads.idle() || !inner.live().running {
                    inner.reads.active.store(false, Ordering::SeqCst);
                    return;
                }
                drop(inner);
                tokio::time::sleep(READ_REFRESH).await;
            }
        });
    }
}

#[cfg(test)]
#[path = "reads_tests.rs"]
mod tests;
