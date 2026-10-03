//! The Engine's lifecycle on the fake runtime: apply, start, stop, the state
//! machine, its events and the status snapshot.

use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::Value;

use super::*;
use crate::config::Platform;
use crate::event::EventItem;
use crate::request::{ClearedPin, Pin, PinClearReason, SwitchKind};
use crate::runtime::fake::{Call, FakeRuntime, Op};
use crate::runtime::RuntimeState;
use crate::status::{DegradedReason, EngineState};
use crate::translate::{ingress_tag, node_tag, AUTO_SUFFIX, SELECTED_TAG};

pub(super) const NODE_1: &str = "3f2c9a1e-0000-4000-8000-000000000001-128";
pub(super) const NODE_2: &str = "3f2c9a1e-0000-4000-8000-000000000002-129";
pub(super) const R1: &str = "2026-09-29T00:00:00Z#1";
pub(super) const R2: &str = "2026-09-29T00:00:00Z#2";

/// The contract golden's profile at `revision`, changed by `edit`.
pub(super) fn profile_with(revision: &str, edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/golden/contract/profiles/base.json"
    );
    let mut value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    value["revision"] = revision.into();
    edit(&mut value);
    serde_json::to_vec(&value).unwrap()
}

pub(super) fn profile(revision: &str) -> Vec<u8> {
    profile_with(revision, |_| {})
}

pub(super) fn engine() -> (Engine, Arc<FakeRuntime>) {
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, "/nonexistent"),
        fake.clone(),
    );
    (engine, fake)
}

/// The events already sent.
pub(super) fn drain(rx: &mut EventReceiver) -> Vec<Event> {
    let mut events = Vec::new();
    while let Some(Some(item)) = rx.recv().now_or_never() {
        match item {
            EventItem::Event { event } => events.push(event),
            other => panic!("unexpected {other:?}"),
        }
    }
    events
}

/// The next event, waiting for the watcher.
pub(super) async fn next_event(rx: &mut EventReceiver) -> Event {
    match tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("an event in time")
    {
        Some(EventItem::Event { event }) => event,
        other => panic!("unexpected {other:?}"),
    }
}

pub(super) fn kinds(events: &[Event]) -> Vec<EventKind> {
    events.iter().map(Event::kind).collect()
}

pub(super) fn state_change(event: &Event) -> (EngineState, EngineState) {
    match event {
        Event::StateChanged {
            state, previous, ..
        } => (state.clone(), previous.clone()),
        other => panic!("not StateChanged: {other:?}"),
    }
}

pub(super) async fn running(engine: &Engine) {
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    engine.start().await.unwrap();
}

