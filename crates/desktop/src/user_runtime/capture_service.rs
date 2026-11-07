//! Presentation adapter: enhanced mode owns a capture lease on the same user
//! forwarder. It never sends privileged Connect, launches another core, or stops
//! forwarding when VPN capture is disabled.
use super::*;
use crate::{
    core_ipc::ApplyChoices,
    service::{ServiceError, ServiceStatus, VersionInfo, WatchEnd},
    RoutingMode,
};
use ppvpn_core::capture::{CaptureCapabilities, CaptureLease, CapturePhase, CaptureStatus};
use std::sync::Mutex;
#[derive(Clone)]
struct Active {
    session: SessionRef,
    lease: CaptureLease,
}
pub(crate) struct CaptureService<S: ServiceApi + ?Sized> {
    service: Arc<S>,
    hosts: Vec<String>,
    active: Mutex<Option<Active>>,
    operation: tokio::sync::Mutex<()>,
}
fn error(code: &str) -> ServiceError {
    ServiceError::Failed(code.into())
}
impl<S: ServiceApi + ?Sized> CaptureService<S> {
    pub fn new(service: Arc<S>, hosts: Vec<String>) -> Self {
        Self {
            service,
            hosts,
            active: Mutex::new(None),
            operation: tokio::sync::Mutex::new(()),
        }
    }
    async fn owner(&self) -> Result<SessionRef, ServiceError> {
        identity(self.service.as_ref())
            .await
            .map_err(|_| error("USER_RUNTIME_UNAVAILABLE"))
    }
    async fn call(&self, path: &str, body: Value) -> Result<Value, ServiceError> {
        self.service
            .core_api(&self.owner().await?, path, body, Duration::from_secs(15))
            .await
    }
    fn active(&self, session: &SessionRef) -> Result<Active, ServiceError> {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .filter(|a| &a.session == session)
            .cloned()
            .ok_or_else(|| error("CAPTURE_LEASE_MISMATCH"))
    }
    async fn capture(&self) -> Result<CaptureStatus, ServiceError> {
        serde_json::from_value(self.call("/v1/get-capture-status", json!({})).await?)
            .map_err(|_| error("CAPTURE_STATUS_INVALID"))
    }
    async fn apply(
        &self,
        profile: &Value,
        mode: RoutingMode,
        choices: &ApplyChoices,
    ) -> Result<(), ServiceError> {
        self.call(
            "/v1/apply-profile",
            crate::core_ipc::profile_request(profile, Some(&self.hosts), Some(mode), choices),
        )
        .await?;
        Ok(())
    }
    async fn status(&self) -> Result<ServiceStatus, ServiceError> {
        let mut status = self.service.get_status().await?;
        let active = self.active.lock().unwrap().clone();
        status.running = false;
        status.session_id = None;
        status.generation = 0;
        status.mode = "shared-capture".into();
        if let Some(active) = active {
            let capture = self.capture().await?;
            if capture.phase == CapturePhase::Attached
                && capture.lease.as_ref() == Some(&active.lease)
            {
                status.running = true;
                status.session_id = Some(active.session.session_id);
                status.generation = active.session.generation;
            }
        }
        Ok(status)
    }
}
impl<S: ServiceApi + ?Sized> ServiceApi for CaptureService<S> {
    fn get_version(&self) -> BoxFuture<'_, Result<VersionInfo, ServiceError>> {
        self.service.get_version()
    }
    fn get_status(&self) -> BoxFuture<'_, Result<ServiceStatus, ServiceError>> {
        Box::pin(self.status())
    }
    fn connect<'a>(
        &'a self,
        session: &'a SessionRef,
        profile: &'a Value,
        _take_over: bool,
        mode: RoutingMode,
        choices: &'a ApplyChoices,
    ) -> BoxFuture<'a, Result<u32, ServiceError>> {
        Box::pin(async move {
            let _operation = self.operation.lock().await;
            if self.active.lock().unwrap().is_some() {
                return Err(error("SERVICE_BUSY"));
            }
            let caps: CaptureCapabilities =
                serde_json::from_value(self.call("/v1/get-capture-capabilities", json!({})).await?)
                    .map_err(|_| error("CAPTURE_CAPABILITIES_INVALID"))?;
            if !caps.supports_live_capture() {
                return Err(error("CAPTURE_UNSUPPORTED"));
            }
            self.apply(profile, mode, choices).await?;
            self.call("/v1/start", json!({})).await?;
            let before = self.capture().await?;
            if before.phase != CapturePhase::Detached {
                return Err(error("SERVICE_BUSY"));
            }
            let attached:CaptureStatus=serde_json::from_value(self.call("/v1/attach-capture",json!({"protocol_version":1,"expected_epoch":before.epoch,"spec":{"resource":{"kind":"host_lease","lease_id":uuid::Uuid::new_v4().to_string()},"ipv6":true}})).await?).map_err(|_|error("CAPTURE_STATUS_INVALID"))?;
            let lease = attached
                .lease
                .ok_or_else(|| error("CAPTURE_LEASE_MISSING"))?;
            *self.active.lock().unwrap() = Some(Active {
                session: session.clone(),
                lease,
            });
            u32::try_from(self.service.get_status().await?.pid)
                .map_err(|_| error("USER_RUNTIME_PID_INVALID"))
        })
    }
    fn update_profile<'a>(
        &'a self,
        session: &'a SessionRef,
        profile: &'a Value,
        mode: RoutingMode,
        choices: &'a ApplyChoices,
    ) -> BoxFuture<'a, Result<ServiceStatus, ServiceError>> {
        Box::pin(async move {
            let _operation = self.operation.lock().await;
            self.active(session)?;
            self.apply(profile, mode, choices).await?;
            self.status().await
        })
    }
    fn renew_lease<'a>(
        &'a self,
        session: &'a SessionRef,
    ) -> BoxFuture<'a, Result<ServiceStatus, ServiceError>> {
        Box::pin(async move {
            let active = self.active(session)?;
            self.call(
                "/v1/renew-capture-lease",
                serde_json::to_value(active.lease).map_err(|_| error("CAPTURE_LEASE_INVALID"))?,
            )
            .await?;
            self.status().await
        })
    }
    fn disconnect<'a>(
        &'a self,
        session: &'a SessionRef,
    ) -> BoxFuture<'a, Result<(), ServiceError>> {
        Box::pin(async move {
            let _operation = self.operation.lock().await;
            let active = match self.active(session) {
                Ok(active) => active,
                Err(_) => return Ok(()),
            };
            let current = self.capture().await?;
            if current.phase != CapturePhase::Detached {
                if current.lease.as_ref() != Some(&active.lease) {
                    return Err(error("CAPTURE_LEASE_MISMATCH"));
                }
                self.call("/v1/detach-capture",json!({"protocol_version":1,"expected_epoch":current.epoch,"lease":active.lease})).await?;
            }
            self.active.lock().unwrap().take();
            Ok(())
        })
    }
    fn core_api<'a>(
        &'a self,
        session: &'a SessionRef,
        path: &'a str,
        body: Value,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Value, ServiceError>> {
        Box::pin(async move {
            self.active(session)?;
            if path == "/v1/stop" {
                return Err(error("USER_RUNTIME_LIFETIME_OWNED"));
            }
            self.service
                .core_api(&self.owner().await?, path, body, timeout)
                .await
        })
    }
    fn watch<'a>(&'a self, session: &'a SessionRef) -> BoxFuture<'a, WatchEnd> {
        Box::pin(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                match self.status().await {
                    Ok(status)
                        if status.running
                            && status.session_id.as_deref() == Some(&session.session_id)
                            && status.generation == session.generation => {}
                    Ok(_) => return WatchEnd::CoreStopped("capture detached".into()),
                    Err(e) => return WatchEnd::Lost(e.detail()),
                }
            }
        })
    }
    fn reset_auth(&self) {
        self.service.reset_auth();
    }
}

#[cfg(test)]
mod tests;
