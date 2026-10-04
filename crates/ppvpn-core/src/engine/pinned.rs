//! A pinned node's checks (docs/backend-profile.md, 故障转移语义: checks go
//! on while pinned, to report whether the ingresses are up). The pin selects
//! the ingress in the node's selector, so the node's fallback group is no
//! longer used and sail's lazy tests pause; while the pin lasts, the Engine
//! has the group test its members each interval instead, so the group keeps
//! a current choice for the unpin and the members' state stays known.

use std::sync::Arc;
use std::time::Duration;

use super::state::Applied;
use super::Inner;
use crate::translate::CHECK_INTERVAL;

/// Within which one round (every member, both URLs) is done.
const CHECK_WITHIN: Duration = Duration::from_secs(10);

impl Inner {
    /// At a start or a restart: checks the pinned nodes' groups each
    /// interval while this run lasts. A pin made or removed later is
    /// followed at the next round.
    pub(super) fn check_pinned(self: &Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let run = self.live().run;
        let weak = Arc::downgrade(self);
        handle.spawn(async move {
            loop {
                tokio::time::sleep(CHECK_INTERVAL).await;
                let Some(inner) = weak.upgrade() else { return };
                let groups = {
                    let live = inner.live();
                    if !live.running || live.run != run {
                        return;
                    }
                    live.applied.as_ref().map(pinned_groups).unwrap_or_default()
                };
                for group in groups {
                    if let Err(e) = inner.runtime.check_group(&group, CHECK_WITHIN).await {
                        tracing::debug!(group = %group, error = %e, "pinned node check failed");
                    }
                }
            }
        });
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
