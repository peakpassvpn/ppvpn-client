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
