//! What an instance undoes on the host (docs/host-integration.md, section
//! 3): the sweep at creation, and one cleanup entry for `shutdown` (10 s,
//! awaited) and for the last handle's drop (5 s, on a thread of its own,
//! never `block_on` on the caller's). The platform modules (TUN, routing,
//! DNS) plug in here.

use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::config::EngineConfig;
use crate::error::Error;
use crate::runtime::Runtime;
use crate::types::ShutdownReport;

/// `shutdown`'s limit.
pub(super) const SHUTDOWN_LIMIT: Duration = Duration::from_secs(10);
/// The last handle's drop: its limit.
pub(super) const DROP_LIMIT: Duration = Duration::from_secs(5);

/// Idempotently removes what a previous instance on this host left (rules,
/// routes, adapters it can tell are its own), before anything else at `new`.
pub(super) fn sweep(config: &EngineConfig) -> Result<(), Error> {
    // sweep: the platform sweep module (separate PR) runs here.
    let _ = config;
    Ok(())
}

/// Undoes everything the instance set up, within `deadline`: stops the
/// runtime if it runs; what is not done in time is a leftover (the next
/// instance's sweep takes it).
pub(super) async fn cleanup(
    runtime: Arc<dyn Runtime>,
    running: bool,
    deadline: Duration,
) -> ShutdownReport {
    let mut report = ShutdownReport::default();
    if running {
        match tokio::time::timeout(deadline, runtime.stop()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => report.leftovers.push(format!("runtime: {e}")),
            Err(_) => report.leftovers.push("runtime: stop timed out".into()),
        }
    }
    // The platform cleanup module (separate PR) runs here: TUN, routing,
    // DNS inside the TUN.
    for leftover in &report.leftovers {
        tracing::warn!(leftover = %leftover, "not cleaned up");
    }
    report
}

/// The last handle went without `shutdown`: `cleanup` on a thread of its
/// own (with its own small runtime), waited for at most DROP_LIMIT.
pub(super) fn cleanup_on_drop(runtime: Arc<dyn Runtime>, running: bool) {
    let (done, wait) = std_mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("ppvpn-core-cleanup".into())
        .spawn(move || {
            let report = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .map(|rt| rt.block_on(cleanup(runtime, running, DROP_LIMIT)));
            let _ = done.send(report.is_ok());
        });
    match spawned {
        Ok(_) => {
            if !matches!(wait.recv_timeout(DROP_LIMIT), Ok(true)) {
                tracing::warn!("cleanup at drop did not finish in time");
            }
        }
        Err(e) => tracing::warn!(error = %e, "cannot start the cleanup thread"),
    }
}
