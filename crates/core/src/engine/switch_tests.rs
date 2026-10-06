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
    // The start's check, then the restart's.
    engine.inner.refuse_local_proxy(2);
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
    // A change the runtime takes only by a restart (it says so).
    config["route"]["final"] = "direct".into();
    next.json = config.to_string();
    fake.fail_next(
        Op::Reload,
        RuntimeError::new("needs_restart", "only a start sets that up"),
    );
    let build = || -> Result<Translation, Error> { Ok(next.clone()) };
    let switched = engine
        .inner
        .switch_to(&running, next.clone(), &build)
        .await
        .unwrap();
    assert!(matches!(switched.kind, SwitchKind::FullRestart { .. }));
    assert_eq!(engine.status().state, left_out);
    assert!(!fake.inbounds().iter().any(|t| t == LOCAL_PROXY_INBOUND_TAG));

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(fake.inbounds().iter().any(|t| t == LOCAL_PROXY_INBOUND_TAG));
    assert_eq!(engine.status().state, EngineState::Running);
}

fn change(generation: u64, kind: &str, offline: bool) -> crate::runtime::NetworkChange {
    let wired = crate::runtime::NetworkSnapshot {
        interface: Some("eth0".into()),
        index: Some(2),
        ..Default::default()
    };
    let none = crate::runtime::NetworkSnapshot {
        offline: true,
        ..Default::default()
    };
    let (old, new) = if offline {
        (wired, none)
    } else {
        (none, wired)
    };
    crate::runtime::NetworkChange {
        generation,
        change: kind.into(),
        reason: "state".into(),
        old,
        new,
    }
}

/// While a restart replaces the run, a late change of the old run is not
/// taken: the new run reads the network when it starts.
#[tokio::test]
async fn a_late_network_change_during_a_restart_is_ignored() {
    let (engine, _fake) = tun_instance();
    running(&engine).await;
    engine.inner.live().restarting = true;
    engine.inner.on_network_change(change(1, "offline", true));
    engine.inner.live().restarting = false;
    assert_eq!(engine.status().state, EngineState::Running);
}

/// A re-probe waiting when a restart stops the old run is armed again in
/// the new one.
#[tokio::test(start_paused = true)]
async fn a_restart_arms_a_waiting_reprobe_again() {
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent"),
        fake.clone(),
    );
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = asked.clone();
    engine.inner.set_host_ipv6_probe(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        HostIpv6 {
            available: true,
            route: Ok(true),
        }
    });
    running(&engine).await;
    // A change arms the re-probe; the restart comes before it fires.
    engine.inner.on_network_change(change(1, "restored", false));
    engine
        .apply(ApplyRequest::new(moved_entry(R2)))
        .await
        .unwrap();
    let before = asked.load(std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    assert_eq!(
        asked.load(std::sync::atomic::Ordering::SeqCst),
        before + 1,
        "re-probed in the new run"
    );
}

/// A restart checks the listeners' ports again once the old ones are
/// closed, as a start does: a shared port taken meanwhile moves.
#[tokio::test]
async fn a_restart_checks_the_listener_ports_again() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, tmp.path())
            .with_local_proxy(crate::config::LocalProxyConfig::new().with_preferred_port(0)),
        fake.clone(),
    );
    running(&engine).await;
    let before = engine.status().local_proxy.unwrap().port;
    let _squatter = std::net::TcpListener::bind(("127.0.0.1", before)).unwrap();
    let (profile, mode, selected, pins, running) = {
        let live = engine.inner.live();
        let a = live.applied.as_ref().unwrap();
        (
            a.profile.clone(),
            a.mode,
            a.selected.clone(),
            a.pins.clone(),
            a.translation.clone(),
        )
    };
    // The runtime takes the change only by a restart (it says so).
    fake.fail_next(
        Op::Reload,
        RuntimeError::new("needs_restart", "only a start sets that up"),
    );
    let build = || -> Result<Translation, Error> {
        let mut t =
            crate::translate::translate(&profile, &engine.inner.options(mode, &selected, &pins))?;
        let mut config: Value = serde_json::from_str(&t.json).unwrap();
        config["route"]["final"] = "direct".into();
        t.json = config.to_string();
        Ok(t)
    };
    let next = build().unwrap();
    let switched = engine
        .inner
        .switch_to(&running, next, &build)
        .await
        .unwrap();
    let now_running = switched.translation;
    assert!(matches!(switched.kind, SwitchKind::FullRestart { .. }));
    let after = engine.status().local_proxy.unwrap().port;
    assert_ne!(after, before);
    assert!(now_running
        .json
        .contains(&format!("\"listen_port\":{after}")));
}