#[tokio::test]
async fn apply_start_stop_walk_the_state_machine() {
    let (engine, fake) = engine();
    let mut rx = engine.subscribe(EventKind::ALL);

    // D1: no profile, no event.
    let err = engine.start().await.unwrap_err();
    assert_eq!(
        (err.code, err.retryable),
        (codes::PROFILE_NOT_APPLIED, false)
    );
    assert!(drain(&mut rx).is_empty());
    assert_eq!(engine.status().state, EngineState::Stopped);

    let result = engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    assert!(result.applied);
    assert_eq!(result.revision, R1);
    assert_eq!(result.selected_node_id, NODE_1);
    assert_eq!(result.switch, None);
    let events = drain(&mut rx);
    assert_eq!(
        kinds(&events),
        [EventKind::StateChanged, EventKind::ProfileApplied]
    );
    assert_eq!(
        state_change(&events[0]),
        (EngineState::Configured, EngineState::Stopped)
    );
    assert!(fake.calls().is_empty(), "stopped: apply only keeps it");
    let status = engine.status();
    assert_eq!(status.state, EngineState::Configured);
    assert_eq!(status.revision.as_deref(), Some(R1));
    assert_eq!(status.routing_mode, Some(RoutingMode::Rules));
    assert_eq!(status.selected_node_id.as_deref(), Some(NODE_1));
    assert_eq!(status.node_count, 2);
    assert_eq!(status.selected_ingress, None);
    assert_eq!(status.nodes[0].ingresses[0].healthy, None);

    engine.start().await.unwrap();
    assert!(matches!(fake.calls().as_slice(), [Call::Start(_)]));
    let events = drain(&mut rx);
    assert_eq!(
        kinds(&events),
        [EventKind::StateChanged, EventKind::CoreStarted]
    );
    assert_eq!(
        state_change(&events[0]),
        (EngineState::Running, EngineState::Configured)
    );
    let status = engine.status();
    assert_eq!(status.state, EngineState::Running);
    assert_eq!(status.selected_ingress.unwrap().endpoint_key, "9001");
    let first = &status.nodes[0].ingresses;
    assert_eq!(
        (first[0].active, first[0].healthy, first[1].active),
        (true, Some(true), false)
    );
    // A single-ingress node has no failover health (as Go).
    assert_eq!(status.nodes[1].ingresses[0].healthy, None);

    engine.start().await.unwrap();
    assert!(drain(&mut rx).is_empty(), "start is idempotent");
    assert_eq!(fake.calls().len(), 1);

    engine.stop().await.unwrap();
    assert_eq!(fake.calls().last(), Some(&Call::Stop));
    let events = drain(&mut rx);
    assert_eq!(
        kinds(&events),
        [EventKind::StateChanged, EventKind::CoreStopped]
    );
    assert_eq!(
        state_change(&events[0]),
        (EngineState::Configured, EngineState::Running)
    );
    engine.stop().await.unwrap();
    assert!(drain(&mut rx).is_empty(), "stop is idempotent");
    let status = engine.status();
    assert_eq!(status.state, EngineState::Configured);
    assert_eq!(status.selected_ingress, None);
    assert_eq!(status.nodes[0].ingresses[0].healthy, None);
}

#[tokio::test]
async fn apply_dedupes_on_the_live_values() {
    let (engine, _) = engine();
    let mut rx = engine.subscribe(EventKind::ALL);
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    drain(&mut rx);

    let again = engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    assert!(!again.applied);
    assert!(drain(&mut rx).is_empty());

    // The default node named explicitly is the same selection.
    let same = ApplyRequest::new(profile(R1)).with_selected_node_id(NODE_1);
    assert!(!engine.apply(same).await.unwrap().applied);

    // The routing mode, the selection and the pins are each part of the key.
    let global = ApplyRequest::new(profile(R1)).with_routing_mode(RoutingMode::Global);
    assert!(engine.apply(global.clone()).await.unwrap().applied);
    assert_eq!(kinds(&drain(&mut rx)), [EventKind::ProfileApplied]);
    assert!(!engine.apply(global.clone()).await.unwrap().applied);
    assert_eq!(engine.status().routing_mode, Some(RoutingMode::Global));

    let second = global.clone().with_selected_node_id(NODE_2);
    assert!(engine.apply(second.clone()).await.unwrap().applied);
    assert!(!engine.apply(second).await.unwrap().applied);
    assert_eq!(engine.status().selected_node_id.as_deref(), Some(NODE_2));

    let pinned = global.with_pins(vec![Pin::new(NODE_1, "9002")]);
    assert!(engine.apply(pinned.clone()).await.unwrap().applied);
    assert!(!engine.apply(pinned).await.unwrap().applied);
    let status = engine.status();
    assert_eq!(status.selected_node_id.as_deref(), Some(NODE_1));
    assert_eq!(status.nodes[0].pinned_endpoint_key.as_deref(), Some("9002"));

    let r2 = ApplyRequest::new(profile(R2))
        .with_routing_mode(RoutingMode::Global)
        .with_pins(vec![Pin::new(NODE_1, "9002")]);
    assert!(engine.apply(r2).await.unwrap().applied);
    assert_eq!(engine.status().revision.as_deref(), Some(R2));
}

