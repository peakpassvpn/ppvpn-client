//! The local proxy and the system proxy listener on the fake runtime.
//! Credentials are compared with `assert!`, never printed.

use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::lifecycle_tests::{drain, profile, running, NODE_1, NODE_2, R1};
use super::*;
use crate::config::{LocalProxyConfig, Platform};
use crate::localproxy::STATE_FILE;
use crate::runtime::fake::{Call, FakeRuntime, Op};
use crate::runtime::RuntimeError;
use crate::status::{CredentialsResetReason, DegradedReason, EngineState};
use crate::translate::{LOCAL_PROXY_INBOUND_TAG, SYSTEM_PROXY_INBOUND_TAG};
use crate::types::LocalProxyKind;

/// A Standard instance with a local proxy in `dir`, on any free port (tests
/// must not race for 7890).
fn standard(dir: &Path, system_proxy: bool) -> (Engine, Arc<FakeRuntime>) {
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, dir)
            .with_local_proxy(LocalProxyConfig::new().with_preferred_port(0))
            .with_system_proxy(system_proxy),
        fake.clone(),
    );
    (engine, fake)
}

/// The inbound tagged `tag` of the configuration the runtime runs.
fn inbound(fake: &FakeRuntime, tag: &str) -> Option<Value> {
    let config: Value = serde_json::from_str(&fake.config()?).unwrap();
    config["inbounds"]
        .as_array()?
        .iter()
        .find(|i| i["tag"] == tag)
        .cloned()
}

/// Binds 127.0.0.1:port for the test; another test may hold a well-known
/// port for an instant.
fn hold(port: u16) -> TcpListener {
    for _ in 0..50 {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return listener;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("port {port} stays taken");
}

fn reloads(fake: &FakeRuntime) -> usize {
    fake.calls()
        .iter()
        .filter(|c| matches!(c, Call::Reload(_)))
        .count()
}

/// Go: api TestLocalProxyMetadataAndCredentialAreSeparated. The state is
/// read at `new`: the routed credential before any apply (contract,
/// section 3), a node's once the profile has it.
#[tokio::test]
async fn metadata_and_credentials_are_separate_and_follow_the_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = standard(tmp.path(), false);

    let routed = engine.local_proxy_routed_credential().unwrap();
    assert_eq!(routed.kind, LocalProxyKind::Routed);
    assert_eq!(routed.node_id, "");
    assert_eq!(routed.listen, "127.0.0.1");
    assert_ne!(routed.port, 0);
    assert!(
        !routed.username.contains('-'),
        "routed user is a bare prefix"
    );
    let metadata = engine.local_proxy_metadata().unwrap();
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].kind, LocalProxyKind::Routed);
    let err = engine.local_proxy_credential(NODE_1).unwrap_err();
    assert_eq!(
        (err.code, err.field.as_deref()),
        (codes::NODE_NOT_FOUND, Some("node_id"))
    );
    let status = engine.status().local_proxy.expect("local proxy status");
    assert_eq!(
        (status.listen.as_str(), status.port, status.listening),
        ("127.0.0.1", routed.port, false)
    );
    assert_eq!(status.credentials_reset, None);

    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    let metadata = engine.local_proxy_metadata().unwrap();
    assert_eq!(
        metadata
            .iter()
            .map(|m| (m.kind, m.node_id.as_str()))
            .collect::<Vec<_>>(),
        [
            (LocalProxyKind::Node, NODE_1),
            (LocalProxyKind::Node, NODE_2),
            (LocalProxyKind::Routed, ""),
        ]
    );
    for entry in &metadata {
        assert_eq!(entry.port, routed.port);
        assert_eq!(entry.protocols, ["http", "socks5"]);
        assert!(entry.auth_required);
    }
    let json = serde_json::to_string(&metadata).unwrap();
    assert!(
        !json.contains("password") && !json.contains("username"),
        "metadata carries a secret"
    );
    let node = engine.local_proxy_credential(NODE_1).unwrap();
    assert_eq!(
        (node.kind, node.node_id.as_str()),
        (LocalProxyKind::Node, NODE_1)
    );
    assert!(
        node.username == format!("{}-{NODE_1}", routed.username),
        "node username"
    );
    assert!(node.password == routed.password, "password not shared");
    let err = engine.local_proxy_credential("").unwrap_err();
    assert_eq!(err.code, codes::NODE_NOT_FOUND, "not an implicit routed");

    // Started: the shared inbound, one user per node and the routed one.
    engine.start().await.unwrap();
    let shared = inbound(&fake, LOCAL_PROXY_INBOUND_TAG).expect("shared inbound");
    assert_eq!(shared["listen"], "127.0.0.1");
    assert_eq!(shared["listen_port"], routed.port);
    assert_eq!(shared["users"].as_array().map(Vec::len), Some(3));
    assert!(engine.status().local_proxy.unwrap().listening);
    engine.stop().await.unwrap();
    assert!(!engine.status().local_proxy.unwrap().listening);
}

