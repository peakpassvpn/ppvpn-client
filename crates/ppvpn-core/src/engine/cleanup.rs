//! What an instance undoes on the host (docs/host-integration.md, section
//! 3): the sweep at creation, and one teardown for `shutdown` (10 s,
//! awaited) and for the last handle's drop (5 s, on a thread of its own,
//! which the dropping thread waits for without `block_on`: a host that
//! exits right after the drop leaves no rules or routes behind).
//!
//! The Engine gathers what an instance holds into [`Parts`], in teardown
//! order: the runtime stops first (listeners closed, connections dropped),
//! then each [`Step`] runs in turn (the TUN's routing, its DNS, ...), and
//! the state directory's lock is released last, so that the next instance
//! finds everything gone. A step that fails or misses the deadline, and the
//! steps after it, are leftovers in the ShutdownReport; the next instance's
//! sweep takes them.

use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{EngineConfig, Platform, Role};
use crate::error::Error;
use crate::runtime::Runtime;
use crate::state_dir::StateDirLock;
use crate::types::ShutdownReport;
use crate::{translate, tunrules};

/// `shutdown`'s limit.
pub(super) const SHUTDOWN_LIMIT: Duration = Duration::from_secs(10);
/// The last handle's drop: its limit.
pub(super) const DROP_LIMIT: Duration = Duration::from_secs(5);
/// How long past the deadline `cleanup` waits for the steps' own report,
/// which names each step left (the steps keep the deadline themselves).
const REPORT_GRACE: Duration = Duration::from_millis(200);

/// Idempotently removes what a previous instance on this host left (rules,
/// routes, adapters it can tell are its own), before anything else at `new`.
///
/// Linux Tun instances: the TUN routing rules and table a previous
/// instance's sail left (tunrules: our own priority range and table only).
/// A sweep that fails is logged and does not stop `new`: what it could not
/// remove does not keep this instance from working, and the guard puts
/// back what this instance needs. Other platforms come with their sweep.
pub(super) fn sweep(config: &EngineConfig) -> Result<(), Error> {
    if config.role == Role::Tun && config.platform == Platform::Linux {
        let scope = tunrules::Scope::desktop(translate::interface_name(config.platform));
        if let Err(e) = tunrules::sweep(&scope) {
            tracing::warn!(error = %e, "tun routing leftovers could not be removed");
        }
    }
    Ok(())
}

/// One synchronous piece of the teardown: restore the routing rules, remove
/// the TUN's DNS, ... An error is a leftover with its message.
pub(super) struct Step {
    pub name: &'static str,
    pub run: Box<dyn FnOnce() -> Result<(), String> + Send>,
}

