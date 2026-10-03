use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::super::lifecycle_tests::{drain, running};
use super::super::tun::HostIpv6;
use super::*;
use crate::config::{EngineConfig, Platform, Role};
use crate::event::{Event, EventKind};
use crate::runtime::fake::FakeRuntime;
use crate::status::{DegradedReason, EngineState};
use crate::Engine;

fn instance(role: Role) -> (Engine, Arc<FakeRuntime>, Arc<AtomicUsize>) {
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(role, Platform::Linux, "/nonexistent"),
        fake.clone(),
    );
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    engine.inner.set_host_ipv6_probe(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        HostIpv6 {
            available: true,
            route: Ok(true),
        }
    });
    (engine, fake, asked)
}

fn wifi() -> NetworkSnapshot {
    NetworkSnapshot {
        interface: Some("wlan0".into()),
        index: Some(3),
        addresses: vec!["192.168.50.22/24".into()],
        ..NetworkSnapshot::default()
    }
}

fn wired() -> NetworkSnapshot {
    NetworkSnapshot {
        interface: Some("eth0".into()),
        index: Some(2),
        addresses: vec!["192.168.60.5/24".into()],
        ..NetworkSnapshot::default()
    }
}

fn offline() -> NetworkSnapshot {
    NetworkSnapshot {
        offline: true,
        ..NetworkSnapshot::default()
    }
}

/// The network events seen: Some((name, index)) or None for offline.
fn changes(events: &[Event]) -> Vec<Option<(String, u32)>> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::NetworkChanged {
                has_default_interface,
                interface_name,
                interface_index,
                ..
            } => Some(has_default_interface.then(|| (interface_name.clone(), *interface_index))),
            _ => None,
        })
        .collect()
}

/// Lets the watcher take what the fake published.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
}

fn no_default_interface(engine: &Engine) -> bool {
    matches!(
        engine.status().state,
        EngineState::Degraded { ref reasons } if reasons.contains(&DegradedReason::NoDefaultInterface)
    )
}

// Go: TestDefaultInterfaceChangeEmitsNetworkChanged.
#[tokio::test]
async fn every_change_of_the_running_engine_is_network_changed() {
    let (engine, fake, _) = instance(Role::Standard);
    running(&engine).await;
    let mut rx = engine.subscribe(&[EventKind::NetworkChanged]);
    fake.change_network(wifi(), "default_interface");
    settle().await;
    fake.change_network(offline(), "state");
    settle().await;
    assert!(no_default_interface(&engine), "offline is degraded");
    fake.change_network(wired(), "default_interface");
    settle().await;
    assert!(!no_default_interface(&engine), "back online");
    assert_eq!(
        changes(&drain(&mut rx)),
        vec![Some(("wlan0".into(), 3)), None, Some(("eth0".into(), 2))]
    );
}

/// The watch keeps the latest change only: offline and back between two
/// reads shows as both steps, from the change's `old`.
#[tokio::test]
async fn a_missed_step_is_replayed_from_the_changes_old() {
    let (engine, _fake, _) = instance(Role::Standard);
    running(&engine).await;
    let mut rx = engine.subscribe(&[EventKind::NetworkChanged]);
    engine.inner.on_network_change(NetworkChange {
        generation: 1,
        change: "interface_changed".into(),
        reason: "default_interface".into(),
        old: NetworkSnapshot::default(),
        new: wifi(),
    });
    // Generation 2 (wifi → offline) was not seen; 3 is offline → wired.
    engine.inner.on_network_change(NetworkChange {
        generation: 3,
        change: "restored".into(),
        reason: "default_interface".into(),
        old: offline(),
        new: wired(),
    });
    assert_eq!(
        changes(&drain(&mut rx)),
        vec![Some(("wlan0".into(), 3)), None, Some(("eth0".into(), 2))]
    );
    // A jump whose `old` is what was last seen replays nothing.
    engine.inner.on_network_change(NetworkChange {
        generation: 7,
        change: "interface_changed".into(),
        reason: "state".into(),
        old: wired(),
        new: wifi(),
    });
    assert_eq!(changes(&drain(&mut rx)), vec![Some(("wlan0".into(), 3))]);
}

