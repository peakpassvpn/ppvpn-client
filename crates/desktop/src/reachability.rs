//! Reachability diagnostics in the apps (#521 step A): ppvpn-account's
//! probe over the API host and the current profile's node hosts. The user's
//! "Network diagnostics" also asks the DoH resolvers; after a call fails to
//! reach the backend, a run with the system resolver only (at most one per
//! [`AUTOMATIC_INTERVAL`]) goes to the log, so the logs a user sends carry
//! it. Nothing here changes how the client resolves or connects.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use ppvpn_account::reachability::{self, Mode, Report, Target};

use crate::session::ClientRef;
use crate::{Client, ConnectionPhase};

/// The automatic run's minimum spacing.
const AUTOMATIC_INTERVAL: Duration = Duration::from_secs(600);

/// When the automatic run last started, for the whole process.
static LAST_AUTOMATIC: Mutex<Option<Instant>> = Mutex::new(None);

impl Client {
    /// The user's run (#521): the system resolver and the DoH resolvers for
    /// the API host and the profile's node hosts, then TCP and TLS to each
    /// address. Logged as well. Takes up to about 15 seconds; never fails.
    pub async fn diagnose_reachability(&self) -> Report {
        let report =
            reachability::run(self.reachability_targets(), Mode::Compare, self.vpn_on()).await;
        tracing::info!(report = %report.json(), "reachability, asked by the user");
        report
    }

    /// The automatic run after a call failed to reach the backend: the
    /// system resolver only (no third party learns which service the user
    /// uses without the user acting), into the log.
    pub(crate) fn diagnose_reachability_after_failure(&self) {
        if cfg!(test) {
            return;
        }
        {
            let mut last = LAST_AUTOMATIC
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if last.is_some_and(|at| at.elapsed() < AUTOMATIC_INTERVAL) {
                return;
            }
            *last = Some(Instant::now());
        }
        // The targets are read on the task: the caller may hold the
        // client's locks.
        let this = self.this.clone();
        self.runtime.spawn(async move {
            let Some(client) = ClientRef::upgrade(&this) else {
                return;
            };
            let (targets, vpn_on) = (client.reachability_targets(), client.vpn_on());
            let report = reachability::run(targets, Mode::SystemOnly, vpn_on).await;
            tracing::info!(report = %report.json(), "reachability after the backend was unreachable");
        });
    }

    fn reachability_targets(&self) -> Vec<Target> {
        let mut targets: Vec<Target> = reachability::api_target(&self.config.api_base)
            .into_iter()
            .collect();
        if let Some(profile) = self.profile_raw() {
            targets.extend(reachability::profile_targets(&profile));
        }
        targets
    }

    /// The tunnel may carry the probe's connections.
    fn vpn_on(&self) -> bool {
        !matches!(
            self.snapshot().connection.phase,
            ConnectionPhase::Off | ConnectionPhase::Error
        )
    }
}
