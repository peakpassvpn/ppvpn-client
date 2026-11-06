//! Host-side transaction coordinator. A driver implements capture-only native
//! transactions on one existing forwarder. No fallback creates/restarts one.

use crate::*;
use async_trait::async_trait;
use futures_util::FutureExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait CaptureDriver: Send + Sync + 'static {
    fn identity(&self) -> ForwarderIdentity;
    fn running(&self) -> bool;
    fn capabilities(&self) -> CaptureCapabilities;
    /// Resolve resource tokens only in this owner's process. Prepare/commit
    /// device, route and DNS changes on its dispatcher. On failure, finish
    /// rollback before returning RolledBack; partial cleanup must return
    /// RollbackFailed. Never restart/replace the forwarder. The driver owns
    /// operation deadlines and serializes with forwarding shutdown/config.
    async fn apply(&self, transaction: &CaptureTransaction) -> CaptureOutcome;
}

pub struct CaptureController<D: CaptureDriver + ?Sized> {
    driver: Arc<D>,
    operation: tokio::sync::Mutex<()>,
    machine: Mutex<CaptureMachine>,
    closing: AtomicBool,
}

enum Request {
    Attach(AttachCapture),
    Detach(DetachCapture),
}

impl<D: CaptureDriver + ?Sized> CaptureController<D> {
    pub fn new(driver: Arc<D>) -> Arc<Self> {
        Arc::new(Self {
            machine: Mutex::new(CaptureMachine::new(
                driver.identity(),
                driver.capabilities(),
            )),
            driver,
            closing: AtomicBool::new(false),
            operation: tokio::sync::Mutex::new(()),
        })
    }
    fn machine(&self) -> std::sync::MutexGuard<'_, CaptureMachine> {
        self.machine.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub fn status(&self) -> CaptureStatus {
        self.machine().status().clone()
    }
    pub fn capabilities(&self) -> CaptureCapabilities {
        self.machine().capabilities().clone()
    }
    pub async fn attach(
        self: &Arc<Self>,
        request: AttachCapture,
    ) -> Result<CaptureStatus, CaptureError> {
        self.run(Request::Attach(request)).await
    }
    pub async fn detach(
        self: &Arc<Self>,
        request: DetachCapture,
    ) -> Result<CaptureStatus, CaptureError> {
        self.run(Request::Detach(request)).await
    }
    /// Before explicit forwarding shutdown: reject new/queued capture work
    /// and wait for the accepted transaction to settle. This does not detach,
    /// stop the dispatcher or terminate mixed sessions. The owner shuts down
    /// afterward with its normal complete resource cleanup.
    pub async fn quiesce(&self) -> CaptureStatus {
        self.closing.store(true, Ordering::SeqCst);
        let _operation = self.operation.lock().await;
        self.status()
    }
    fn stopping(&self) -> CaptureError {
        CaptureError {
            code: CaptureErrorCode::CaptureOwnerStopping,
            status: Box::new(self.status()),
        }
    }
    async fn run(self: &Arc<Self>, request: Request) -> Result<CaptureStatus, CaptureError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(self.stopping());
        }
        let (reply, result) = tokio::sync::oneshot::channel();
        let owner = self.clone();
        tokio::spawn(async move {
            let _operation = owner.operation.lock().await;
            // A caller that left before acceptance must not attach later.
            if reply.is_closed() {
                return;
            }
            if owner.closing.load(Ordering::SeqCst) {
                let _ = reply.send(Err(owner.stopping()));
                return;
            }
            // Once accepted, cancellation cannot abandon preparation/rollback.
            let settled = std::panic::AssertUnwindSafe(owner.execute(request))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err(owner.uncertain()));
            let _ = reply.send(settled);
        });
        result.await.unwrap_or_else(|_| Err(self.uncertain()))
    }
    fn uncertain(&self) -> CaptureError {
        let mut machine = self.machine();
        let epoch = machine.status().epoch;
        machine.require_recovery(vec![CaptureLeftover {
            kind: ResourceKind::Worker,
            resource_id: format!("capture-worker-{epoch}"),
        }])
    }

    /// Caller holds operation; all native work settles before it is released.
    async fn execute(&self, request: Request) -> Result<CaptureStatus, CaptureError> {
        let transaction = {
            let mut machine = self.machine();
            if machine.status().owner != self.driver.identity() {
                return Err(CaptureError {
                    code: CaptureErrorCode::CaptureLeaseMismatch,
                    status: Box::new(machine.status().clone()),
                });
            }
            match request {
                Request::Attach(request) => machine.begin_attach(request, self.driver.running())?,
                Request::Detach(request) => machine.begin_detach(request, self.driver.running())?,
            }
        };
        let result = std::panic::AssertUnwindSafe(self.driver.apply(&transaction))
            .catch_unwind()
            .await;
        let outcome = match result {
            Ok(outcome)
                if self.driver.identity() == *transaction.owner() && self.driver.running() =>
            {
                outcome
            }
            _ => CaptureOutcome::RollbackFailed(vec![CaptureLeftover {
                kind: ResourceKind::Worker,
                resource_id: format!("capture-transaction-{}", transaction.epoch()),
            }]),
        };
        self.machine().complete(&transaction, outcome)
    }
}

#[cfg(test)]
mod tests;