#[tokio::test]
async fn stale_selection_and_pins_are_reset_and_reported() {
    let (engine, _) = engine();
    let mut rx = engine.subscribe(EventKind::ALL);
    let request = ApplyRequest::new(profile(R1))
        .with_selected_node_id("gone")
        .with_pins(vec![
            Pin::new(NODE_1, "9999"),
            Pin::new("gone", "9001"),
            Pin::new(NODE_2, "9003"),
        ]);
    let result = engine.apply(request).await.unwrap();
    assert!(result.selection_reset);
    assert_eq!(result.selected_node_id, NODE_1);
    let cleared = |node: &str, key: &str, reason| ClearedPin {
        node_id: node.into(),
        endpoint_key: key.into(),
        reason,
    };
    assert_eq!(
        result.cleared_pins,
        [
            cleared(NODE_1, "9999", PinClearReason::IngressRemoved),
            cleared("gone", "9001", PinClearReason::NodeRemoved),
        ]
    );
    let events = drain(&mut rx);
    assert_eq!(
        kinds(&events),
        [
            EventKind::StateChanged,
            EventKind::ProfileApplied,
            EventKind::NodeIngressPinCleared,
            EventKind::NodeIngressPinCleared,
        ]
    );
    assert!(matches!(
        &events[2],
        Event::NodeIngressPinCleared { revision, node_id, endpoint_key, reason: PinClearReason::IngressRemoved, .. }
            if revision == R1 && node_id == NODE_1 && endpoint_key == "9999"
    ));
    let status = engine.status();
    assert_eq!(status.nodes[0].pinned_endpoint_key, None);
    assert_eq!(status.nodes[1].pinned_endpoint_key.as_deref(), Some("9003"));
}

#[tokio::test]
async fn a_running_apply_reloads_and_sets_selection_and_pins_again() {
    let (engine, fake) = engine();
    running(&engine).await;
    let result = engine
        .apply(
            ApplyRequest::new(profile(R2))
                .with_selected_node_id(NODE_2)
                .with_pins(vec![Pin::new(NODE_1, "9002")]),
        )
        .await
        .unwrap();
    assert_eq!(result.switch, Some(SwitchKind::KernelSwitch));
    let calls = fake.calls();
    assert!(matches!(calls[1], Call::Reload(_)), "{calls:?}");
    assert!(calls.contains(&Call::Select(SELECTED_TAG.into(), node_tag(NODE_2))));
    assert!(calls.contains(&Call::Select(node_tag(NODE_1), ingress_tag(NODE_1, "9002"))));
    let status = engine.status();
    assert_eq!(status.state, EngineState::Running);
    assert_eq!(status.revision.as_deref(), Some(R2));
    assert_eq!(status.selected_ingress.unwrap().endpoint_key, "9003");
    let first = &status.nodes[0].ingresses;
    assert_eq!((first[0].active, first[1].active), (false, true));

    // Unpinned again: the node's selector goes back to its fallback group.
    engine
        .apply(ApplyRequest::new(profile(R2)).with_selected_node_id(NODE_2))
        .await
        .unwrap();
    assert!(fake.calls().contains(&Call::Select(
        node_tag(NODE_1),
        format!("{}{AUTO_SUFFIX}", node_tag(NODE_1))
    )));
}

