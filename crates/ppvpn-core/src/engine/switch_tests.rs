use std::sync::Arc;

use serde_json::{json, Value};

use super::super::lifecycle_tests::{profile, profile_with, running, R1, R2};
use super::super::tun::HostIpv6;
use super::*;
use crate::config::{EngineConfig, Platform, Role};
use crate::engine::Engine;
use crate::request::ApplyRequest;
use crate::runtime::fake::{Call, FakeRuntime, Op};
use crate::runtime::RuntimeError;
use crate::status::EngineState;

fn local_proxy(port: u16, password: &str, users: &[&str]) -> Value {
    json!({
        "type": "mixed",
        "tag": LOCAL_PROXY_INBOUND_TAG,
        "listen": "127.0.0.1",
        "listen_port": port,
        "users": users.iter().map(|u| json!({ "username": u, "password": password })).collect::<Vec<_>>(),
    })
}

/// Random bytes, hex: tests never hard-code a secret.
fn secret() -> String {
    let mut buf = [0u8; 8];
    getrandom::fill(&mut buf).unwrap();
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn tun(mtu: u32) -> Value {
    json!({ "type": "tun", "tag": TUN_INBOUND_TAG, "mtu": mtu, "auto_route": true })
}

fn config(inbounds: Vec<Value>, route: Value) -> String {
    json!({ "inbounds": inbounds, "route": route }).to_string()
}

/// Go: internal/runtime TestFullRestartReasons. Only listener changes stop
/// the engine; the reasons are Go's words.
#[test]
fn full_restart_reasons_are_a_whitelist() {
    // Two passwords, random (never a literal credential).
    let (s, x) = (secret(), secret());
    let route = json!({ "auto_detect_interface": true });
    let running = config(
        vec![local_proxy(7890, &s, &["p-a", "p"]), tun(9000)],
        route.clone(),
    );
    let cases = [
        (
            "same",
            config(
                vec![local_proxy(7890, &s, &["p-a", "p"]), tun(9000)],
                route.clone(),
            ),
            "",
        ),
        (
            "local proxy users only",
            config(
                vec![local_proxy(7890, &s, &["p-b", "p"]), tun(9000)],
                route.clone(),
            ),
            "",
        ),
        (
            "system proxy added",
            config(
                vec![
                    local_proxy(7890, &s, &["p-a", "p"]),
                    tun(9000),
                    json!({ "type": "mixed", "tag": SYSTEM_PROXY_INBOUND_TAG, "listen": "127.0.0.1", "listen_port": 7891 }),
                ],
                route.clone(),
            ),
            "",
        ),
        (
            "rules and nodes",
            config(
                vec![local_proxy(7890, &s, &["p-a", "p"]), tun(9000)],
                json!({ "auto_detect_interface": true, "final": "other" }),
            ),
            "",
        ),
        (
            "tun options",
            config(
                vec![local_proxy(7890, &s, &["p-a", "p"]), tun(1500)],
                route.clone(),
            ),
            "tun options changed",
        ),
        (
            "local proxy port",
            config(
                vec![local_proxy(7899, &s, &["p-a", "p"]), tun(9000)],
                route.clone(),
            ),
            "local proxy listener changed",
        ),
        // Users, replaced in place.
        (
            "local proxy password",
            config(
                vec![local_proxy(7890, &x, &["p-a", "p"]), tun(9000)],
                route.clone(),
            ),
            "",
        ),
        (
            "tun removed",
            config(vec![local_proxy(7890, &s, &["p-a", "p"])], route.clone()),
            "inbound tun added or removed",
        ),
        (
            "interface options",
            config(
                vec![local_proxy(7890, &s, &["p-a", "p"]), tun(9000)],
                json!({}),
            ),
            "interface options changed",
        ),
    ];
    for (name, next, want) in cases {
        assert_eq!(
            full_restart_reasons(&running, &next).join("; "),
            want,
            "{name}"
        );
    }
}

fn tun_instance() -> (Engine, Arc<FakeRuntime>) {
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent"),
        fake.clone(),
    );
    engine.inner.set_host_ipv6_probe(|| HostIpv6 {
        available: true,
        route: Ok(true),
    });
    (engine, fake)
}

/// The profile with its first ingress on another entry IP: the TUN
/// excludes it from the tunnel.
fn moved_entry(revision: &str) -> Vec<u8> {
    profile_with(revision, |p| {
        p["nodes"][0]["ingresses"][0]["endpoint"]["ip"] = "9.9.9.9".into();
    })
}

