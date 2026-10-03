//! The network as sail sees it (one monitor, #45): NetworkChanged, the
//! offline state (Degraded{NoDefaultInterface}, probes failing fast with
//! NO_DEFAULT_INTERFACE) and, on a TUN instance, the host IPv6 re-probe
//! REPROBE_DELAY after the last change of a burst.
//!
//! Transitional (rust-parity, "过渡实现"): the source is sail's network
//! watch (`Runtime::network_changes`), which keeps the latest change only.
//! A reader that falls behind sees a jump in `generation`; when the change's
//! `old` is not the network last seen, the step in between is replayed
//! from it, so offline-and-back still shows as both. Shorter blips inside
//! one jump are lost until sail::embed's bounded `Event::Network` lands.

use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use tokio::task::JoinHandle;

use super::Inner;
use crate::config::Role;
use crate::runtime::{NetworkChange, NetworkSnapshot};

/// How long the network must stay unchanged before the host's IPv6 path is
/// probed again (Go: ReprobeDelay): a burst of changes probes once.
pub(super) const REPROBE_DELAY: Duration = Duration::from_secs(2);

/// After a start whose snapshot knows no network yet: how often, and how
/// many times, the snapshot is read again until it does (sail reports no
/// change for the first default interface; its detection takes at most
/// 1 s). Past that the network counts as offline.
const FIRST_NETWORK_POLL: Duration = Duration::from_millis(100);
const FIRST_NETWORK_POLLS: u32 = 20;

/// What the Engine follows of sail's network.
#[derive(Default)]
pub(super) struct NetworkState {
    track: Mutex<Track>,
}

#[derive(Default)]
struct Track {
    /// The last change's generation in this run (0 before the first: sail
    /// counts from 1 at each start).
    generation: u64,
    /// The network last seen in this run, from the start's snapshot or a
    /// change.
    last: Option<NetworkSnapshot>,
    /// The re-probe timer while it still sleeps; once it fires it leaves
    /// here, so a later change never cancels a re-probe under way.
    timer: Option<JoinHandle<()>>,
    /// Which arming the sleeping timer belongs to.
    armed: u64,
}

impl NetworkState {
    fn track(&self) -> MutexGuard<'_, Track> {
        self.track.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The default interface of `snapshot` as `on_network` takes it: none when
/// offline. Offline is sail's word only: before sail knows a network the
/// snapshot names no interface yet and is not offline.
fn interface(snapshot: &NetworkSnapshot) -> Option<(&str, u32)> {
    interface_if(snapshot, snapshot.offline)
}

fn interface_if(snapshot: &NetworkSnapshot, offline: bool) -> Option<(&str, u32)> {
    if offline {
        return None;
    }
    Some((
        snapshot.interface.as_deref().unwrap_or(""),
        snapshot.index.unwrap_or(0),
    ))
}

/// What says where the network is: offline, and which interface. Addresses
/// and the gateway may differ in a way that is no step of its own.
fn same_place(s: &NetworkSnapshot) -> (bool, Option<&str>, Option<u32>) {
    (s.offline, s.interface.as_deref(), s.index)
}

/// Go's `default interface` line: `event=start|changed`, then the
/// interface, or `name=none`.
fn log_default_interface(event: &str, snapshot: &NetworkSnapshot) {
    match interface(snapshot) {
        Some((name, index)) => tracing::info!(
            event,
            name,
            index,
            addresses = snapshot.addresses.join(","),
            "default interface"
        ),
        None => tracing::info!(event, name = "none", "default interface"),
    }
}

impl Inner {
    /// After a start: the network as sail has it now. Not a change, so no
    /// NetworkChanged; the offline state follows it. A change this run has
    /// already reported is newer than the snapshot and stays.
    pub(super) fn network_started(self: &Arc<Self>) {
        if !self.take_first_network() {
            self.await_first_network();
        }
    }

    /// The snapshot as the run's first network, unless a change came first
    /// (it is newer). False when the snapshot knows no network yet (no
    /// interface, not offline): it says nothing.
    fn take_first_network(&self) -> bool {
        let mut track = self.network.track();
        if track.generation > 0 {
            return true;
        }
        let snapshot = self.runtime.network();
        track.last = snapshot.clone();
        let Some(snapshot) = snapshot.filter(|s| s.offline || s.interface.is_some()) else {
            return false;
        };
        log_default_interface("start", &snapshot);
        self.local_dns_network(&snapshot);
        let mut live = self.live();
        live.offline = snapshot.offline;
        self.settle(&mut live);
        true
    }