/// Every reload switch is `KernelSwitched` (and Go's `kernel switched`
/// line), the apply's included; a full restart is not one.
#[tokio::test]
async fn a_hot_apply_sends_kernel_switched() {
    let (engine, _fake) = tun_instance();
    running(&engine).await;
    let mut rx = engine.subscribe(&[crate::event::EventKind::KernelSwitched]);
    engine.apply(ApplyRequest::new(profile(R2))).await.unwrap();
    let events = super::super::lifecycle_tests::drain(&mut rx);
    assert!(
        matches!(events.as_slice(), [crate::event::Event::KernelSwitched { revision, .. }] if revision == R2),
        "{events:?}"
    );
    engine
        .apply(ApplyRequest::new(moved_entry("2026-09-29T00:00:00Z#3")))
        .await
        .unwrap();
    assert!(super::super::lifecycle_tests::drain(&mut rx).is_empty());
}

/// A listener that moves (another port) is a reload, not a restart: sail
/// replaces it in place, only its own connections close, and the apply
/// says which listener changed (contract 4.1).
#[tokio::test]
async fn a_moved_listener_is_replaced_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, tmp.path())
            .with_local_proxy(crate::config::LocalProxyConfig::new().with_preferred_port(0))
            .with_system_proxy(true),
        fake.clone(),
    );
    running(&engine).await;
    engine.set_system_proxy_listener(true).await.unwrap();
    let connection = |id, inbound: &str| crate::runtime::RuntimeConnection {
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
        connection(1, LOCAL_PROXY_INBOUND_TAG),
        connection(2, SYSTEM_PROXY_INBOUND_TAG),
    ]);
    let running_translation = engine
        .inner
        .live()
        .applied
        .as_ref()
        .unwrap()
        .translation
        .clone();
    let mut next = running_translation.clone();
    let mut config: Value = serde_json::from_str(&next.json).unwrap();
    for inbound in config["inbounds"].as_array_mut().unwrap() {
        if inbound["tag"] == LOCAL_PROXY_INBOUND_TAG {
            inbound["listen_port"] = 1.into();
        }
    }
    next.json = config.to_string();
    let build = || -> Result<Translation, Error> { Ok(next.clone()) };
    let switched = engine
        .inner
        .switch_to(&running_translation, next.clone(), &build)
        .await
        .unwrap();
    assert_eq!(switched.kind, SwitchKind::KernelSwitch);
    assert_eq!(
        switched.listeners,
        [ListenerChange {
            tag: LOCAL_PROXY_INBOUND_TAG.into(),
            change: ListenerChangeKind::Replaced
        }]
    );
    assert_eq!(count(&fake, |c| matches!(c, Call::Stop)), 0);
    let left: Vec<u64> = crate::runtime::Runtime::connections(fake.as_ref())
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(left, [2], "the system proxy's connection is kept");
}

/// Go: internal/runtime TestApplyClosesConnectionsOfRemovedNodes and
/// TestCloseOnSwitchDecidesByRecordedNode. Taking a node away closes the
/// connections through it, through any of its ingresses, on the switch; a
/// connection on a node that stays, or through no node, keeps running.
/// Which node a connection is on is the one its chain recorded: the tag is
/// made from the node's id, so the remaining node keeps its tag although
/// its place in the profile moved. (On a desktop TUN a removed node's entry
/// IPs leave the TUN's excluded routes: a full restart, which closes all.)
#[tokio::test]
async fn a_switch_closes_the_connections_of_removed_nodes() {
    use super::super::lifecycle_tests::{drain, engine, NODE_1, NODE_2};
    let (engine, fake) = engine();
    running(&engine).await;
    let applied = |engine: &Engine| {
        engine
            .inner
            .live()
            .applied
            .as_ref()
            .unwrap()
            .translation
            .clone()
    };
    let before = applied(&engine);
    let (n1, n2) = (
        before.node_tags[NODE_1].clone(),
        before.node_tags[NODE_2].clone(),
    );
    let n1_backup = before.members[&n1][1].clone();
    let connection = |id, chain: &[&str]| crate::runtime::RuntimeConnection {
        id,
        inbound: SYSTEM_PROXY_INBOUND_TAG.into(),
        chain: chain.iter().map(|t| t.to_string()).collect(),
        network: "tcp".into(),
        destination: "192.0.2.10:443".into(),
        upload_bytes: 0,
        download_bytes: 0,
        started: std::time::SystemTime::now(),
    };
    fake.set_connections(vec![
        connection(1, &[crate::translate::SELECTED_TAG, &n1, &n1_backup]),
        connection(2, &[&n2]),
        connection(3, &["direct"]),
        connection(4, &[&n1_backup]),
    ]);
    let mut rx = engine.subscribe(&[crate::event::EventKind::KernelSwitched]);

    let without_node_1 = profile_with(R2, |v| {
        v["nodes"].as_array_mut().unwrap().remove(0);
        v["selection"]["default_node_id"] = NODE_2.into();
    });
    engine
        .apply(ApplyRequest::new(without_node_1))
        .await
        .unwrap();

    let events = drain(&mut rx);
    assert!(
        matches!(
            events.as_slice(),
            [crate::event::Event::KernelSwitched {
                closed_connections: 2,
                kept_connections: 2,
                ..
            }]
        ),
        "{events:?}"
    );
    let closed: Vec<u64> = fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::CloseConnection(id) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(closed, [1, 4]);
    assert_eq!(applied(&engine).node_tags[NODE_2], n2);
}

