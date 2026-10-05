//! A pinned node's checks (docs/backend-profile.md, 故障转移语义: checks go
//! on while pinned, to report whether the ingresses are up). The pin selects
//! the ingress in the node's selector, so the node's fallback group is no
//! longer used and sail's lazy tests pause; while a pin lasts, the Engine
//! has the group test its members at the group's own interval instead, so
//! the group keeps a current choice for the unpin and the members' state
//! stays known. The timer exists only while running with a multi-ingress
//! node pinned: an unpin, a stop, a fatal or the shutdown cancels it.

use std::sync::Mutex;
use std::time::Duration;

use tokio::task::JoinHandle;

use super::state::Applied;
use super::Inner;

/// Within which one round (every member, both URLs) is done.
const CHECK_WITHIN: Duration = Duration::from_secs(10);

/// The timer and the run it belongs to.
#[derive(Default)]
pub(super) struct PinnedChecks(Mutex<Option<(u64, JoinHandle<()>)>>);

impl PinnedChecks {
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<(u64, JoinHandle<()>)>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a timer runs (tests).
    #[cfg(test)]
    pub(super) fn running(&self) -> bool {
        self.slot().as_ref().is_some_and(|(_, t)| !t.is_finished())
    }
}

impl Inner {
    /// After a start, a restart, an apply or a pin: a timer while running
    /// with a pinned group, none otherwise.
    pub(super) fn sync_pinned_checks(&self) {
        let (want, run, interval) = {
            let live = self.live();
            let applied = live.applied.as_ref();
            let pinned = applied.is_some_and(|a| !pinned_groups(a).is_empty());
            let interval = applied.map(|a| a.translation.check_interval);
            (live.running && pinned, live.run, interval)
        };
        let mut slot = self.pinned.slot();
        let (Some(interval), true) = (interval.filter(|i| !i.is_zero()), want) else {
            if let Some((_, task)) = slot.take() {
                task.abort();
            }
            return;
        };
        if slot
            .as_ref()
            .is_some_and(|(r, task)| *r == run && !task.is_finished())
        {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = self.this.clone();
        let task = handle.spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = weak.upgrade() else { return };
                let groups = {
                    let live = inner.live();
                    if !live.running || live.run != run {
                        return;
                    }
                    live.applied.as_ref().map(pinned_groups).unwrap_or_default()
                };
                if groups.is_empty() {
                    return;
                }
                for group in groups {
                    if let Err(e) = inner.runtime.check_group(&group, CHECK_WITHIN).await {
                        tracing::debug!(group = %group, error = %e, "pinned node check failed");
                    }
                }
            }
        });
        if let Some((_, previous)) = slot.replace((run, task)) {
            previous.abort();
        }
    }

    /// At a stop, a fatal or the shutdown.
    pub(super) fn stop_pinned_checks(&self) {
        if let Some((_, task)) = self.pinned.slot().take() {
            task.abort();
        }
    }
}

/// The fallback groups of the pinned multi-ingress nodes (a single-ingress
/// node has none).
fn pinned_groups(applied: &Applied) -> Vec<String> {
    let t = &applied.translation;
    applied
        .pins
        .keys()
        .filter_map(|node| t.groups.get(t.node_tags.get(node)?))
        .cloned()
        .collect()
}
