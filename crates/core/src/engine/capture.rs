//! One native capture coordinator per successful forwarding run. Lock order:
//! controller operation -> Engine operation -> Sail capture operation.
use super::{Engine, Inner};
use crate::capture::driver::{CaptureController, CaptureDriver};
use crate::capture::*;
use crate::capture_host::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

const STEP_LIMIT: Duration = Duration::from_secs(3);
pub(super) type Controller = CaptureController<NativeDriver>;
#[derive(Default)]
pub(super) struct State {
    pub controller: Mutex<Option<Arc<Controller>>>,
    pub stop_epoch: AtomicU64,
}
#[derive(Clone)]
struct Active {
    resource: Arc<dyn CaptureAttachment>,
    inbound: String,
}
pub(super) struct NativeDriver {
    inner: Weak<Inner>,
    active: Mutex<Option<Active>>,
}
fn unknown(epoch: u64) -> Vec<CaptureLeftover> {
    vec![CaptureLeftover {
        kind: ResourceKind::Worker,
        resource_id: format!("native-capture-{epoch}"),
    }]
}
async fn host_step<T>(
    work: impl std::future::Future<Output = CaptureHostResult<T>>,
    epoch: u64,
) -> CaptureHostResult<T> {
    tokio::time::timeout(STEP_LIMIT, work)
        .await
        .unwrap_or_else(|_| Err(unknown(epoch)))
}
fn live_capabilities() -> CaptureCapabilities {
    CaptureCapabilities {
        protocol_version: CAPTURE_PROTOCOL_VERSION,
        live_attach: true,
        live_detach: true,
        transactional_rollback: true,
        cross_process_migration: false,
        preserves: Preservation {
            dispatcher: true,
            mixed_tcp: true,
            mixed_udp: true,
            dns_state: true,
            selection: true,
            counters: true,
        },
    }
}
impl Engine {
    pub fn capture_status(&self) -> Result<CaptureStatus, CaptureError> {
        let controller = self
            .inner
            .capture_controller()
            .ok_or_else(|| self.inner.capture_unavailable())?;
        let status = controller.status();
        if status.phase == CapturePhase::Attached
            && matches!(
                self.inner.runtime.native_capture_state(),
                Ok(sail::embed::TunState::Failed { .. }
                    | sail::embed::TunState::Draining { .. }
                    | sail::embed::TunState::Detached)
                    | Err(_)
            )
        {
            return Ok(controller.invalidate_attached(unknown(status.epoch)));
        }
        Ok(status)
    }
    pub async fn attach_capture(
        &self,
        request: AttachCapture,
    ) -> Result<CaptureStatus, CaptureError> {
        let c = self
            .inner
            .capture_controller()
            .ok_or_else(|| self.inner.capture_unavailable())?;
        c.attach(request).await
    }
    pub async fn detach_capture(
        &self,
        request: DetachCapture,
    ) -> Result<CaptureStatus, CaptureError> {
        let c = self
            .inner
            .capture_controller()
            .ok_or_else(|| self.inner.capture_unavailable())?;
        c.detach(request).await
    }
}
impl Inner {
    pub(super) fn capture_capabilities(&self) -> CaptureCapabilities {
        if self.config.uses_dynamic_capture()
            && self.config.capture_host.is_some()
            && self.runtime.has_dynamic_capture()
            && self.network.host().is_some()
        {
            live_capabilities()
        } else {
            CaptureCapabilities::unsupported()
        }
    }
    fn capture_unavailable(&self) -> CaptureError {
        CaptureError {
            code: if self.capture_capabilities().supports_live_capture() {
                CaptureErrorCode::CaptureNotRunning
            } else {
                CaptureErrorCode::CaptureUnsupported
            },
            status: Box::new(
                CaptureMachine::new(
                    self.runtime.observation().owner,
                    self.capture_capabilities(),
                )
                .status()
                .clone(),
            ),
        }
    }
    pub(super) fn capture_controller(&self) -> Option<Arc<Controller>> {
        self.capture
            .controller
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub(super) fn install_capture(self: &Arc<Self>, start_ticket: u64) {
        if !self.capture_capabilities().supports_live_capture() {
            return;
        }
        let driver = Arc::new(NativeDriver {
            inner: Arc::downgrade(self),
            active: Mutex::new(None),
        });
        let controller = CaptureController::new(driver);
        if self.capture.stop_epoch.load(Ordering::SeqCst) != start_ticket {
            controller.close_admission();
        }
        *self
            .capture
            .controller
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(controller);
    }
    pub(super) fn close_capture_admission(&self) -> Option<Arc<Controller>> {
        self.capture.stop_epoch.fetch_add(1, Ordering::SeqCst);
        let controller = self.capture_controller();
        if let Some(c) = &controller {
            c.close_admission();
        }
        controller
    }
}
#[async_trait]
impl CaptureDriver for NativeDriver {
    fn identity(&self) -> ForwarderIdentity {
        self.inner
            .upgrade()
            .map(|i| i.runtime.observation().owner)
            .unwrap_or(ForwarderIdentity {
                instance: "owner-ended".into(),
                generation: 0,
            })
    }
    fn running(&self) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|i| i.live().running && i.runtime.observation().running)
    }
    fn capabilities(&self) -> CaptureCapabilities {
        self.inner
            .upgrade()
            .map(|i| i.capture_capabilities())
            .unwrap_or_else(CaptureCapabilities::unsupported)
    }
    async fn apply(&self, txn: &CaptureTransaction) -> CaptureOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return CaptureOutcome::RollbackFailed(unknown(txn.epoch()));
        };
        let _op = inner.op.lock().await;
        if inner.admit().is_err() || !self.running() || self.identity() != *txn.owner() {
            return CaptureOutcome::RollbackFailed(unknown(txn.epoch()));
        }
        match txn.change() {
            CaptureChange::Attach(spec) => self.attach(&inner, spec, txn.epoch()).await,
            CaptureChange::Detach(_) => self.detach(&inner, txn.epoch()).await,
        }
    }
}
impl NativeDriver {
    async fn attach(&self, inner: &Inner, spec: &CaptureSpec, epoch: u64) -> CaptureOutcome {
        // Existing mobile VPN captures IPv6 even on a physical IPv4-only
        // network. Do not silently omit its IPv6 route in the new transport.
        if crate::mobile::mobile(inner.config.platform) && !spec.ipv6 {
            return CaptureOutcome::RolledBack;
        }
        let Ok(plan) = inner.shared_capture_translation(Some(spec)) else {
            return CaptureOutcome::RolledBack;
        };
        let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&plan.json) else {
            return CaptureOutcome::RolledBack;
        };
        let Some(inbound) = json["inbounds"]
            .as_array_mut()
            .and_then(|i| i.iter_mut().find(|v| v["type"] == "tun"))
        else {
            return CaptureOutcome::RolledBack;
        };
        let settings = CaptureNetworkSettings {
            mtu: crate::translate::TUN_MTU,
            ipv4_address: crate::translate::TUN_INET4_ADDRESS.into(),
            ipv6_address: spec
                .ipv6
                .then(|| crate::translate::TUN_INET6_ADDRESS.into()),
            dns_servers: vec![crate::translate::TUN_DNS_ADDRESS.into()],
        };
        let host = inner.config.capture_host.as_ref().expect("capability host");
        let resource = match host_step(
            host.0.prepare(&self.identity(), epoch, spec, &settings),
            epoch,
        )
        .await
        {
            Ok(resource) => resource,
            Err(left) if left.is_empty() => return CaptureOutcome::RolledBack,
            Err(left) => return CaptureOutcome::RollbackFailed(left),
        };
        let transport = resource.transport();
        inbound["interface_name"] = resource.interface_name().into();
        let active = Active {
            resource: resource.clone(),
            inbound: inbound.to_string(),
        };
        *self.active.lock().unwrap_or_else(|p| p.into_inner()) = Some(active.clone());
        if inner.config.platform == crate::Platform::Ios && transport == CaptureTransport::Device {
            return self.rollback_attach(inner, &active, epoch).await;
        }
        if transport == CaptureTransport::Device {
            let physical = inner
                .network
                .host()
                .as_deref()
                .and_then(|json| serde_json::from_str::<crate::NetworkState>(json).ok())
                .and_then(|s| s.interface);
            if physical.as_deref() == Some(resource.interface_name()) {
                return self.rollback_attach(inner, &active, epoch).await;
            }
        }
        let prepared = match transport {
            CaptureTransport::Packets { capacity } => tokio::time::timeout(
                STEP_LIMIT,
                inner
                    .runtime
                    .prepare_capture_packets(&active.inbound, capacity),
            )
            .await
            .unwrap_or_else(|_| {
                Err(crate::runtime::RuntimeError::new(
                    "timeout",
                    "capture prepare timed out",
                ))
            })
            .map(|(p, f)| (p, Some(f))),
            CaptureTransport::Device => {
                #[cfg(unix)]
                {
                    if resource.interface_name().is_empty()
                        || inner
                            .runtime
                            .stage_capture_device(Some(resource.clone()))
                            .is_err()
                    {
                        return self.rollback_attach(inner, &active, epoch).await;
                    }
                    tokio::time::timeout(
                        STEP_LIMIT,
                        inner.runtime.prepare_capture_device(&active.inbound),
                    )
                    .await
                    .unwrap_or_else(|_| {
                        Err(crate::runtime::RuntimeError::new(
                            "timeout",
                            "capture prepare timed out",
                        ))
                    })
                    .map(|p| (p, None))
                }
                #[cfg(not(unix))]
                {
                    Err(crate::runtime::RuntimeError::new(
                        "unsupported",
                        "device capture unavailable",
                    ))
                }
            }
        };
        let Ok((prepared, flow)) = prepared else {
            return self.rollback_attach(inner, &active, epoch).await;
        };
        if !matches!(
            tokio::time::timeout(STEP_LIMIT, prepared.commit()).await,
            Ok(Ok(()))
        ) {
            return self.rollback_attach(inner, &active, epoch).await;
        }
        if let Some(flow) = flow {
            if resource.publish_packets(flow).is_err() {
                return self.rollback_attach(inner, &active, epoch).await;
            }
        }
        if host_step(resource.commit_attach(), epoch).await.is_err()
            || !matches!(
                inner.runtime.native_capture_state(),
                Ok(sail::embed::TunState::Attached { .. })
            )
        {
            return self.rollback_attach(inner, &active, epoch).await;
        }
        CaptureOutcome::Committed
    }
    async fn rollback_attach(&self, inner: &Inner, active: &Active, epoch: u64) -> CaptureOutcome {
        let native = tokio::time::timeout(STEP_LIMIT, inner.runtime.detach_native_capture()).await;
        let host = host_step(active.resource.rollback_attach(), epoch).await;
        if matches!(native,Ok(Ok(ref r)) if r.clean()) && host.is_ok() {
            let _ = inner.runtime.stage_capture_device(None);
            *self.active.lock().unwrap_or_else(|p| p.into_inner()) = None;
            CaptureOutcome::RolledBack
        } else {
            let mut left = host.err().unwrap_or_default();
            left.extend(unknown(epoch));
            CaptureOutcome::RollbackFailed(left)
        }
    }
    async fn detach(&self, inner: &Inner, epoch: u64) -> CaptureOutcome {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let Some(active) = active else {
            return CaptureOutcome::RollbackFailed(unknown(epoch));
        };
        if host_step(active.resource.commit_detach(), epoch)
            .await
            .is_err()
        {
            return match host_step(active.resource.rollback_detach(), epoch).await {
                Ok(()) => CaptureOutcome::RolledBack,
                Err(mut left) => {
                    left.extend(unknown(epoch));
                    CaptureOutcome::RollbackFailed(left)
                }
            };
        }
        let native = tokio::time::timeout(STEP_LIMIT, inner.runtime.detach_native_capture()).await;
        if matches!(native,Ok(Ok(ref r)) if r.clean()) {
            let _ = inner.runtime.stage_capture_device(None);
            *self.active.lock().unwrap_or_else(|p| p.into_inner()) = None;
            CaptureOutcome::Committed
        } else if matches!(
            inner.runtime.native_capture_state(),
            Ok(sail::embed::TunState::Attached { .. })
        ) {
            match host_step(active.resource.rollback_detach(), epoch).await {
                Ok(()) => CaptureOutcome::RolledBack,
                Err(mut left) => {
                    left.extend(unknown(epoch));
                    CaptureOutcome::RollbackFailed(left)
                }
            }
        } else {
            // Draining/failed native state cannot honestly restore capture.
            // Keep the resource/FD ledger alive and OS admission removed.
            CaptureOutcome::RollbackFailed(unknown(epoch))
        }
    }
}

pub(super) fn shutdown_leftovers(status: &CaptureStatus) -> Vec<crate::Leftover> {
    status
        .leftovers
        .iter()
        .map(|left| {
            let kind = match left.kind {
                ResourceKind::Worker => crate::LeftoverKind::Task,
                ResourceKind::Route => crate::LeftoverKind::Route,
                ResourceKind::Dns => crate::LeftoverKind::Dns,
                ResourceKind::Device | ResourceKind::Handle => crate::LeftoverKind::Tun,
            };
            crate::Leftover::new(
                kind,
                format!("capture/{}", left.resource_id),
                "owned capture resource requires broker reconciliation",
            )
        })
        .collect()
}