/// A port taken while stopped moves at start: LocalProxyEndpointChanged,
/// and status, metadata and the runtime follow.
#[tokio::test]
async fn start_moves_a_taken_shared_port() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = standard(tmp.path(), false);
    let before = engine.status().local_proxy.unwrap().port;
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    let _squatter = hold(before);
    let mut rx = engine.subscribe(&[EventKind::LocalProxyEndpointChanged]);
    engine.start().await.unwrap();
    let after = engine.status().local_proxy.unwrap().port;
    assert_ne!(after, before);
    match drain(&mut rx).as_slice() {
        [Event::LocalProxyEndpointChanged { listen, port, .. }] => {
            assert_eq!((listen.as_str(), *port), ("127.0.0.1", after));
        }
        other => panic!("{other:?}"),
    }
    assert!(engine
        .local_proxy_metadata()
        .unwrap()
        .iter()
        .all(|m| m.port == after));
    assert_eq!(
        inbound(&fake, LOCAL_PROXY_INBOUND_TAG).unwrap()["listen_port"],
        after
    );
}

/// A state file that cannot be used, or that others may read, gets new
/// credentials at `new`: `status.local_proxy.credentials_reset` says why
/// for the instance's lifetime; the file is private.
#[tokio::test]
async fn rebuilt_credentials_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(STATE_FILE);
    std::fs::write(&path, "{").unwrap();
    let (engine, _) = standard(tmp.path(), false);
    let first = engine.local_proxy_routed_credential().unwrap();
    assert_eq!(
        engine.status().local_proxy.unwrap().credentials_reset,
        Some(CredentialsResetReason::Corrupt)
    );
    let status = serde_json::to_value(engine.status()).unwrap();
    assert_eq!(status["local_proxy"]["credentials_reset"], "corrupt");
    // Still set later in the instance's life: nothing clears it.
    running(&engine).await;
    engine.stop().await.unwrap();
    assert_eq!(
        engine.status().local_proxy.unwrap().credentials_reset,
        Some(CredentialsResetReason::Corrupt)
    );
    drop(engine);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (engine, _) = standard(tmp.path(), false);
        let second = engine.local_proxy_routed_credential().unwrap();
        assert!(second.password != first.password, "readable secret kept");
        assert_eq!(second.port, first.port, "port kept");
        assert_eq!(
            engine.status().local_proxy.unwrap().credentials_reset,
            Some(CredentialsResetReason::InsecurePermissions)
        );
        let status = serde_json::to_value(engine.status()).unwrap();
        assert_eq!(
            status["local_proxy"]["credentials_reset"],
            "insecure_permissions"
        );
        assert_eq!(mode(&path), 0o600);
        drop(engine);
    }

    // Kept: nothing to report.
    let (engine, _) = standard(tmp.path(), false);
    assert_eq!(engine.status().local_proxy.unwrap().credentials_reset, None);
    let json = serde_json::to_value(engine.status()).unwrap();
    assert!(json["local_proxy"].get("credentials_reset").is_none());
}

/// Go: api TestLocalProxyAPIsReportDisabledCore, runtime
/// TestTUNOnlyCoreRejectsLocalProxyAPIs, TestSystemProxyUnavailableInTUNCore
/// and api TestSystemProxyUnavailableWithoutStateOrInTUNCore.
#[tokio::test]
async fn instances_without_a_local_proxy_refuse_its_calls() {
    let fake = Arc::new(FakeRuntime::default());
    let tun = Engine::with_runtime(
        EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent")
            .with_local_proxy(LocalProxyConfig::new())
            .with_system_proxy(true),
        fake.clone(),
    );
    running(&tun).await;
    let bare = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, "/nonexistent"),
        Arc::new(FakeRuntime::default()),
    );
    bare.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    for engine in [&tun, &bare] {
        let refused = [
            engine.local_proxy_metadata().map(|_| ()),
            engine.local_proxy_credential(NODE_1).map(|_| ()),
            engine.local_proxy_routed_credential().map(|_| ()),
            engine
                .probe_availability(ProbeAvailabilityRequest::new(
                    NODE_1,
                    "http://example.com/",
                    1000,
                ))
                .await
                .map(|_| ()),
        ];
        for (i, result) in refused.into_iter().enumerate() {
            assert_eq!(
                result.map_err(|e| (e.code, e.retryable)),
                Err((codes::LOCAL_PROXY_DISABLED, false)),
                "call {i}"
            );
        }
        let err = engine.set_system_proxy_listener(true).await.unwrap_err();
        assert_eq!(
            (err.code, err.retryable),
            (codes::SYSTEM_PROXY_UNAVAILABLE, false)
        );
        let status = engine.status();
        assert!(!status.system_proxy.available);
        assert_eq!(status.local_proxy, None);
    }
    // The TUN instance runs no listener of its own.
    let config: Value = serde_json::from_str(&fake.config().unwrap()).unwrap();
    assert!(config["inbounds"]
        .as_array()
        .into_iter()
        .flatten()
        .all(|i| i["tag"] != LOCAL_PROXY_INBOUND_TAG && i["tag"] != SYSTEM_PROXY_INBOUND_TAG));
}