#[allow(dead_code)] // the platform modules' steps (tunrules, TUN DNS) use it
impl Step {
    pub(super) fn new(
        name: &'static str,
        run: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Self {
        Step {
            name,
            run: Box::new(run),
        }
    }
}

/// What an instance holds, in teardown order.
#[derive(Default)]
pub(super) struct Parts {
    /// Set when the runtime runs: it is stopped first.
    pub runtime: Option<Arc<dyn Runtime>>,
    pub steps: Vec<Step>,
    pub state_dir: Option<StateDirLock>,
}

/// `shutdown`: takes `parts` down within `deadline` and reports what is left.
/// The steps run on a thread of their own, so that one stuck in a system
/// call cannot hold the caller past the deadline; it is left running and
/// named in the report.
pub(super) async fn cleanup(parts: Parts, deadline: Duration) -> ShutdownReport {
    let until = Instant::now() + deadline;
    let Parts {
        runtime,
        steps,
        state_dir,
    } = parts;
    let mut leftovers = Vec::new();
    if let Some(runtime) = runtime {
        match tokio::time::timeout_at(until.into(), runtime.stop()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => leftovers.push(format!("runtime: {e}")),
            Err(_) => leftovers.push("runtime: stop timed out".into()),
        }
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = thread::Builder::new()
        .name("ppvpn-core-cleanup".into())
        .spawn(move || {
            let _ = tx.send(run_steps(steps, until, state_dir));
        });
    match spawned {
        Ok(_) => match tokio::time::timeout_at((until + REPORT_GRACE).into(), rx).await {
            Ok(Ok(mut left)) => leftovers.append(&mut left),
            // No report: what the thread had not finished is unknown from
            // here, so name the phase.
            _ => leftovers.push("cleanup steps: did not finish in time".into()),
        },
        Err(e) => leftovers.push(format!("cleanup steps: no thread: {e}")),
    }
    report(leftovers)
}

/// The last handle went without `shutdown`: the same teardown on a thread of
/// its own (the runtime stopped on a small runtime there), waited for at
/// most DROP_LIMIT on the dropping thread, without `block_on`, so that it is
/// safe on a tokio worker. What is not done by then is a leftover, logged;
/// the thread goes on and the next instance's sweep takes what it leaves.
pub(super) fn cleanup_on_drop(parts: Parts) -> ShutdownReport {
    let (tx, rx) = std_mpsc::channel();
    let spawned = thread::Builder::new()
        .name("ppvpn-core-cleanup".into())
        .spawn(move || {
            let until = Instant::now() + DROP_LIMIT;
            let Parts {
                runtime,
                steps,
                state_dir,
            } = parts;
            let mut leftovers = Vec::new();
            if let Some(runtime) = runtime {
                let stopped = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map(|rt| {
                        rt.block_on(async {
                            tokio::time::timeout_at(until.into(), runtime.stop()).await
                        })
                    });
                match stopped {
                    Ok(Ok(Ok(()))) => {}
                    Ok(Ok(Err(e))) => leftovers.push(format!("runtime: {e}")),
                    Ok(Err(_)) => leftovers.push("runtime: stop timed out".into()),
                    Err(e) => leftovers.push(format!("runtime: {e}")),
                }
            }
            leftovers.append(&mut run_steps(steps, until, state_dir));
            let _ = tx.send(leftovers);
        });
    let leftovers = match spawned {
        Ok(_) => rx
            .recv_timeout(DROP_LIMIT + REPORT_GRACE)
            .unwrap_or_else(|_| vec!["cleanup at drop: did not finish in time".into()]),
        Err(e) => vec![format!("cleanup at drop: no thread: {e}")],
    };
    report(leftovers)
}

/// Runs the steps in order until `until`, then releases the state directory.
/// Each step runs on a thread of its own so that the deadline holds even
/// when one blocks; a step that misses it, and every step after it, is a
/// leftover. The lock is released only when every step finished: otherwise
/// a new instance could start beside a teardown still at work.
fn run_steps(steps: Vec<Step>, until: Instant, state_dir: Option<StateDirLock>) -> Vec<String> {
    let mut leftovers = Vec::new();
    let mut steps = steps.into_iter();
    while let Some(step) = steps.next() {
        let left = until.saturating_duration_since(Instant::now());
        let (tx, rx) = std_mpsc::channel();
        let run = step.run;
        let started = thread::Builder::new()
            .name(format!("ppvpn-core-cleanup-{}", step.name))
            .spawn(move || {
                let _ = tx.send(run());
            });
        match started.ok().and_then(|_| rx.recv_timeout(left).ok()) {
            Some(Ok(())) => {}
            Some(Err(e)) => leftovers.push(format!("{}: {e}", step.name)),
            None => {
                leftovers.push(format!("{}: did not finish in time", step.name));
                leftovers.extend(steps.map(|s| format!("{}: not run", s.name)));
                // Kept held: the directory stays this instance's until the
                // process ends.
                std::mem::forget(state_dir);
                return leftovers;
            }
        }
    }
    drop(state_dir);
    leftovers
}

fn report(leftovers: Vec<String>) -> ShutdownReport {
    for leftover in &leftovers {
        tracing::warn!(leftover = %leftover, "not cleaned up");
    }
    ShutdownReport { leftovers }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::runtime::fake::FakeRuntime;
    use crate::runtime::RuntimeState;

    type Order = Arc<Mutex<Vec<&'static str>>>;

    fn order() -> (Order, impl Fn(&'static str) -> Step) {
        let log: Order = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        (log, move |name| {
            let l = l.clone();
            Step::new(name, move || {
                l.lock().unwrap().push(name);
                Ok(())
            })
        })
    }

    fn state_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ppvpn-core-cleanup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sleeping(name: &'static str) -> Step {
        Step::new(name, || {
            thread::sleep(Duration::from_secs(3));
            Ok(())
        })
    }

    #[tokio::test]
    async fn cleanup_runs_everything_in_order_and_frees_the_directory() {
        let (log, step) = order();
        let dir = state_dir("order");
        let runtime = Arc::new(FakeRuntime::default());
        runtime.start("{}").await.unwrap();
        let parts = Parts {
            runtime: Some(runtime.clone()),
            steps: vec![step("routing"), step("dns")],
            state_dir: Some(StateDirLock::acquire(&dir).unwrap()),
        };
        let report = cleanup(parts, SHUTDOWN_LIMIT).await;
        assert!(report.leftovers.is_empty(), "{report:?}");
        assert_eq!(*log.lock().unwrap(), ["routing", "dns"]);
        assert_eq!(runtime.state(), RuntimeState::Stopped);
        StateDirLock::acquire(&dir).expect("free");
    }

    #[tokio::test]
    async fn a_failing_step_is_a_leftover_and_the_rest_still_run() {
        let (log, step) = order();
        let parts = Parts {
            steps: vec![
                Step::new("routing", || Err("rule 9093 is gone".into())),
                step("dns"),
            ],
            ..Parts::default()
        };
        let report = cleanup(parts, SHUTDOWN_LIMIT).await;
        assert_eq!(report.leftovers, ["routing: rule 9093 is gone"]);
        assert_eq!(*log.lock().unwrap(), ["dns"]);
    }

    #[tokio::test]
    async fn a_stuck_step_does_not_hold_cleanup_past_its_deadline() {
        let dir = state_dir("stuck");
        let parts = Parts {
            steps: vec![sleeping("routing"), Step::new("dns", || Ok(()))],
            state_dir: Some(StateDirLock::acquire(&dir).unwrap()),
            ..Parts::default()
        };
        let started = Instant::now();
        let report = cleanup(parts, Duration::from_millis(300)).await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            report.leftovers,
            ["routing: did not finish in time", "dns: not run"]
        );
        // The teardown may still be at work: the directory is not given away.
        assert!(StateDirLock::acquire(&dir).is_err());
    }

    #[tokio::test]
    async fn drop_finishes_the_teardown_before_it_returns() {
        let (log, step) = order();
        let dir = state_dir("drop");
        let runtime = Arc::new(FakeRuntime::default());
        runtime.start("{}").await.unwrap();
        let parts = Parts {
            runtime: Some(runtime.clone()),
            steps: vec![
                Step::new("slow", || {
                    thread::sleep(Duration::from_millis(200));
                    Ok(())
                }),
                step("dns"),
            ],
            state_dir: Some(StateDirLock::acquire(&dir).unwrap()),
        };
        let report = cleanup_on_drop(parts);
        assert!(report.leftovers.is_empty(), "{report:?}");
        assert_eq!(*log.lock().unwrap(), ["dns"]);
        assert_eq!(runtime.state(), RuntimeState::Stopped);
        StateDirLock::acquire(&dir).expect("free");
    }

    #[test]
    fn drop_waits_at_most_its_limit() {
        let parts = Parts {
            steps: vec![sleeping("routing")],
            ..Parts::default()
        };
        let started = Instant::now();
        // DROP_LIMIT is 5 s and the step 3 s: it finishes in time.
        let report = cleanup_on_drop(parts);
        assert!(report.leftovers.is_empty(), "{report:?}");
        assert!(started.elapsed() < DROP_LIMIT + Duration::from_secs(1));
    }

    #[test]
    fn drop_inside_a_tokio_runtime_is_safe() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = rt.block_on(async {
            let runtime = Arc::new(FakeRuntime::default());
            runtime.start("{}").await.unwrap();
            cleanup_on_drop(Parts {
                runtime: Some(runtime),
                ..Parts::default()
            })
        });
        assert!(report.leftovers.is_empty(), "{report:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_on_a_multi_thread_runtime_worker() {
        let runtime = Arc::new(FakeRuntime::default());
        runtime.start("{}").await.unwrap();
        let report = cleanup_on_drop(Parts {
            runtime: Some(runtime.clone()),
            ..Parts::default()
        });
        assert!(report.leftovers.is_empty(), "{report:?}");
        assert_eq!(runtime.state(), RuntimeState::Stopped);
    }

    #[test]
    fn sweep_leaves_alone_what_it_does_not_own() {
        // A Standard instance has no TUN routing; nor has any instance off
        // Linux yet: nothing to sweep, nothing that can fail.
        let standard = EngineConfig::new(Role::Standard, Platform::Linux, state_dir("sweep"));
        assert!(sweep(&standard).is_ok());
        let macos = EngineConfig::new(Role::Tun, Platform::Macos, state_dir("sweep-mac"));
        assert!(sweep(&macos).is_ok());
    }
}