#[tokio::test]
async fn a_failed_apply_keeps_what_runs_and_says_why() {
    let (engine, fake) = engine();
    let mut rx = engine.subscribe(EventKind::ALL);

    let err = engine
        .apply(ApplyRequest::new(Vec::new()))
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::PROFILE_REQUIRED);
    let events = drain(&mut rx);
    assert!(matches!(
        events.as_slice(),
        [Event::ReloadFailed { code, .. }] if code == codes::PROFILE_REQUIRED
    ));
    assert_eq!(engine.status().state, EngineState::Stopped);

    running(&engine).await;
    drain(&mut rx);

    // D3: a default node the profile does not have is refused as given.
    let stale_default = profile_with(R2, |p| {
        p["selection"]["default_node_id"] = "missing".into();
    });
    let err = engine
        .apply(ApplyRequest::new(stale_default).with_selected_node_id(NODE_1))
        .await
        .unwrap_err();
    assert_eq!(
        (err.code, err.field.as_deref()),
        (
            codes::DEFAULT_NODE_NOT_FOUND,
            Some("selection.default_node_id")
        )
    );
    assert_eq!(kinds(&drain(&mut rx)), [EventKind::ReloadFailed]);

    fake.fail_next(Op::Reload, RuntimeError::new("config", "bad"));
    let err = engine
        .apply(ApplyRequest::new(profile(R2)))
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::CORE_OPERATION_FAILED);
    let events = drain(&mut rx);
    assert!(matches!(
        events.as_slice(),
        [Event::ReloadFailed { code, .. }] if code == codes::CORE_OPERATION_FAILED
    ));
    let status = engine.status();
    assert_eq!(status.state, EngineState::Running);
    assert_eq!(status.revision.as_deref(), Some(R1));

    // The same request goes through once the runtime takes it.
    assert!(
        engine
            .apply(ApplyRequest::new(profile(R2)))
            .await
            .unwrap()
            .applied
    );
}

#[tokio::test]
async fn a_failed_start_stays_configured_and_a_panic_is_fatal() {
    let (engine, fake) = engine();
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    let mut rx = engine.subscribe(EventKind::ALL);

    fake.fail_next(Op::Start, RuntimeError::new("io", "busy"));
    let err = engine.start().await.unwrap_err();
    assert_eq!(
        (err.code, err.retryable),
        (codes::CORE_OPERATION_FAILED, true)
    );
    assert_eq!(engine.status().state, EngineState::Configured);
    assert!(drain(&mut rx).is_empty());

    fake.fail_next(Op::Start, RuntimeError::new("panicked", "boom"));
    let err = engine.start().await.unwrap_err();
    assert_eq!(err.code, codes::CORE_PANICKED);
    assert_eq!(
        engine.status().state,
        EngineState::Fatal {
            reason: FatalReason::Panic
        }
    );
    assert_eq!(
        state_change(&drain(&mut rx)[0]),
        (
            EngineState::Fatal {
                reason: FatalReason::Panic
            },
            EngineState::Configured
        )
    );
    assert_eq!(engine.start().await.unwrap_err().code, codes::ENGINE_FATAL);
    assert_eq!(
        engine
            .apply(ApplyRequest::new(profile(R2)))
            .await
            .unwrap_err()
            .code,
        codes::ENGINE_FATAL
    );
}

#[tokio::test]
async fn a_runtime_that_ends_on_its_own_is_fatal() {
    let (engine, fake) = engine();
    running(&engine).await;
    let mut rx = engine.subscribe(EventKind::ALL);
    fake.set_state(RuntimeState::Failed {
        code: "internal".into(),
        message: "gone".into(),
    });
    assert_eq!(
        state_change(&next_event(&mut rx).await),
        (
            EngineState::Fatal {
                reason: FatalReason::KernelUnrecoverable
            },
            EngineState::Running
        )
    );
    assert_eq!(engine.stop().await.unwrap_err().code, codes::ENGINE_FATAL);
}

