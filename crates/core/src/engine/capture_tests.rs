//! Real Sail + Engine capture transactions with an in-memory host OS ledger.
//! Packet transport only; these tests never create an OS tunnel or routes.
use super::*;
use crate::capture::*;
use crate::capture_host::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Host {
    attached: AtomicBool,
    fail_attach: AtomicBool,
    fail_detach: AtomicBool,
    dirty_rollback: AtomicBool,
    drop_packets: AtomicBool,
    pause_attach: AtomicBool,
    attach_entered: tokio::sync::Notify,
    attach_release: tokio::sync::Notify,
    flow: Mutex<Option<CapturePacketFlow>>,
}
struct Provider(Arc<Host>);
#[async_trait]
impl CaptureHostApi for Provider {
    async fn prepare(
        &self,
        owner: &ForwarderIdentity,
        attachment_epoch: u64,
        spec: &CaptureSpec,
        settings: &CaptureNetworkSettings,
    ) -> CaptureHostResult<Arc<dyn CaptureAttachment>> {
        assert_eq!(owner.generation, 1);
        assert!(attachment_epoch > 0);
        assert!(
            matches!(&spec.resource, CaptureResource::HostLease{lease_id} if lease_id=="isolated-test-lease")
        );
        assert_eq!(settings.ipv4_address, crate::translate::TUN_INET4_ADDRESS);
        Ok(self.0.clone())
    }
}
#[async_trait]
impl CaptureAttachment for Host {
    fn interface_name(&self) -> &str {
        ""
    }
    fn transport(&self) -> CaptureTransport {
        CaptureTransport::Packets { capacity: 2 }
    }
    fn publish_packets(&self, flow: CapturePacketFlow) -> CaptureHostResult<()> {
        if !self.drop_packets.load(Ordering::SeqCst) {
            *self.flow.lock().unwrap() = Some(flow);
        }
        Ok(())
    }
    async fn commit_attach(&self) -> CaptureHostResult<()> {
        self.attached.store(true, Ordering::SeqCst);
        if self.pause_attach.load(Ordering::SeqCst) {
            self.attach_entered.notify_one();
            self.attach_release.notified().await;
        }
        if self.fail_attach.swap(false, Ordering::SeqCst) {
            Err(Vec::new())
        } else {
            Ok(())
        }
    }
    async fn rollback_attach(&self) -> CaptureHostResult<()> {
        if self.dirty_rollback.load(Ordering::SeqCst) {
            return Err(Vec::new());
        }
        self.attached.store(false, Ordering::SeqCst);
        self.flow.lock().unwrap().take();
        Ok(())
    }
    async fn commit_detach(&self) -> CaptureHostResult<()> {
        if self.fail_detach.swap(false, Ordering::SeqCst) {
            return Err(Vec::new());
        }
        self.attached.store(false, Ordering::SeqCst);
        Ok(())
    }
    async fn rollback_detach(&self) -> CaptureHostResult<()> {
        if self.dirty_rollback.load(Ordering::SeqCst) {
            return Err(Vec::new());
        }
        self.attached.store(true, Ordering::SeqCst);
        Ok(())
    }
}
async fn engine() -> (Engine, Arc<Host>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::default());
    let config = EngineConfig::new(Role::Shared, crate::Platform::Macos, dir.path())
        .with_local_proxy(crate::LocalProxyConfig::new().with_preferred_port(0))
        .with_capture_host(CaptureHost::new(Arc::new(Provider(host.clone()))));
    let engine = Engine::new(config).await.unwrap();
    // Explicit physical network input overrides Sail detection; no host OS
    // network state is changed. All test destinations are loopback.
    let state = crate::NetworkState {
        interface: Some("lo0".into()),
        addresses: vec!["127.0.0.1/8".into()],
        ..Default::default()
    };
    engine.set_network_state(state).unwrap();
    engine
        .apply(ApplyRequest::new(super::lifecycle_tests::profile(
            super::lifecycle_tests::R1,
        )))
        .await
        .unwrap();
    engine.start().await.unwrap();
    (engine, host, dir)
}
fn attach(epoch: u64) -> AttachCapture {
    AttachCapture {
        protocol_version: 1,
        expected_epoch: epoch,
        spec: CaptureSpec {
            resource: CaptureResource::HostLease {
                lease_id: "isolated-test-lease".into(),
            },
            ipv6: true,
        },
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_native_capture_transactions_preserve_owner_and_rollback_exact_lease() {
    let (engine, host, _dir) = engine().await;
    assert!(engine.capture_capabilities().supports_live_capture());
    let before = engine.runtime_observation();
    let first = engine.attach_capture(attach(0)).await.unwrap();
    assert_eq!(first.phase, CapturePhase::Attached);
    assert!(host.attached.load(Ordering::SeqCst));
    let lease = first.lease.clone().unwrap();
    host.fail_detach.store(true, Ordering::SeqCst);
    let failed = engine
        .detach_capture(DetachCapture {
            protocol_version: 1,
            expected_epoch: first.epoch,
            lease: lease.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(failed.code, CaptureErrorCode::CaptureOperationFailed);
    assert_eq!(failed.status.phase, CapturePhase::Attached);
    assert_eq!(failed.status.lease, Some(lease.clone()));
    assert!(host.attached.load(Ordering::SeqCst));
    let detached = engine
        .detach_capture(DetachCapture {
            protocol_version: 1,
            expected_epoch: failed.status.epoch,
            lease,
        })
        .await
        .unwrap();
    assert_eq!(detached.phase, CapturePhase::Detached);
    assert!(!host.attached.load(Ordering::SeqCst));
    host.fail_attach.store(true, Ordering::SeqCst);
    let failed = engine
        .attach_capture(attach(detached.epoch))
        .await
        .unwrap_err();
    assert_eq!(failed.code, CaptureErrorCode::CaptureOperationFailed);
    assert_eq!(failed.status.phase, CapturePhase::Detached);
    assert!(!host.attached.load(Ordering::SeqCst));
    assert_eq!(engine.runtime_observation().owner, before.owner);
    assert_eq!(
        engine.runtime_observation().native_runtime_id,
        before.native_runtime_id
    );
    let attached = engine
        .attach_capture(attach(failed.status.epoch))
        .await
        .unwrap();
    assert_eq!(attached.owner, before.owner);
    engine.stop().await.unwrap();
    assert!(!host.attached.load(Ordering::SeqCst));
    engine.start().await.unwrap();
    assert_eq!(
        engine.runtime_observation().owner.generation,
        before.owner.generation + 1
    );
    assert_eq!(
        engine.capture_status().unwrap().phase,
        CapturePhase::Detached
    );
    assert_eq!(engine.capture_status().unwrap().epoch, 0);
    engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_dirty_rollback_blocks_capture_but_keeps_forwarder_running() {
    let (engine, host, _dir) = engine().await;
    let before = engine.runtime_observation();
    host.fail_attach.store(true, Ordering::SeqCst);
    host.dirty_rollback.store(true, Ordering::SeqCst);
    let failed = engine.attach_capture(attach(0)).await.unwrap_err();
    assert_eq!(failed.code, CaptureErrorCode::CaptureRollbackFailed);
    assert_eq!(failed.status.phase, CapturePhase::RecoveryRequired);
    assert!(!failed.status.leftovers.is_empty());
    assert_eq!(
        engine
            .attach_capture(attach(failed.status.epoch))
            .await
            .unwrap_err()
            .code,
        CaptureErrorCode::CaptureRecoveryRequired
    );
    assert!(engine.runtime_observation().running);
    assert_eq!(engine.runtime_observation().owner, before.owner);
    // Explicit full shutdown reports the unresolved broker ledger.
    let report = engine.shutdown().await.unwrap();
    assert!(report.leftovers.iter().any(|l| l.name == "capture"));
    assert!(report
        .leftovers
        .iter()
        .any(|l| l.name == "capture/native-capture-1" && l.kind == crate::LeftoverKind::Task));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_native_packet_owner_has_no_proxy_and_keeps_android_protection() {
    for platform in [crate::Platform::Ios, crate::Platform::Android] {
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(Host::default());
        let protected = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut config = EngineConfig::new(Role::Tun, platform, dir.path())
            .with_capture_host(CaptureHost::new(Arc::new(Provider(host.clone()))));
        if platform == crate::Platform::Android {
            let count = protected.clone();
            config.socket_protect = Some(crate::SocketProtect::new(move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
        }
        let mut invalid = config.clone();
        invalid.tun = Some(crate::TunConfig {
            fd: Some(0),
            ..Default::default()
        });
        assert!(
            super::tun::check(&invalid).is_err(),
            "no raw startup FD fallback"
        );
        let engine = Engine::new(config).await.unwrap();
        engine
            .set_network_state(crate::NetworkState {
                interface: Some("lo0".into()),
                addresses: vec!["127.0.0.1/8".into()],
                ..Default::default()
            })
            .unwrap();
        engine
            .apply(ApplyRequest::new(super::lifecycle_tests::profile(
                super::lifecycle_tests::R1,
            )))
            .await
            .unwrap();
        engine.start().await.unwrap();
        assert!(engine.capture_capabilities().supports_live_capture());
        let cfg: serde_json::Value = serde_json::from_str(
            &engine
                .inner
                .live()
                .applied
                .as_ref()
                .unwrap()
                .translation
                .json,
        )
        .unwrap();
        assert!(
            cfg.get("inbounds")
                .is_none_or(|v| v.as_array().is_some_and(|a| a.is_empty())),
            "mobile starts no proxy or static TUN"
        );
        assert_ne!(
            cfg["route"]["auto_detect_interface"], true,
            "mobile retains socket protection/NE routing rather than desktop interface binding"
        );
        assert_eq!(
            engine.local_proxy_metadata().unwrap_err().code,
            crate::codes::LOCAL_PROXY_DISABLED
        );
        let before = engine.runtime_observation();
        let mut no_ipv6 = attach(0);
        no_ipv6.spec.ipv6 = false;
        let refused = engine.attach_capture(no_ipv6).await.unwrap_err();
        assert_eq!(refused.status.phase, CapturePhase::Detached);
        assert!(!host.attached.load(Ordering::SeqCst));
        engine
            .attach_capture(attach(refused.status.epoch))
            .await
            .unwrap();
        // A real direct dial uses this same runtime. The Android callback must
        // survive replacement of the old static HostPlatform by native capture.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let count = protected.load(Ordering::SeqCst);
        let _stream = engine
            .inner
            .runtime
            .dial_tcp(
                crate::translate::DIRECT_TAG,
                crate::runtime::Target::Addr(listener.local_addr().unwrap()),
                std::time::Duration::from_secs(3),
            )
            .await
            .unwrap();
        if platform == crate::Platform::Android {
            assert!(protected.load(Ordering::SeqCst) > count);
        }
        assert_eq!(engine.runtime_observation().owner, before.owner);
        engine.stop().await.unwrap();
        assert!(!host.attached.load(Ordering::SeqCst));
        assert!(!engine.runtime_observation().running);
        engine.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn mobile_dynamic_owner_never_restarts_to_recover_a_failed_reload() {
    use crate::runtime::fake::{Call, FakeRuntime, Op};
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeRuntime::default());
    let config = EngineConfig::new(Role::Tun, crate::Platform::Ios, dir.path()).with_capture_host(
        CaptureHost::new(Arc::new(Provider(Arc::new(Host::default())))),
    );
    let engine = Engine::with_runtime(config, fake.clone());
    engine
        .apply(ApplyRequest::new(super::lifecycle_tests::profile(
            super::lifecycle_tests::R1,
        )))
        .await
        .unwrap();
    engine.start().await.unwrap();
    let before = engine.runtime_observation().owner;
    fake.fail_next(
        Op::Reload,
        crate::runtime::RuntimeError::new("needs_restart", "test capture owner refusal"),
    );
    assert!(engine
        .apply(ApplyRequest::new(super::lifecycle_tests::profile(
            super::lifecycle_tests::R2
        )))
        .await
        .is_err());
    assert_eq!(engine.runtime_observation().owner, before);
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::Start(_)))
            .count(),
        1
    );
    assert!(!fake.calls().iter().any(|c| matches!(c, Call::Stop)));
    engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_ingress_that_fails_during_host_commit_cannot_report_attach_success() {
    let (engine, host, _dir) = engine().await;
    let before = engine.runtime_observation();
    host.drop_packets.store(true, Ordering::SeqCst);
    host.pause_attach.store(true, Ordering::SeqCst);
    let work = engine.clone();
    let attaching = tokio::spawn(async move { work.attach_capture(attach(0)).await });
    host.attach_entered.notified().await;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                engine.inner.runtime.native_capture_state(),
                Ok(sail::embed::TunState::Failed { .. })
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropped transport fails its capture scope");
    host.attach_release.notify_one();
    let error = attaching.await.unwrap().unwrap_err();
    assert_eq!(error.code, CaptureErrorCode::CaptureOperationFailed);
    assert_eq!(error.status.phase, CapturePhase::Detached);
    assert!(!host.attached.load(Ordering::SeqCst));
    assert!(engine.runtime_observation().running);
    assert_eq!(engine.runtime_observation().owner, before.owner);
    engine.shutdown().await.unwrap();
}
