//! Opt-in presentation client for the persistent user service. No Engine,
//! subprocess launcher or privileged Connect fallback exists in this module.
use crate::{
    core_ipc::{BoxFuture, CoreCallError, CoreTransport},
    errors::{ClientErrorInfo, ErrorCode},
    service::{ServiceApi, ServiceCoreTransport, SessionRef},
    standard::{CoreLauncher, Launched},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::oneshot;
pub(crate) struct RemoteLauncher<S: ServiceApi + ?Sized> {
    service: Arc<S>,
    hosts: Vec<String>,
}
impl<S: ServiceApi + ?Sized> RemoteLauncher<S> {
    pub(crate) fn new(service: Arc<S>, hosts: Vec<String>) -> Self {
        Self { service, hosts }
    }
}
fn failure(detail: impl Into<String>) -> ClientErrorInfo {
    ClientErrorInfo::new(ErrorCode::StandardCoreFailed, detail.into())
}
async fn identity<S: ServiceApi + ?Sized>(service: &S) -> Result<SessionRef, ClientErrorInfo> {
    let status = service
        .get_status()
        .await
        .map_err(|_| failure("USER_RUNTIME_UNAVAILABLE"))?;
    if status.mode != "user-runtime" || status.generation == 0 {
        return Err(failure("USER_RUNTIME_IDENTITY_INVALID"));
    }
    let session_id = status
        .session_id
        .filter(|id| !id.is_empty())
        .ok_or_else(|| failure("USER_RUNTIME_IDENTITY_INVALID"))?;
    Ok(SessionRef {
        session_id,
        generation: status.generation,
    })
}
struct PresentationTransport<S: ServiceApi + ?Sized> {
    inner: ServiceCoreTransport<S>,
}
impl<S: ServiceApi + ?Sized> CoreTransport for PresentationTransport<S> {
    fn call<'a>(
        &'a self,
        path: &'static str,
        body: Value,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Value, CoreCallError>> {
        Box::pin(async move {
            // StandardCore::stop on app exit detaches this client only.
            // Account end uses RemoteLauncher::end_session explicitly below.
            if path == "/v1/stop" {
                return Ok(json!({}));
            }
            self.inner.call(path, body, timeout).await
        })
    }
}
impl<S: ServiceApi + ?Sized + 'static> CoreLauncher for RemoteLauncher<S> {
    fn launch(&self) -> BoxFuture<'_, Result<Launched, ClientErrorInfo>> {
        Box::pin(async move {
            let session = identity(self.service.as_ref()).await?;
            let transport = Arc::new(PresentationTransport {
                inner: ServiceCoreTransport {
                    service: self.service.clone(),
                    session: session.clone(),
                },
            });
            let (stop, mut stopped) = oneshot::channel();
            let service = self.service.clone();
            let exited = Box::pin(async move {
                loop {
                    tokio::select! {
                        _=&mut stopped=>return "presentation detached".into(),
                        _=tokio::time::sleep(Duration::from_secs(5))=>{
                            match identity(service.as_ref()).await {
                                Ok(current) if current==session=>(),
                                _=>return "user runtime unavailable or replaced".into(),
                            }
                        }
                    }
                }
            });
            Ok(Launched {
                transport,
                exited,
                stop,
                rule_set_hosts: self.hosts.clone(),
                credentials_reset: false,
            })
        })
    }
    fn end_session(&self) -> BoxFuture<'_, Result<(), ClientErrorInfo>> {
        Box::pin(async move {
            let session = identity(self.service.as_ref()).await?;
            self.service
                .core_api(
                    &session,
                    "/v1/stop",
                    json!({"owner_reason":"session_ended"}),
                    Duration::from_secs(10),
                )
                .await
                .map_err(|_| failure("USER_RUNTIME_SESSION_END_FAILED"))?;
            Ok(())
        })
    }
    // macOS Sail owns detection. Do not turn UI network hints into host-pushed
    // generic move events (which the pinned Sail may use to close old flows).
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_ipc::ApplyChoices;
    use crate::service::{ServiceError, ServiceStatus, VersionInfo};
    use crate::RoutingMode;
    use std::sync::Mutex;
    struct Fake {
        status: Mutex<ServiceStatus>,
        calls: Mutex<Vec<(String, Value)>>,
    }
    fn unsupported<T: Send + 'static>() -> BoxFuture<'static, Result<T, ServiceError>> {
        Box::pin(async { Err(ServiceError::Failed("CAPTURE_UNSUPPORTED".into())) })
    }
    impl ServiceApi for Fake {
        fn get_version(&self) -> BoxFuture<'_, Result<VersionInfo, ServiceError>> {
            unsupported()
        }
        fn get_status(&self) -> BoxFuture<'_, Result<ServiceStatus, ServiceError>> {
            Box::pin(async { Ok(self.status.lock().unwrap().clone()) })
        }
        fn connect<'a>(
            &'a self,
            _: &'a SessionRef,
            _: &'a Value,
            _: bool,
            _: RoutingMode,
            _: &'a ApplyChoices,
        ) -> BoxFuture<'a, Result<u32, ServiceError>> {
            panic!("remote launcher must not invoke a privileged core")
        }
        fn update_profile<'a>(
            &'a self,
            _: &'a SessionRef,
            _: &'a Value,
            _: RoutingMode,
            _: &'a ApplyChoices,
        ) -> BoxFuture<'a, Result<ServiceStatus, ServiceError>> {
            unsupported()
        }
        fn renew_lease<'a>(
            &'a self,
            _: &'a SessionRef,
        ) -> BoxFuture<'a, Result<ServiceStatus, ServiceError>> {
            unsupported()
        }
        fn disconnect<'a>(&'a self, _: &'a SessionRef) -> BoxFuture<'a, Result<(), ServiceError>> {
            unsupported()
        }
        fn core_api<'a>(
            &'a self,
            _: &'a SessionRef,
            path: &'a str,
            body: Value,
            _: Duration,
        ) -> BoxFuture<'a, Result<Value, ServiceError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push((path.into(), body));
                Ok(json!({}))
            })
        }
    }
    fn fake() -> Arc<Fake> {
        Arc::new(Fake {
            status: Mutex::new(ServiceStatus {
                mode: "user-runtime".into(),
                session_id: Some("only-runtime".into()),
                generation: 1,
                ..Default::default()
            }),
            calls: Mutex::new(vec![]),
        })
    }
    #[tokio::test]
    async fn ui_exit_and_network_hint_do_not_stop_the_service_but_signout_is_explicit() {
        let service = fake();
        let launcher = RemoteLauncher::new(service.clone(), vec![]);
        let launched = launcher.launch().await.unwrap();
        launched
            .transport
            .call("/v1/get-status", json!({}), Duration::from_secs(1))
            .await
            .unwrap();
        launched
            .transport
            .call("/v1/stop", json!({}), Duration::from_secs(1))
            .await
            .unwrap();
        launcher
            .network_changed(ppvpn_core::NetworkState::offline())
            .unwrap();
        launched.stop.send(()).unwrap();
        assert_eq!(launched.exited.await, "presentation detached");
        assert_eq!(service.calls.lock().unwrap().len(), 1);
        launcher.end_session().await.unwrap();
        let calls = service.calls.lock().unwrap();
        assert_eq!(calls[1].0, "/v1/stop");
        assert_eq!(calls[1].1["owner_reason"], "session_ended");
    }
    #[tokio::test]
    async fn foreign_or_legacy_service_cannot_be_used_as_user_runtime() {
        let service = fake();
        service.status.lock().unwrap().mode = "tun".into();
        let launcher = RemoteLauncher::new(service.clone(), vec![]);
        assert!(launcher.launch().await.is_err());
        assert!(service.calls.lock().unwrap().is_empty());
    }
}