    /// Transitional, removed once sail's start returns with the snapshot
    /// ready (a known interface or offline, then `Restored`): today sail
    /// may start before it knows the default interface and reports no
    /// change when it learns it, so dns-local would have no interface
    /// (SERVFAIL) until the first change. The snapshot is read again for a
    /// while, within this run.
    fn await_first_network(self: &Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let run = self.live().run;
        let weak: Weak<Inner> = Arc::downgrade(self);
        handle.spawn(async move {
            for _ in 0..FIRST_NETWORK_POLLS {
                tokio::time::sleep(FIRST_NETWORK_POLL).await;
                let Some(inner) = weak.upgrade() else { return };
                {
                    let live = inner.live();
                    if !live.running || live.run != run {
                        return;
                    }
                }
                if inner.take_first_network() {
                    return;
                }
            }
            let Some(inner) = weak.upgrade() else { return };
            inner.first_network_unknown(run);
        });
    }

    /// No network known 2 s after the start: offline, as sail will say
    /// itself once it waits for its first detection.
    fn first_network_unknown(&self, run: u64) {
        let track = self.network.track();
        let mut live = self.live();
        if track.generation > 0 || !live.running || live.run != run {
            return;
        }
        tracing::warn!("no default interface known 2 s after the start: offline");
        live.offline = true;
        self.settle(&mut live);
    }

    /// At stop or shutdown: the run's network goes with it. A sleeping
    /// re-probe is cancelled (one under way holds the operation lock and
    /// ends first), and the network is unknown again: offline no longer
    /// holds once sail no longer runs.
    pub(super) fn network_stopped(&self) {
        {
            let mut track = self.network.track();
            if let Some(timer) = track.timer.take() {
                timer.abort();
            }
            track.armed += 1;
            track.generation = 0;
            track.last = None;
        }
        let mut live = self.live();
        if live.offline {
            live.offline = false;
            self.settle(&mut live);
        }
    }

    /// One change from sail's network watch, while it runs.
    pub(super) fn on_network_change(self: &Arc<Self>, change: NetworkChange) {
        if !self.live().running {
            // Late, from a run that has stopped.
            return;
        }
        let missed = {
            let mut track = self.network.track();
            let jumped = change.generation > track.generation + 1;
            let missed =
                jumped && track.last.as_ref().map(same_place) != Some(same_place(&change.old));
            track.generation = change.generation;
            track.last = Some(change.new.clone());
            missed
        };
        if missed {
            // The watch kept only this change: the step that led to `old`
            // happened unseen.
            log_default_interface("changed", &change.old);
            self.on_network(interface(&change.old));
        }
        // The change's kind decides offline (sail's one definition).
        let offline = change.change == "offline";
        log_default_interface("changed", &change.new);
        self.local_dns_network(&change.new);
        self.on_network(interface_if(&change.new, offline));
        self.guard_check("network changed");
        self.arm_reprobe();
    }

    /// (Re)arms the host IPv6 re-probe on a TUN instance: REPROBE_DELAY
    /// after the last change. Only a timer that still sleeps is cancelled;
    /// a re-probe under way runs to its end, and the next waits for it on
    /// the operation lock. The re-probe itself skips while offline.
    fn arm_reprobe(self: &Arc<Self>) {
        if self.config.role != Role::Tun {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak: Weak<Inner> = Arc::downgrade(self);
        let mut track = self.network.track();
        if let Some(previous) = track.timer.take() {
            previous.abort();
        }
        track.armed += 1;
        let armed = track.armed;
        track.timer = Some(handle.spawn(async move {
            tokio::time::sleep(REPROBE_DELAY).await;
            let Some(inner) = weak.upgrade() else { return };
            {
                let mut track = inner.network.track();
                if track.armed != armed {
                    return;
                }
                // Fired: no longer cancellable.
                track.timer = None;
            }
            inner.reprobe_host_ipv6().await;
        }));
    }
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