#[tokio::test]
async fn a_start_reads_the_network_without_reporting_a_change() {
    let (engine, fake, _) = instance(Role::Standard);
    fake.change_network(offline(), "state");
    let mut rx = engine.subscribe(&[EventKind::NetworkChanged]);
    running(&engine).await;
    settle().await;
    assert!(no_default_interface(&engine), "started offline");
    assert!(changes(&drain(&mut rx)).is_empty());
}

// Go: TestReprobeDebouncesBursts.
#[tokio::test(start_paused = true)]
async fn a_burst_of_changes_reprobes_once_after_the_delay() {
    let (engine, fake, asked) = instance(Role::Tun);
    running(&engine).await;
    let before = asked.load(Ordering::SeqCst);
    for snapshot in [wifi(), wired(), wifi()] {
        fake.change_network(snapshot, "default_interface");
        settle().await;
    }
    assert_eq!(asked.load(Ordering::SeqCst), before, "not before the delay");
    tokio::time::sleep(REPROBE_DELAY + Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(
        asked.load(Ordering::SeqCst),
        before + 1,
        "once for the burst"
    );
}

// Go: TestStopCancelsPendingReprobe.
#[tokio::test(start_paused = true)]
async fn stop_cancels_a_pending_reprobe() {
    let (engine, fake, asked) = instance(Role::Tun);
    running(&engine).await;
    let before = asked.load(Ordering::SeqCst);
    fake.change_network(wifi(), "default_interface");
    settle().await;
    engine.stop().await.unwrap();
    tokio::time::sleep(REPROBE_DELAY * 2).await;
    settle().await;
    assert_eq!(asked.load(Ordering::SeqCst), before);
}

#[tokio::test(start_paused = true)]
async fn a_standard_instance_never_reprobes() {
    let (engine, fake, asked) = instance(Role::Standard);
    running(&engine).await;
    fake.change_network(wifi(), "default_interface");
    tokio::time::sleep(REPROBE_DELAY * 2).await;
    settle().await;
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

/// A timer that fired leaves the cancellable slot: a change after it can
/// no longer abort a re-probe under way (its reload, say), only queue one.
#[tokio::test(start_paused = true)]
async fn a_fired_reprobe_is_not_cancelled_by_the_next_change() {
    let (engine, fake, asked) = instance(Role::Tun);
    running(&engine).await;
    let before = asked.load(Ordering::SeqCst);
    fake.change_network(wifi(), "default_interface");
    settle().await;
    assert!(engine.inner.network.track().timer.is_some(), "armed");
    tokio::time::sleep(REPROBE_DELAY + Duration::from_millis(100)).await;
    settle().await;
    assert!(
        engine.inner.network.track().timer.is_none(),
        "fired, left the slot"
    );
    fake.change_network(wired(), "default_interface");
    settle().await;
    tokio::time::sleep(REPROBE_DELAY + Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(asked.load(Ordering::SeqCst), before + 2, "both ran");
}

/// Offline is sail's report while it runs: after a stop the network is
/// unknown, not offline.
#[tokio::test]
async fn stop_forgets_offline() {
    let (engine, fake, _) = instance(Role::Standard);
    running(&engine).await;
    fake.change_network(offline(), "state");
    settle().await;
    assert!(no_default_interface(&engine));
    engine.stop().await.unwrap();
    assert!(!engine.inner.live().offline);
    // A change from the stopped run that arrives late changes nothing.
    engine.inner.on_network_change(NetworkChange {
        generation: 9,
        change: "offline".into(),
        reason: "state".into(),
        old: wifi(),
        new: offline(),
    });
    assert!(!engine.inner.live().offline);
}