/// Go: api TestSetSystemProxyToggleAndStatus. Idempotent; before a start
/// only the status changes, while running the runtime reloads.
#[tokio::test]
async fn system_proxy_listener_toggles() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = standard(tmp.path(), true);
    assert_eq!(
        serde_json::to_string(&engine.status().system_proxy).unwrap(),
        r#"{"available":true,"enabled":false,"listening":false}"#
    );
    let mut rx = engine.subscribe(&[EventKind::SystemProxyChanged]);

    let on = engine.set_system_proxy_listener(true).await.unwrap();
    assert!(on.available && on.enabled && !on.listening);
    assert_eq!(on.listen, "127.0.0.1");
    assert_eq!(on.protocols, ["http", "socks5"]);
    assert_ne!(on.port, 0);
    assert_ne!(on.port, engine.status().local_proxy.unwrap().port);
    assert_eq!(engine.status().system_proxy, on);
    match drain(&mut rx).as_slice() {
        [Event::SystemProxyChanged {
            revision, message, ..
        }] => assert_eq!((revision.as_str(), message.as_str()), ("", "enabled")),
        other => panic!("{other:?}"),
    }
    assert_eq!(engine.set_system_proxy_listener(true).await.unwrap(), on);
    assert!(drain(&mut rx).is_empty(), "idempotent");

    running(&engine).await;
    let listening = engine.status().system_proxy;
    assert!(listening.listening);
    assert_eq!(
        inbound(&fake, SYSTEM_PROXY_INBOUND_TAG).unwrap()["listen_port"],
        listening.port
    );

    // While running the listener alone comes and goes: an inbounds-only
    // reload each time (nothing else built again), the local proxy's stays.
    let listening = |fake: &FakeRuntime, tag: &str| fake.inbounds().iter().any(|t| t == tag);
    assert!(listening(&fake, SYSTEM_PROXY_INBOUND_TAG));
    let connection = |id: u64, inbound: &str| crate::runtime::RuntimeConnection {
        id,
        inbound: inbound.into(),
        chain: vec!["direct".into()],
        network: "tcp".into(),
        destination: "192.0.2.10:443".into(),
        upload_bytes: 0,
        download_bytes: 0,
        started: std::time::SystemTime::now(),
    };
    fake.set_connections(vec![
        connection(1, SYSTEM_PROXY_INBOUND_TAG),
        connection(2, LOCAL_PROXY_INBOUND_TAG),
    ]);
    let off = engine.set_system_proxy_listener(false).await.unwrap();
    // Closing it ends its own connections only.
    let left: Vec<u64> = fake
        .connections()
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(left, [2]);
    assert_eq!(
        serde_json::to_string(&off).unwrap(),
        r#"{"available":true,"enabled":false,"listening":false}"#
    );
    assert_eq!(engine.inner.toggle_paths(), ["inbounds_only"]);
    assert!(!listening(&fake, SYSTEM_PROXY_INBOUND_TAG), "closed");
    assert!(listening(&fake, LOCAL_PROXY_INBOUND_TAG), "untouched");
    match drain(&mut rx).as_slice() {
        [Event::SystemProxyChanged {
            revision, message, ..
        }] => assert_eq!((revision.as_str(), message.as_str()), (R1, "disabled")),
        other => panic!("{other:?}"),
    }

    // The runtime refuses it: still off, SYSTEM_PROXY_START_FAILED.
    fake.fail_next(Op::Reload, RuntimeError::new("config", "refused"));
    let err = engine.set_system_proxy_listener(true).await.unwrap_err();
    assert_eq!(
        (err.code, err.retryable),
        (codes::SYSTEM_PROXY_START_FAILED, true)
    );
    assert!(!engine.status().system_proxy.enabled);
    assert!(!listening(&fake, SYSTEM_PROXY_INBOUND_TAG));
    assert!(drain(&mut rx).is_empty());

    let on = engine.set_system_proxy_listener(true).await.unwrap();
    assert!(on.enabled && on.listening);
    assert_eq!(
        engine.inner.toggle_paths(),
        ["inbounds_only", "inbounds_only"],
        "each toggle reloads the inbounds alone"
    );
    assert!(listening(&fake, SYSTEM_PROXY_INBOUND_TAG), "open");
    assert!(listening(&fake, LOCAL_PROXY_INBOUND_TAG));
    // The translation the next reload or start uses has it.
    let json = engine
        .inner
        .live()
        .applied
        .as_ref()
        .unwrap()
        .translation
        .json
        .clone();
    let config: Value = serde_json::from_str(&json).unwrap();
    assert!(config["inbounds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["tag"] == SYSTEM_PROXY_INBOUND_TAG && i["listen_port"] == on.port));
    assert_eq!(engine.status().state, EngineState::Running);

    // stop closes it with everything else, its connections included.
    fake.set_connections(vec![connection(7, SYSTEM_PROXY_INBOUND_TAG)]);
    engine.stop().await.unwrap();
    assert!(fake.inbounds().is_empty());
    assert!(fake.calls().iter().any(|c| matches!(c, Call::Stop)));
}

/// Go: TestSystemProxyStartFallsBackWhenPortTaken.
#[tokio::test]
async fn system_proxy_start_falls_back_when_port_taken() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = standard(tmp.path(), true);
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    let first = engine.set_system_proxy_listener(true).await.unwrap();
    let _squatter = hold(first.port);
    engine.start().await.unwrap();
    let got = engine.status().system_proxy;
    assert!(got.listening);
    assert_ne!(got.port, first.port);
    assert_ne!(got.port, engine.status().local_proxy.unwrap().port);
    assert_eq!(
        inbound(&fake, SYSTEM_PROXY_INBOUND_TAG).unwrap()["listen_port"],
        got.port
    );
}

