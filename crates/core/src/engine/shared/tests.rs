use crate::capture::{CaptureResource, CaptureSpec, RuntimeBackend};
use crate::engine::{
    lifecycle_tests::{profile, R1, R2},
    tun::HostIpv6,
};
use crate::runtime::{
    fake::{Call, FakeRuntime, Op},
    RuntimeError,
};
use crate::{ApplyRequest, Engine, EngineConfig, LocalProxyConfig, Platform, Role};
use serde_json::Value;
use std::sync::Arc;

fn engine() -> (Engine, Arc<FakeRuntime>, std::path::PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ppvpn-shared-plan-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Shared, Platform::Macos, &dir)
            .with_local_proxy(LocalProxyConfig::new().with_preferred_port(0)),
        fake.clone(),
    );
    engine.inner.set_host_ipv6_probe(|| HostIpv6 {
        available: true,
        route: Ok(true),
    });
    (engine, fake, dir)
}

#[tokio::test]
async fn capture_planning_preserves_forwarding_policy_and_mixed_credentials() {
    let (engine, fake, dir) = engine();
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    engine.start().await.unwrap();
    let before = engine.runtime_observation();
    let calls = fake.calls();
    let _op = engine.inner.op.lock().await;
    let detached = engine.inner.shared_capture_translation(None).unwrap();
    let base: Value = serde_json::from_str(&detached.json).unwrap();
    assert!(!base["inbounds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["type"] == "tun"));
    assert!(base["inbounds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["type"] == "mixed" && !i["users"].as_array().unwrap().is_empty()));
    assert!(!base["dns"].is_null());
    for ipv6 in [false, true] {
        let attached = engine
            .inner
            .shared_capture_translation(Some(&CaptureSpec {
                resource: CaptureResource::HostLease {
                    lease_id: "test-opaque-host-lease".into(),
                },
                ipv6,
            }))
            .unwrap();
        let mut actual: Value = serde_json::from_str(&attached.json).unwrap();
        let inbounds = actual["inbounds"].as_array_mut().unwrap();
        let tun = inbounds.iter().find(|i| i["type"] == "tun").unwrap();
        assert_ne!(tun["auto_route"], true);
        inbounds.retain(|i| i["type"] != "tun");
        assert_eq!(
            actual, base,
            "capture only adds the TUN inbound, including IPv6 changes"
        );
        assert_eq!(attached.node_tags, detached.node_tags);
        assert_eq!(attached.rule_ids, detached.rule_ids);
    }
    assert_eq!(
        fake.calls(),
        calls,
        "planning cannot start/reload/stop a runtime"
    );
    assert_eq!(engine.runtime_observation(), before);
    drop(_op);
    engine.shutdown().await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn shared_reload_requiring_restart_keeps_original_runtime() {
    let (engine, fake, dir) = engine();
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    engine.start().await.unwrap();
    let before = engine.runtime_observation();
    fake.fail_next(
        Op::Reload,
        RuntimeError::new("needs_restart", "test restart refusal"),
    );
    assert!(engine.apply(ApplyRequest::new(profile(R2))).await.is_err());
    assert_eq!(engine.runtime_observation(), before);
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::Start(_)))
            .count(),
        1
    );
    assert!(!fake.calls().iter().any(|c| matches!(c, Call::Stop)));
    engine.shutdown().await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn fake_observation_distinguishes_reload_from_successful_restart() {
    let (engine, _fake, dir) = engine();
    let idle = engine.runtime_observation();
    assert_eq!(idle.backend, RuntimeBackend::Fake);
    assert_eq!(idle.owner.generation, 0);
    assert_eq!(idle.active_runtime_ids, None);
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    engine.start().await.unwrap();
    let first = engine.runtime_observation();
    assert_eq!(first.owner.generation, 1);
    engine.apply(ApplyRequest::new(profile(R2))).await.unwrap();
    assert_eq!(engine.runtime_observation().owner, first.owner);
    engine.stop().await.unwrap();
    engine.start().await.unwrap();
    let next = engine.runtime_observation();
    assert_eq!(next.owner.instance, first.owner.instance);
    assert_eq!(next.owner.generation, 2);
    engine.shutdown().await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shared_role_is_not_enabled_on_mobile() {
    for platform in [
        Platform::Ios,
        Platform::Android,
        Platform::Linux,
        Platform::Windows,
    ] {
        let config = EngineConfig::new(Role::Shared, platform, "/unused");
        assert!(crate::engine::tun::check(&config).is_err());
    }
}

#[tokio::test]
async fn shared_start_does_not_retry_without_its_mixed_listener() {
    let (engine, fake, dir) = engine();
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    fake.fail_next(Op::Start, RuntimeError::new("io", "listener refused"));
    assert!(engine.start().await.is_err());
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::Start(_)))
            .count(),
        1
    );
    assert!(!engine.inner.local_proxy_left_out());
    engine.shutdown().await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