#[tokio::test]
async fn no_default_interface_degrades_a_running_instance() {
    let (engine, _) = engine();
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    let mut rx = engine.subscribe(EventKind::ALL);

    // Not running: remembered, nothing sent.
    engine.on_network(None);
    assert!(drain(&mut rx).is_empty());
    assert_eq!(engine.status().state, EngineState::Configured);

    let offline = EngineState::Degraded {
        reasons: vec![DegradedReason::NoDefaultInterface],
    };
    engine.start().await.unwrap();
    let events = drain(&mut rx);
    assert_eq!(
        state_change(&events[0]),
        (offline.clone(), EngineState::Configured)
    );

    engine.on_network(Some(("en0", 4)));
    let events = drain(&mut rx);
    assert_eq!(
        kinds(&events),
        [EventKind::NetworkChanged, EventKind::StateChanged]
    );
    assert!(matches!(
        &events[0],
        Event::NetworkChanged { has_default_interface: true, interface_name, interface_index: 4, .. }
            if interface_name == "en0"
    ));
    assert_eq!(
        state_change(&events[1]),
        (EngineState::Running, offline.clone())
    );

    engine.on_network(None);
    let events = drain(&mut rx);
    assert!(matches!(
        &events[0],
        Event::NetworkChanged {
            has_default_interface: false,
            ..
        }
    ));
    assert_eq!(state_change(&events[1]), (offline, EngineState::Running));
    assert!(matches!(
        engine.status().state,
        EngineState::Degraded { .. }
    ));
}

#[tokio::test]
async fn failover_is_reported_and_shown() {
    let (engine, fake) = engine();
    running(&engine).await;
    let mut rx = engine.subscribe(EventKind::ALL);
    let auto = format!("{}{AUTO_SUFFIX}", node_tag(NODE_1));
    fake.switch(&auto, &ingress_tag(NODE_1, "9002"), "down");
    assert!(matches!(
        next_event(&mut rx).await,
        Event::NodeIngressSwitched { node_id, endpoint_key, previous_endpoint_key, .. }
            if node_id == NODE_1 && endpoint_key == "9002" && previous_endpoint_key == "9001"
    ));
    let selected = engine.status().selected_ingress.unwrap();
    assert_eq!(
        (
            selected.endpoint_key.as_str(),
            selected.previous_endpoint_key.as_str(),
            selected.role.as_str()
        ),
        ("9002", "9001", "backup")
    );
    assert!(selected.switched_at.is_some());
}

#[tokio::test]
async fn shutdown_stops_the_runtime_and_ends_subscriptions() {
    let (engine, fake) = engine();
    running(&engine).await;
    let mut rx = engine.subscribe(EventKind::ALL);
    assert_eq!(engine.shutdown().await, Ok(ShutdownReport::default()));
    assert_eq!(fake.calls().last(), Some(&Call::Stop));
    assert_eq!(
        state_change(&drain(&mut rx)[0]),
        (EngineState::Stopped, EngineState::Running)
    );
    assert_eq!(rx.recv().await, None);
    assert_eq!(engine.status().state, EngineState::Stopped);
    assert_eq!(
        engine.start().await.unwrap_err().code,
        codes::ENGINE_SHUT_DOWN
    );
    assert_eq!(engine.shutdown().await, Ok(ShutdownReport::default()));
}

#[tokio::test]
async fn tun_routing_degrades_recovers_and_breaks() {
    use crate::engine::TunRoutingSignal;
    use crate::status::TunRouting;

    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent"),
        fake,
    );
    running(&engine).await;
    assert_eq!(engine.status().tun_routing, Some(TunRouting::Ok));
    let mut rx = engine.subscribe(EventKind::ALL);

    let restoring = EngineState::Degraded {
        reasons: vec![DegradedReason::TunRoutingRestoring],
    };
    engine.on_tun_routing(TunRoutingSignal::Restoring);
    let events = drain(&mut rx);
    assert_eq!(kinds(&events), [EventKind::StateChanged]);
    assert_eq!(
        state_change(&events[0]),
        (restoring.clone(), EngineState::Running)
    );
    assert_eq!(engine.status().tun_routing, Some(TunRouting::Restoring));

    engine.on_tun_routing(TunRoutingSignal::Restored {
        missing: vec!["9101/v4 nop".into()],
    });
    let events = drain(&mut rx);
    assert_eq!(
        kinds(&events),
        [EventKind::TunRoutingRestored, EventKind::StateChanged]
    );
    assert_eq!(state_change(&events[1]), (EngineState::Running, restoring));

    engine.on_tun_routing(TunRoutingSignal::Unguarded);
    assert_eq!(
        engine.status().state,
        EngineState::Degraded {
            reasons: vec![DegradedReason::TunRoutingUnguarded]
        }
    );
    drain(&mut rx);

    engine.on_tun_routing(TunRoutingSignal::Broken {
        missing: vec!["9093/v4 iif tun0 goto 9101".into()],
        error: "file exists".into(),
    });
    let events = drain(&mut rx);
    assert_eq!(
        kinds(&events),
        [EventKind::TunRoutingBroken, EventKind::StateChanged]
    );
    let broken = EngineState::Fatal {
        reason: FatalReason::TunRoutingBroken {
            missing: vec!["9093/v4 iif tun0 goto 9101".into()],
        },
    };
    assert_eq!(state_change(&events[1]).0, broken);
    assert_eq!(engine.status().state, broken);
    assert_eq!(engine.start().await.unwrap_err().code, codes::ENGINE_FATAL);
}