fn local_proxy_open(fake: &FakeRuntime) -> bool {
    fake.inbounds().iter().any(|t| t == LOCAL_PROXY_INBOUND_TAG)
}

fn local_proxy_unavailable() -> EngineState {
    EngineState::Degraded {
        reasons: vec![DegradedReason::LocalProxyUnavailable],
    }
}

/// Contract 4.6: a shared listener that cannot be opened leaves the run
/// without it, `Degraded{LocalProxyUnavailable}`, and is retried with
/// backoff (1 s, then 2 s, …) until it opens in place.
#[tokio::test(start_paused = true)]
async fn an_unavailable_local_proxy_degrades_and_is_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = standard(tmp.path(), false);
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    // The start's check, then the first retry.
    engine.inner.refuse_local_proxy(2);
    engine.start().await.unwrap();
    assert!(!local_proxy_open(&fake));
    assert!(inbound(&fake, LOCAL_PROXY_INBOUND_TAG).is_none());
    assert_eq!(engine.status().state, local_proxy_unavailable());

    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!local_proxy_open(&fake), "the first retry was refused");
    assert_eq!(engine.status().state, local_proxy_unavailable());

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(local_proxy_open(&fake), "the second retry opens it");
    assert_eq!(engine.status().state, EngineState::Running);
    assert_eq!(
        engine.inner.toggle_paths(),
        ["inbounds_only"],
        "opened by an inbounds-only reload"
    );

    // Opened: no further retries.
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(reloads(&fake), 1);
}

/// A start that fails with the shared listener in it (its port taken
/// since it was checked) starts again without it, degraded, and retries.
#[tokio::test(start_paused = true)]
async fn a_start_failing_with_the_local_proxy_goes_on_without_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = standard(tmp.path(), false);
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    fake.fail_next(Op::Start, RuntimeError::new("config", "address in use"));
    engine.start().await.unwrap();
    let starts: Vec<_> = fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Start(config) => Some(config),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(starts[0].contains(LOCAL_PROXY_INBOUND_TAG));
    assert!(!starts[1].contains(LOCAL_PROXY_INBOUND_TAG));
    assert_eq!(engine.status().state, local_proxy_unavailable());

    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(local_proxy_open(&fake));
    assert_eq!(engine.status().state, EngineState::Running);
}

/// A stop ends the retries with the run; the next start tries the
/// listener afresh.
#[tokio::test(start_paused = true)]
async fn stop_cancels_the_local_proxy_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = standard(tmp.path(), false);
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    engine.inner.refuse_local_proxy(1);
    engine.start().await.unwrap();
    assert_eq!(engine.status().state, local_proxy_unavailable());
    engine.stop().await.unwrap();
    assert_eq!(engine.status().state, EngineState::Configured);

    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(reloads(&fake), 0);

    engine.start().await.unwrap();
    assert!(local_proxy_open(&fake));
    assert_eq!(engine.status().state, EngineState::Running);
}
