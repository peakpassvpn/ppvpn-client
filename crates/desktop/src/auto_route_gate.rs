//! Client-wide mutation lease and synchronous invalidation. Lock order:
//! mutation -> mode_switch/cores -> session/snapshot -> Engine transport gate.
//! Synchronous callbacks invalidate versions but never block waiting for lease.
use crate::auto_route::Epoch;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub(crate) enum Change {
    Intent,
    Scope,
    Network,
    Profile,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Version {
    pub epoch: Epoch,
    pub request: u64,
}
#[derive(Clone)]
pub(crate) struct Gate {
    mutation: Arc<tokio::sync::RwLock<()>>,
    version: Arc<Mutex<Option<Version>>>,
}
impl Default for Gate {
    fn default() -> Self {
        Self {
            mutation: Arc::new(tokio::sync::RwLock::new(())),
            version: Arc::new(Mutex::new(Some(Version {
                epoch: Epoch {
                    session: 1,
                    network: 1,
                    profile: 1,
                },
                request: 1,
            }))),
        }
    }
}
impl Gate {
    pub fn snapshot(&self) -> Option<Version> {
        self.version
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
    /// Invalidate immediately, before awaiting a busy writer. Overflow disables
    /// this gate's Auto identity permanently; never wrap to a previous version.
    pub fn invalidate(&self, change: Change) {
        let mut state = self
            .version
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(mut next) = state.clone() else {
            return;
        };
        let advanced = (|| {
            next.request = next.request.checked_add(1)?;
            match change {
                Change::Intent => {}
                Change::Scope => next.epoch.session = next.epoch.session.checked_add(1)?,
                Change::Network => next.epoch.network = next.epoch.network.checked_add(1)?,
                Change::Profile => next.epoch.profile = next.epoch.profile.checked_add(1)?,
            }
            Some(next)
        })();
        *state = advanced;
    }
    /// Existing legacy actions retain their concurrency. An automatic
    /// transaction needs the exclusive lease spanning its entire journal.
    pub async fn enter(&self) -> tokio::sync::OwnedRwLockReadGuard<()> {
        self.mutation.clone().read_owned().await
    }
    pub async fn enter_automatic(&self) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        self.mutation.clone().write_owned().await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn intent_invalidates_before_waiting_and_clones_share_one_lease() {
        let gate = Gate::default();
        let previous = gate.snapshot().unwrap();
        let lease = gate.enter().await;
        let other = gate.clone();
        other.invalidate(Change::Scope);
        assert_ne!(gate.snapshot().unwrap(), previous);
        assert!(other.mutation.try_write().is_err());
        drop(lease);
        assert!(other.mutation.try_write().is_ok());
    }
    #[tokio::test]
    async fn automatic_lease_excludes_legacy_mutations_without_serializing_legacy_calls() {
        let gate = Gate::default();
        let first = gate.enter().await;
        let second = tokio::time::timeout(std::time::Duration::from_secs(1), gate.enter())
            .await
            .unwrap();
        drop((first, second));
        let automatic = gate.enter_automatic().await;
        let old = gate.snapshot().unwrap();
        gate.invalidate(Change::Intent);
        assert_ne!(gate.snapshot().unwrap(), old);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), gate.enter())
                .await
                .is_err()
        );
        drop(automatic);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), gate.enter())
                .await
                .is_ok()
        );
    }

    #[test]
    fn network_and_profile_do_not_reuse_old_scope_or_wrap_versions() {
        let gate = Gate::default();
        let previous = gate.snapshot().unwrap();
        gate.invalidate(Change::Network);
        gate.invalidate(Change::Profile);
        let now = gate.snapshot().unwrap();
        assert_eq!(now.epoch.session, previous.epoch.session);
        assert_eq!(now.epoch.network, previous.epoch.network + 1);
        assert_eq!(now.epoch.profile, previous.epoch.profile + 1);
        gate.version.lock().unwrap().as_mut().unwrap().request = u64::MAX;
        gate.invalidate(Change::Intent);
        assert!(gate.snapshot().is_none());
        gate.invalidate(Change::Scope);
        assert!(gate.snapshot().is_none());
    }
}
