use super::*;
use ppvpn_core::capture::{CaptureMachine, ForwarderIdentity, Preservation};
struct Fake {
    state: Mutex<CaptureStatus>,
    calls: Mutex<Vec<(String, Value)>>,
    supported: bool,
}
fn caps(supported: bool) -> CaptureCapabilities {
    if !supported {
        return CaptureCapabilities::unsupported();
    }
    CaptureCapabilities {
        protocol_version: 1,
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
fn fake(supported: bool) -> Arc<Fake> {
    Arc::new(Fake {
        state: Mutex::new(
            CaptureMachine::new(
                ForwarderIdentity {
                    instance: "one-user-forwarder".into(),
                    generation: 1,
                },
                caps(supported),
            )
            .status()
            .clone(),
        ),
        calls: Mutex::new(vec![]),
        supported,
    })
}
impl ServiceApi for Fake {
    fn get_version(&self) -> BoxFuture<'_, Result<VersionInfo, ServiceError>> {
        Box::pin(async { Ok(VersionInfo::default()) })
    }
    fn get_status(&self) -> BoxFuture<'_, Result<ServiceStatus, ServiceError>> {
        Box::pin(async {
            Ok(ServiceStatus {
                running: true,
                pid: 42,
                mode: "user-runtime".into(),
                session_id: Some("one-user-forwarder".into()),
                generation: 1,
                ..Default::default()
            })
        })
    }
    fn connect<'a>(
        &'a self,
        _: &'a SessionRef,
        _: &'a Value,
        _: bool,
        _: RoutingMode,
        _: &'a ApplyChoices,
    ) -> BoxFuture<'a, Result<u32, ServiceError>> {
        panic!("must never launch a root/second core")
    }
    fn update_profile<'a>(
        &'a self,
        _: &'a SessionRef,
        _: &'a Value,
        _: RoutingMode,
        _: &'a ApplyChoices,
    ) -> BoxFuture<'a, Result<ServiceStatus, ServiceError>> {
        panic!("must use shared CoreApi")
    }
    fn renew_lease<'a>(
        &'a self,
        _: &'a SessionRef,
    ) -> BoxFuture<'a, Result<ServiceStatus, ServiceError>> {
        panic!("must renew capture only")
    }
    fn disconnect<'a>(&'a self, _: &'a SessionRef) -> BoxFuture<'a, Result<(), ServiceError>> {
        panic!("must not stop a privileged forwarding core")
    }
    fn core_api<'a>(
        &'a self,
        owner: &'a SessionRef,
        path: &'a str,
        body: Value,
        _: Duration,
    ) -> BoxFuture<'a, Result<Value, ServiceError>> {
        Box::pin(async move {
            assert_eq!(owner.session_id, "one-user-forwarder");
            assert_eq!(owner.generation, 1);
            self.calls.lock().unwrap().push((path.into(), body.clone()));
            let mut s = self.state.lock().unwrap();
            match path {
                "/v1/get-capture-capabilities" => {
                    return Ok(serde_json::to_value(caps(self.supported)).unwrap())
                }
                "/v1/attach-capture" => {
                    assert_eq!(s.phase, CapturePhase::Detached);
                    assert_eq!(body["expected_epoch"], s.epoch);
                    s.epoch += 1;
                    s.phase = CapturePhase::Attached;
                    s.lease = Some(CaptureLease {
                        owner: s.owner.clone(),
                        attachment_epoch: s.epoch,
                    });
                }
                "/v1/detach-capture" => {
                    assert_eq!(body["lease"], serde_json::to_value(&s.lease).unwrap());
                    assert_eq!(body["expected_epoch"], s.epoch);
                    s.epoch += 1;
                    s.phase = CapturePhase::Detached;
                    s.lease = None;
                }
                "/v1/renew-capture-lease" => {
                    assert_eq!(body, serde_json::to_value(&s.lease).unwrap())
                }
                "/v1/stop" => panic!("VPN off must not stop mixed forwarding"),
                _ => (),
            }
            Ok(serde_json::to_value(&*s).unwrap())
        })
    }
}
#[tokio::test]
async fn enhanced_toggle_maps_to_capture_on_one_owner_and_rejects_stale_ui() {
    let raw = fake(true);
    let service = CaptureService::new(raw.clone(), vec!["example.com".into()]);
    let session = SessionRef {
        session_id: "ui".into(),
        generation: 3,
    };
    let other = SessionRef {
        session_id: "stale-ui".into(),
        generation: 2,
    };
    let profile = json!({"revision":"r1"});
    let choices = ApplyChoices::default();
    for _ in 0..10 {
        assert_eq!(
            service
                .connect(&session, &profile, false, RoutingMode::Rules, &choices)
                .await
                .unwrap(),
            42
        );
        assert!(service.get_status().await.unwrap().running);
        assert_eq!(
            service.get_status().await.unwrap().session_id.as_deref(),
            Some("ui")
        );
        assert!(service.renew_lease(&other).await.is_err());
        service.disconnect(&other).await.unwrap();
        assert!(service.get_status().await.unwrap().running);
        service.renew_lease(&session).await.unwrap();
        service.disconnect(&session).await.unwrap();
        assert!(!service.get_status().await.unwrap().running);
        service.disconnect(&session).await.unwrap();
    }
    let calls = raw.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(p, _)| p == "/v1/attach-capture")
            .count(),
        10
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(p, _)| p == "/v1/detach-capture")
            .count(),
        10
    );
    assert!(!calls.iter().any(|(p, _)| p == "/v1/stop"));
    let apply = &calls
        .iter()
        .find(|(p, _)| p == "/v1/apply-profile")
        .unwrap()
        .1;
    assert_eq!(apply["allowed_rule_set_hosts"], json!(["example.com"]));
}
#[tokio::test]
async fn unsupported_backend_is_refused_before_profile_or_start_without_fallback() {
    let raw = fake(false);
    let service = CaptureService::new(raw.clone(), vec![]);
    let session = SessionRef {
        session_id: "ui".into(),
        generation: 1,
    };
    assert!(service
        .connect(
            &session,
            &json!({}),
            false,
            RoutingMode::Rules,
            &ApplyChoices::default()
        )
        .await
        .is_err());
    assert_eq!(
        raw.calls.lock().unwrap().as_slice(),
        [("/v1/get-capture-capabilities".into(), json!({}))]
    );
}