fn count(fake: &FakeRuntime, f: impl Fn(&Call) -> bool) -> usize {
    fake.calls().iter().filter(|c| f(c)).count()
}

/// #150: a new entry IP changes the TUN's excluded routes, which a reload
/// would keep: the apply stops and starts with the new configuration.
#[tokio::test]
async fn a_changed_tun_restarts_instead_of_reloading() {
    let (engine, fake) = tun_instance();
    running(&engine).await;
    let result = engine
        .apply(ApplyRequest::new(moved_entry(R2)))
        .await
        .unwrap();
    assert_eq!(
        result.switch,
        Some(SwitchKind::FullRestart {
            reasons: vec!["tun options changed".into()]
        })
    );
    assert_eq!(count(&fake, |c| matches!(c, Call::Reload(_))), 0);
    assert_eq!(count(&fake, |c| matches!(c, Call::Stop)), 1);
    assert_eq!(count(&fake, |c| matches!(c, Call::Start(_))), 2);
    assert!(fake.config().unwrap().contains("9.9.9.9"));
    assert_eq!(engine.status().state, EngineState::Running);
}

/// Only what a reload takes (a new revision, outbounds, users): a reload.
#[tokio::test]
async fn an_unchanged_listener_set_reloads() {
    let (engine, fake) = tun_instance();
    running(&engine).await;
    let result = engine.apply(ApplyRequest::new(profile(R2))).await.unwrap();
    assert_eq!(result.switch, Some(SwitchKind::KernelSwitch));
    assert_eq!(count(&fake, |c| matches!(c, Call::Reload(_))), 1);
    assert_eq!(count(&fake, |c| matches!(c, Call::Stop)), 0);
}

/// A restart whose new configuration does not start puts the running one
/// back: the apply fails and the instance runs as before.
#[tokio::test]
async fn a_failed_restart_puts_the_running_configuration_back() {
    let (engine, fake) = tun_instance();
    running(&engine).await;
    let before = fake.config().unwrap();
    fake.fail_next(Op::Start, RuntimeError::new("config", "refused"));
    engine
        .apply(ApplyRequest::new(moved_entry(R2)))
        .await
        .unwrap_err();
    assert_eq!(count(&fake, |c| matches!(c, Call::Start(_))), 3);
    assert_eq!(fake.config().unwrap(), before);
    assert_eq!(engine.status().state, EngineState::Running);
    assert_eq!(engine.status().revision.as_deref(), Some(R1));
}

/// A full restart while the local proxy listener is left out keeps it out
/// and retries it in the new run (the old run's retry ends with it).
#[tokio::test(start_paused = true)]
async fn a_full_restart_retries_a_left_out_local_proxy() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, tmp.path())
            .with_local_proxy(crate::config::LocalProxyConfig::new().with_preferred_port(0)),
        fake.clone(),
    );
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    engine.inner.refuse_local_proxy(1);
    engine.start().await.unwrap();
    let left_out = EngineState::Degraded {
        reasons: vec![crate::status::DegradedReason::LocalProxyUnavailable],
    };
    assert_eq!(engine.status().state, left_out);

    // Another listener appears: a full restart, still without the local
    // proxy listener.
    let running = engine
        .inner
        .live()
        .applied
        .as_ref()
        .unwrap()
        .translation
        .clone();
    let mut next = running.clone();
    let mut config: Value = serde_json::from_str(&next.json).unwrap();
    // Left out, the local proxy was the only listener: none is left.
    let extra = json!({ "type": "mixed", "tag": "extra", "listen": "127.0.0.1", "listen_port": 1 });
    match config["inbounds"].as_array_mut() {
        Some(inbounds) => inbounds.push(extra),
        None => config["inbounds"] = json!([extra]),
    }
    next.json = config.to_string();
    let switch = engine.inner.switch_to(&running, &next).await.unwrap();
    assert!(matches!(switch, SwitchKind::FullRestart { .. }));
    assert_eq!(engine.status().state, left_out);
    assert!(!fake.inbounds().iter().any(|t| t == LOCAL_PROXY_INBOUND_TAG));

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(fake.inbounds().iter().any(|t| t == LOCAL_PROXY_INBOUND_TAG));
    assert_eq!(engine.status().state, EngineState::Running);
}