#[tokio::test]
async fn the_last_handles_drop_stops_the_runtime() {
    let (engine, fake) = engine();
    running(&engine).await;
    let other = engine.clone();
    drop(engine);
    assert_eq!(
        fake.calls().last(),
        Some(&Call::Start(fake.config().unwrap()))
    );
    drop(other);
    assert_eq!(fake.calls().last(), Some(&Call::Stop));
}

/// Every public method after `shutdown`: lifecycle calls (and probes) are
/// ENGINE_SHUT_DOWN, queries the last snapshot, subscriptions closed. A
/// new public method goes into this table.
#[tokio::test]
async fn every_public_method_after_shutdown() {
    use crate::config::LocalProxyConfig;
    use crate::types::{ProbeAvailabilityRequest, ProbeEntrancesRequest, ProbeMethod};

    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, "/nonexistent")
            .with_local_proxy(LocalProxyConfig::new())
            .with_system_proxy(true),
        Arc::new(FakeRuntime::default()),
    );
    running(&engine).await;
    engine.shutdown().await.unwrap();

    let refused: Vec<(&str, Result<(), Error>)> = vec![
        (
            "apply",
            engine
                .apply(ApplyRequest::new(profile(R2)))
                .await
                .map(|_| ()),
        ),
        ("start", engine.start().await),
        ("stop", engine.stop().await),
        ("select_node", engine.select_node(NODE_2).await),
        (
            "pin_ingress",
            engine.pin_ingress(NODE_1, Some("9002")).await,
        ),
        (
            "set_system_proxy_listener",
            engine.set_system_proxy_listener(true).await.map(|_| ()),
        ),
        (
            "probe_entrances",
            engine
                .probe_entrances(ProbeEntrancesRequest::new(ProbeMethod::Tcp, 1000, 1))
                .await
                .map(|_| ()),
        ),
        (
            "probe_availability",
            engine
                .probe_availability(ProbeAvailabilityRequest::new(
                    NODE_1,
                    "https://example.com/",
                    1000,
                ))
                .await
                .map(|_| ()),
        ),
    ];
    for (method, result) in refused {
        assert_eq!(
            result.map_err(|e| e.code),
            Err(codes::ENGINE_SHUT_DOWN),
            "{method}"
        );
    }

    // Queries: the last snapshot, Stopped.
    let status = engine.status();
    assert_eq!(status.state, EngineState::Stopped);
    assert_eq!(status.revision.as_deref(), Some(R1));
    let _ = (
        engine.nodes(),
        engine.selected_node(),
        engine.traffic(),
        engine.connections(),
        engine.local_proxy_metadata(),
        engine.local_proxy_credential(NODE_1),
        engine.local_proxy_routed_credential(),
        engine.logs(),
    );
    assert_eq!(engine.subscribe(EventKind::ALL).recv().await, None);
    // Idempotent.
    assert_eq!(engine.shutdown().await, Ok(ShutdownReport::default()));
}
