use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::super::lifecycle_tests::running;
use super::*;
use crate::config::EngineConfig;
use crate::engine::Engine;
use crate::runtime::fake::FakeRuntime;
use crate::status::{DegradedReason, EngineState};

fn guarded(role: Role, platform: Platform) -> Engine {
    let engine = Engine::with_runtime(
        EngineConfig::new(role, platform, "/nonexistent"),
        Arc::new(FakeRuntime::default()),
    );
    engine
        .inner
        .set_host_ipv6_probe(|| super::super::tun::HostIpv6 {
            available: true,
            route: Ok(true),
        });
    engine.inner.routing.enabled.store(true, Ordering::SeqCst);
    engine
}

#[test]
fn verdicts_are_the_engine_signals() {
    assert_eq!(signal(TunRoutingStatus::Ok), None);
    assert_eq!(
        signal(TunRoutingStatus::Restoring {
            missing: vec!["rule".into()]
        }),
        Some(TunRoutingSignal::Restoring)
    );
    assert_eq!(
        signal(TunRoutingStatus::Restored {
            missing: vec!["rule".into()]
        }),
        Some(TunRoutingSignal::Restored {
            missing: vec!["rule".into()]
        })
    );
    assert_eq!(
        signal(TunRoutingStatus::Unguarded {
            error: "no netlink".into()
        }),
        Some(TunRoutingSignal::Unguarded)
    );
    assert_eq!(
        signal(TunRoutingStatus::Broken {
            missing: vec!["route".into()],
            error: "EPERM".into()
        }),
        Some(TunRoutingSignal::Broken {
            missing: vec!["route".into()],
            error: "EPERM".into()
        })
    );
}

/// The Linux desktop TUN is guarded from its start to its stop. The fake
/// runtime installs no routing, so the real guard finds nothing to keep
/// and says so: `Degraded{TunRoutingUnguarded}`, through the wiring.
#[tokio::test]
async fn the_guard_runs_with_the_linux_tun() {
    let tun = guarded(Role::Tun, Platform::Linux);
    running(&tun).await;
    assert_eq!(tun.inner.guard_running(), cfg!(target_os = "linux"));
    if cfg!(target_os = "linux") {
        let unguarded = EngineState::Degraded {
            reasons: vec![DegradedReason::TunRoutingUnguarded],
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while tun.status().state != unguarded {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the guard's verdict reaches the state");
    }
    tun.stop().await.unwrap();
    assert!(!tun.inner.guard_running());

    let standard = guarded(Role::Standard, Platform::Linux);
    running(&standard).await;
    assert!(!standard.inner.guard_running());
}

/// A stop that fails leaves the TUN running: it stays guarded.
#[tokio::test]
async fn a_failed_stop_keeps_the_tun_guarded() {
    let fake = Arc::new(FakeRuntime::default());
    let tun = Engine::with_runtime(
        EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent"),
        fake.clone(),
    );
    tun.inner
        .set_host_ipv6_probe(|| super::super::tun::HostIpv6 {
            available: true,
            route: Ok(true),
        });
    tun.inner.routing.enabled.store(true, Ordering::SeqCst);
    running(&tun).await;
    fake.fail_next(
        crate::runtime::fake::Op::Stop,
        crate::runtime::RuntimeError::new("io", "busy"),
    );
    tun.stop().await.unwrap_err();
    assert_eq!(tun.inner.guard_running(), cfg!(target_os = "linux"));
    tun.stop().await.unwrap();
    assert!(!tun.inner.guard_running());
}

/// macOS: sail tells that another program changed the TUN's routing
/// (`SystemChanged`). sail does not put it back: `TunRoutingBroken` and
/// Fatal, the host rebuilds the instance. (The Engine takes it on any
/// platform sail tells it on; the TUN here is the Linux one, unguarded.)
#[tokio::test]
async fn a_change_by_another_program_breaks_the_tun_routing() {
    use super::super::lifecycle_tests::next_event;
    use crate::event::{Event, EventKind};
    use crate::status::FatalReason;
    let fake = Arc::new(FakeRuntime::default());
    let tun = Engine::with_runtime(
        EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent"),
        fake.clone(),
    );
    tun.inner
        .set_host_ipv6_probe(|| super::super::tun::HostIpv6 {
            available: true,
            route: Ok(true),
        });
    running(&tun).await;
    let mut rx = tun.subscribe(&[EventKind::TunRoutingBroken]);
    let resource = "route 128.0.0.0/1 into utun9: 128.0.0.0/2 on utun4 wins";
    fake.system_changed("route", resource);

    match next_event(&mut rx).await {
        Event::TunRoutingBroken { missing, error, .. } => {
            assert_eq!(missing, [resource]);
            assert_eq!(error, "route changed by another program");
        }
        other => panic!("not TunRoutingBroken: {other:?}"),
    }
    assert_eq!(
        tun.status().state,
        EngineState::Fatal {
            reason: FatalReason::TunRoutingBroken {
                missing: vec![resource.into()]
            }
        }
    );
    let json = serde_json::to_value(tun.status()).unwrap();
    assert_eq!(json["reason"]["kind"], "tun_routing_broken");
}
