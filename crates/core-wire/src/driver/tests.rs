use super::*;
use crate::tests::{attach, capabilities, owner};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Forwarding {
    dispatcher: u64,
    tcp: Vec<u64>,
    udp: Vec<u64>,
    auth: Vec<String>,
    dns: Vec<String>,
    selected: String,
    counters: (u64, u64),
    starts: usize,
    stops: usize,
}
#[derive(Clone, Copy)]
enum Outcome {
    Commit,
    Rollback,
    Dirty,
    Panic,
    Replaced,
}
struct FakeRuntime {
    forwarding: Mutex<Forwarding>,
    capture: Mutex<bool>,
    outcome: Mutex<Outcome>,
    generation: AtomicUsize,
    calls: AtomicUsize,
    gate: Mutex<Option<Arc<Gate>>>,
}
#[derive(Default)]
struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl FakeRuntime {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            forwarding: Mutex::new(Forwarding {
                dispatcher: 41,
                tcp: vec![101, 102],
                udp: vec![201],
                auth: vec!["synthetic-user".into()],
                dns: vec!["synthetic-cache".into()],
                selected: "node-b".into(),
                counters: (300, 400),
                starts: 1,
                stops: 0,
            }),
            capture: Mutex::new(false),
            outcome: Mutex::new(Outcome::Commit),
            generation: AtomicUsize::new(1),
            calls: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }
}
#[async_trait]
impl CaptureDriver for FakeRuntime {
    fn identity(&self) -> ForwarderIdentity {
        ForwarderIdentity {
            generation: self.generation.load(Ordering::SeqCst) as u64,
            ..owner()
        }
    }
    fn running(&self) -> bool {
        true
    }
    fn capabilities(&self) -> CaptureCapabilities {
        capabilities()
    }
    async fn apply(&self, transaction: &CaptureTransaction) -> CaptureOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        let before = *self.capture.lock().unwrap();
        *self.capture.lock().unwrap() = matches!(transaction.change(), CaptureChange::Attach(_));
        let outcome = *self.outcome.lock().unwrap();
        match outcome {
            Outcome::Commit => CaptureOutcome::Committed,
            Outcome::Rollback => {
                *self.capture.lock().unwrap() = before;
                CaptureOutcome::RolledBack
            }
            Outcome::Dirty => CaptureOutcome::RollbackFailed(vec![CaptureLeftover {
                kind: ResourceKind::Route,
                resource_id: "owned-route".into(),
            }]),
            Outcome::Panic => panic!("synthetic native driver panic"),
            Outcome::Replaced => {
                self.generation.fetch_add(1, Ordering::SeqCst);
                CaptureOutcome::Committed
            }
        }
    }
}
fn detach_request(c: &CaptureController<FakeRuntime>) -> DetachCapture {
    let status = c.status();
    DetachCapture {
        protocol_version: 1,
        expected_epoch: status.epoch,
        lease: status.lease.unwrap(),
    }
}
async fn entered(gate: &Gate) {
    tokio::time::timeout(std::time::Duration::from_secs(2), gate.entered.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn attach_and_detach_preserve_the_same_dispatcher_mixed_sessions_and_state() {
    let runtime = FakeRuntime::new();
    let c = CaptureController::new(runtime.clone());
    let before = runtime.forwarding.lock().unwrap().clone();
    c.attach(attach(0)).await.unwrap();
    assert!(*runtime.capture.lock().unwrap());
    c.detach(detach_request(&c)).await.unwrap();
    assert!(!*runtime.capture.lock().unwrap());
    assert_eq!(*runtime.forwarding.lock().unwrap(), before);
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 2);
    assert_eq!(c.status().owner, owner());
}
#[tokio::test]
async fn failed_attach_and_detach_settle_rollback_before_answering() {
    let runtime = FakeRuntime::new();
    let c = CaptureController::new(runtime.clone());
    *runtime.outcome.lock().unwrap() = Outcome::Rollback;
    c.attach(attach(0)).await.unwrap_err();
    assert!(!*runtime.capture.lock().unwrap());
    *runtime.outcome.lock().unwrap() = Outcome::Commit;
    c.attach(attach(1)).await.unwrap();
    let before = c.status();
    *runtime.outcome.lock().unwrap() = Outcome::Rollback;
    c.detach(detach_request(&c)).await.unwrap_err();
    assert!(*runtime.capture.lock().unwrap());
    assert_eq!(c.status().lease, before.lease);
}
#[tokio::test]
async fn dropping_the_ipc_caller_does_not_abandon_an_in_progress_transaction() {
    let runtime = FakeRuntime::new();
    let c = CaptureController::new(runtime.clone());
    let gate = Arc::new(Gate::default());
    *runtime.gate.lock().unwrap() = Some(gate.clone());
    let task = tokio::spawn({
        let c = c.clone();
        async move { c.attach(attach(0)).await }
    });
    entered(&gate).await;
    assert_eq!(c.status().phase, CapturePhase::Attaching);
    task.abort();
    let _ = task.await;
    gate.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while c.status().phase == CapturePhase::Attaching {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(c.status().phase, CapturePhase::Attached);
    assert!(*runtime.capture.lock().unwrap());
}
#[tokio::test]
async fn dirty_rollback_and_panics_block_capture_work_without_restarting_forwarding() {
    for outcome in [Outcome::Dirty, Outcome::Panic] {
        let runtime = FakeRuntime::new();
        let c = CaptureController::new(runtime.clone());
        *runtime.outcome.lock().unwrap() = outcome;
        assert_eq!(
            c.attach(attach(0)).await.unwrap_err().code,
            CaptureErrorCode::CaptureRollbackFailed
        );
        assert_eq!(
            c.attach(attach(1)).await.unwrap_err().code,
            CaptureErrorCode::CaptureRecoveryRequired
        );
        let state = runtime.forwarding.lock().unwrap();
        assert_eq!((state.starts, state.stops), (1, 0));
    }
}
#[tokio::test]
async fn a_driver_that_replaces_the_owner_cannot_report_success() {
    let runtime = FakeRuntime::new();
    let c = CaptureController::new(runtime.clone());
    *runtime.outcome.lock().unwrap() = Outcome::Replaced;
    assert_eq!(
        c.attach(attach(0)).await.unwrap_err().code,
        CaptureErrorCode::CaptureRollbackFailed
    );
    assert_eq!(c.status().phase, CapturePhase::RecoveryRequired);
}
#[tokio::test]
async fn concurrent_callers_execute_only_one_native_transaction_for_an_epoch() {
    let runtime = FakeRuntime::new();
    let c = CaptureController::new(runtime.clone());
    let gate = Arc::new(Gate::default());
    *runtime.gate.lock().unwrap() = Some(gate.clone());
    let first = tokio::spawn({
        let c = c.clone();
        async move { c.attach(attach(0)).await }
    });
    entered(&gate).await;
    let second = tokio::spawn({
        let c = c.clone();
        async move { c.attach(attach(0)).await }
    });
    gate.release.notify_one();
    first.await.unwrap().unwrap();
    assert_eq!(
        second.await.unwrap().unwrap_err().code,
        CaptureErrorCode::CaptureEpochMismatch
    );
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelling_a_queued_detach_does_not_remove_capture_later() {
    use std::future::Future;
    use std::task::Poll;
    let runtime = FakeRuntime::new();
    let c = CaptureController::new(runtime.clone());
    let gate = Arc::new(Gate::default());
    *runtime.gate.lock().unwrap() = Some(gate.clone());
    let first = tokio::spawn({
        let c = c.clone();
        async move { c.attach(attach(0)).await }
    });
    entered(&gate).await;
    let request = DetachCapture {
        protocol_version: 1,
        expected_epoch: 1,
        lease: CaptureLease {
            owner: owner(),
            attachment_epoch: 1,
        },
    };
    let mut queued = Box::pin(c.detach(request));
    std::future::poll_fn(|cx| {
        assert!(queued.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(queued);
    gate.release.notify_one();
    first.await.unwrap().unwrap();
    let _settled = c.operation.lock().await;
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    assert!(*runtime.capture.lock().unwrap());
    assert_eq!(c.status().phase, CapturePhase::Attached);
}

#[tokio::test]
async fn forwarding_shutdown_waits_for_settlement_and_rejects_new_capture_work() {
    use std::future::Future;
    use std::task::Poll;
    let runtime = FakeRuntime::new();
    let c = CaptureController::new(runtime.clone());
    let gate = Arc::new(Gate::default());
    *runtime.gate.lock().unwrap() = Some(gate.clone());
    let first = tokio::spawn({
        let c = c.clone();
        async move { c.attach(attach(0)).await }
    });
    entered(&gate).await;
    let mut closing = Box::pin(c.quiesce());
    std::future::poll_fn(|cx| {
        assert!(closing.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(
        c.attach(attach(1)).await.unwrap_err().code,
        CaptureErrorCode::CaptureOwnerStopping
    );
    gate.release.notify_one();
    first.await.unwrap().unwrap();
    assert_eq!(closing.await.phase, CapturePhase::Attached);
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.forwarding.lock().unwrap().stops, 0);
}

#[tokio::test]
async fn shutdown_detaches_only_the_exact_active_capture_and_closes_admission() {
    let driver = FakeRuntime::new();
    let before = driver.forwarding.lock().unwrap().clone();
    let controller = CaptureController::new(driver.clone());
    controller.attach(attach(0)).await.unwrap();
    let stopped = controller.detach_for_shutdown().await.unwrap();
    assert_eq!(stopped.phase, CapturePhase::Detached);
    assert!(!*driver.capture.lock().unwrap());
    assert_eq!(*driver.forwarding.lock().unwrap(), before);
    assert_eq!(
        controller
            .attach(attach(stopped.epoch))
            .await
            .unwrap_err()
            .code,
        CaptureErrorCode::CaptureOwnerStopping
    );
    assert_eq!(controller.detach_for_shutdown().await.unwrap(), stopped);
}

#[tokio::test]
async fn native_invalidation_quarantines_a_settled_attachment() {
    let controller = CaptureController::new(FakeRuntime::new());
    let attached = controller.attach(attach(0)).await.unwrap();
    let status = controller.invalidate_attached(vec![CaptureLeftover {
        kind: ResourceKind::Worker,
        resource_id: "native-failed".into(),
    }]);
    assert_eq!(status.phase, CapturePhase::RecoveryRequired);
    assert_eq!(status.lease, attached.lease);
    assert_eq!(
        controller
            .attach(attach(status.epoch))
            .await
            .unwrap_err()
            .code,
        CaptureErrorCode::CaptureRecoveryRequired
    );
}

#[tokio::test]
async fn cancelled_shutdown_detach_keeps_settling_native_cleanup() {
    let driver = FakeRuntime::new();
    let controller = CaptureController::new(driver.clone());
    controller.attach(attach(0)).await.unwrap();
    let gate = Arc::new(Gate::default());
    *driver.gate.lock().unwrap() = Some(gate.clone());
    let work = controller.clone();
    let caller = tokio::spawn(async move { work.detach_for_shutdown().await });
    gate.entered.notified().await;
    caller.abort();
    gate.release.notify_one();
    controller.quiesce().await;
    assert_eq!(controller.status().phase, CapturePhase::Detached);
    assert!(!*driver.capture.lock().unwrap());
}