/// Go: internal/runtime TestApplyClosesConnectionsANewRuleRejects. sail's
/// recheck closes, in the reload, the connections the new rules reject:
/// they are among `KernelSwitched`'s closed, each once, also when its node
/// was removed too.
#[tokio::test]
async fn a_switch_counts_the_connections_a_new_rule_rejects() {
    use super::super::lifecycle_tests::{drain, engine, NODE_1, NODE_2};
    let (engine, fake) = engine();
    running(&engine).await;
    let before = engine
        .inner
        .live()
        .applied
        .as_ref()
        .unwrap()
        .translation
        .clone();
    let (n1, n2) = (
        before.node_tags[NODE_1].clone(),
        before.node_tags[NODE_2].clone(),
    );
    let connection = |id, chain: &str| crate::runtime::RuntimeConnection {
        id,
        inbound: SYSTEM_PROXY_INBOUND_TAG.into(),
        chain: vec![chain.into()],
        network: "tcp".into(),
        destination: "192.0.2.10:443".into(),
        upload_bytes: 0,
        download_bytes: 0,
        started: std::time::SystemTime::now(),
    };
    fake.set_connections(vec![
        connection(1, &n1),
        connection(2, &n2),
        connection(3, "direct"),
        connection(4, "direct"),
    ]);
    // 1: its node goes and a rule rejects it; 3: a rule rejects it.
    fake.recheck_next(vec![(1, Some(0)), (3, None)]);
    let mut rx = engine.subscribe(&[crate::event::EventKind::KernelSwitched]);

    let without_node_1 = profile_with(R2, |v| {
        v["nodes"].as_array_mut().unwrap().remove(0);
        v["selection"]["default_node_id"] = NODE_2.into();
    });
    engine
        .apply(ApplyRequest::new(without_node_1))
        .await
        .unwrap();

    let events = drain(&mut rx);
    assert!(
        matches!(
            events.as_slice(),
            [crate::event::Event::KernelSwitched {
                closed_connections: 2,
                kept_connections: 2,
                ..
            }]
        ),
        "{events:?}"
    );
    let left: Vec<u64> = crate::runtime::Runtime::connections(fake.as_ref())
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(left, [2, 4]);
}

/// After a full restart the first read is of the new run, measured as it
/// started (the apply reads the runtime once it switched).
#[tokio::test]
async fn the_first_read_after_a_full_restart_is_of_the_new_run() {
    use crate::runtime::RuntimeTraffic;
    let (engine, fake) = tun_instance();
    running(&engine).await;
    fake.set_traffic(RuntimeTraffic {
        upload_bytes: 8,
        download_bytes: 9,
    });
    let before = chrono::Utc::now();
    let result = engine
        .apply(ApplyRequest::new(moved_entry(R2)))
        .await
        .unwrap();
    assert!(
        matches!(result.switch, Some(SwitchKind::FullRestart { .. })),
        "{result:?}"
    );
    let traffic = engine.traffic();
    assert_eq!((traffic.upload_bytes, traffic.download_bytes), (8, 9));
    assert!(traffic.measured_at >= before, "{traffic:?}");
}
