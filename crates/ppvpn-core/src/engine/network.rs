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

/// What the Engine follows of sail's network.
#[derive(Default)]
pub(super) struct NetworkState {
    track: Mutex<Track>,
}

#[derive(Default)]
struct Track {
    /// The last change's generation (0 after a start: sail counts from 1).
    generation: u64,
    /// The network last seen, from a start's snapshot or a change.
    last: Option<NetworkSnapshot>,
    /// The armed re-probe, if any.
    reprobe: Option<JoinHandle<()>>,
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
    if snapshot.offline {
        return None;
    }
    Some((
        snapshot.interface.as_deref().unwrap_or(""),
        snapshot.index.unwrap_or(0),
    ))
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
    /// NetworkChanged; the offline state follows it.
    pub(super) fn network_started(&self) {
        let snapshot = self.runtime.network();
        {
            let mut track = self.network.track();
            track.generation = 0;
            track.last = snapshot.clone();
        }
        // A snapshot that knows no network yet (no interface, not offline)
        // says nothing: what was known before stays.
        if let Some(snapshot) = snapshot.filter(|s| s.offline || s.interface.is_some()) {
            log_default_interface("start", &snapshot);
            let mut live = self.live();
            live.offline = snapshot.offline;
            self.settle(&mut live);
        }
    }

    /// At stop: a pending re-probe goes with the run.
    pub(super) fn network_stopped(&self) {
        if let Some(reprobe) = self.network.track().reprobe.take() {
            reprobe.abort();
        }
    }

    /// One change from sail's network watch.
    pub(super) fn on_network_change(self: &Arc<Self>, change: NetworkChange) {
        let missed = {
            let mut track = self.network.track();
            let jumped = change.generation > track.generation + 1;
            let missed = jumped && track.last.as_ref() != Some(&change.old);
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
        log_default_interface("changed", &change.new);
        self.on_network(interface(&change.new));
        self.arm_reprobe();
    }

    /// (Re)arms the host IPv6 re-probe on a TUN instance: REPROBE_DELAY
    /// after the last change. The re-probe itself skips while offline and
    /// waits for an apply in progress.
    fn arm_reprobe(self: &Arc<Self>) {
        if self.config.role != Role::Tun {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak: Weak<Inner> = Arc::downgrade(self);
        let mut track = self.network.track();
        if let Some(previous) = track.reprobe.take() {
            previous.abort();
        }
        track.reprobe = Some(handle.spawn(async move {
            tokio::time::sleep(REPROBE_DELAY).await;
            if let Some(inner) = weak.upgrade() {
                inner.reprobe_host_ipv6().await;
            }
        }));
    }
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
