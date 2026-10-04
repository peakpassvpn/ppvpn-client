//! select_node, pin_ingress and the queries on the fake runtime.

use std::time::{Duration, UNIX_EPOCH};

use super::lifecycle_tests::{drain, engine, kinds, profile, running, NODE_1, NODE_2, R1};
use super::*;
use crate::request::Pin;
use crate::runtime::fake::{Call, Op};
use crate::runtime::{RuntimeConnection, RuntimeTraffic};
use crate::translate::{ingress_tag, node_tag, AUTO_SUFFIX, SELECTED_TAG};

#[tokio::test]
async fn select_node_before_and_after_a_profile() {
    let (engine, fake) = engine();
    // D2: before a profile, PROFILE_NOT_APPLIED.
    let err = engine.select_node(NODE_2).await.unwrap_err();
    assert_eq!(
        (err.code, err.retryable),
        (codes::PROFILE_NOT_APPLIED, false)
    );

    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    let mut rx = engine.subscribe(EventKind::ALL);
    // D2: an unknown node, NODE_NOT_FOUND; the selection stays.
    let err = engine.select_node("missing").await.unwrap_err();
    assert_eq!(
        (err.code, err.field.as_deref(), err.retryable),
        (codes::NODE_NOT_FOUND, Some("node_id"), false)
    );
    assert!(drain(&mut rx).is_empty());
    assert_eq!(engine.status().selected_node_id.as_deref(), Some(NODE_1));

    // Stopped: kept, and the start builds with it.
    engine.select_node(NODE_2).await.unwrap();
    assert!(matches!(
        drain(&mut rx).as_slice(),
        [Event::NodeSelected { revision, node_id, .. }] if revision == R1 && node_id == NODE_2
    ));
    assert!(fake.calls().is_empty());
    assert_eq!(engine.selected_node().unwrap().id, NODE_2);
    engine.start().await.unwrap();
    let config: serde_json::Value = serde_json::from_str(&fake.config().unwrap()).unwrap();
    let selected = config["outbounds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["tag"] == SELECTED_TAG)
        .unwrap();
    assert_eq!(selected["default"], node_tag(NODE_2).as_str());

    // Running: sail's selector moves.
    engine.select_node(NODE_1).await.unwrap();
    assert_eq!(
        fake.calls().last(),
        Some(&Call::Select(SELECTED_TAG.into(), node_tag(NODE_1)))
    );
    let status = engine.status();
    assert_eq!(status.selected_node_id.as_deref(), Some(NODE_1));
    assert_eq!(status.selected_ingress.unwrap().endpoint_key, "9001");

    // The host passes back what it persisted: nothing to do.
    let back = ApplyRequest::new(profile(R1)).with_selected_node_id(NODE_1);
    assert!(!engine.apply(back).await.unwrap().applied);
}

#[tokio::test]
async fn a_select_sail_refuses_changes_nothing() {
    let (engine, fake) = engine();
    running(&engine).await;
    fake.fail_next(Op::Select, RuntimeError::new("internal", "no"));
    let err = engine.select_node(NODE_2).await.unwrap_err();
    assert_eq!(err.code, codes::CORE_OPERATION_FAILED);
    assert_eq!(engine.status().selected_node_id.as_deref(), Some(NODE_1));
}

#[tokio::test]
async fn pin_ingress_validates_then_pins_and_unpins() {
    let (engine, fake) = engine();
    let err = engine.pin_ingress(NODE_1, Some("9002")).await.unwrap_err();
    assert_eq!(err.code, codes::PROFILE_NOT_APPLIED);
    running(&engine).await;
    let mut rx = engine.subscribe(EventKind::ALL);

    for key in ["9999", ""] {
        let err = engine.pin_ingress(NODE_1, Some(key)).await.unwrap_err();
        assert_eq!(
            (err.code, err.field.as_deref()),
            (codes::INGRESS_NOT_FOUND, Some("endpoint_key")),
            "{key:?}"
        );
    }
    let err = engine
        .pin_ingress("missing", Some("9001"))
        .await
        .unwrap_err();
    assert_eq!(
        (err.code, err.field.as_deref()),
        (codes::NODE_NOT_FOUND, Some("node_id"))
    );
    assert!(drain(&mut rx).is_empty());

    engine.pin_ingress(NODE_1, Some("9002")).await.unwrap();
    assert_eq!(
        fake.calls().last(),
        Some(&Call::Select(node_tag(NODE_1), ingress_tag(NODE_1, "9002")))
    );
    assert!(matches!(
        drain(&mut rx).as_slice(),
        [Event::NodeIngressPinned { node_id, endpoint_key, .. }] if node_id == NODE_1 && endpoint_key == "9002"
    ));
    let status = engine.status();
    let node = &status.nodes[0];
    assert_eq!(node.pinned_endpoint_key.as_deref(), Some("9002"));
    assert_eq!(
        (node.ingresses[0].active, node.ingresses[1].active),
        (false, true)
    );
    assert_eq!(status.selected_ingress.unwrap().endpoint_key, "9002");

    // What the host persisted, passed back: nothing to do.
    let back = ApplyRequest::new(profile(R1)).with_pins(vec![Pin::new(NODE_1, "9002")]);
    assert!(!engine.apply(back).await.unwrap().applied);

    // A single-ingress node: kept, no group to move.
    let calls = fake.calls().len();
    engine.pin_ingress(NODE_2, Some("9003")).await.unwrap();
    assert_eq!(fake.calls().len(), calls);
    assert_eq!(
        engine.status().nodes[1].pinned_endpoint_key.as_deref(),
        Some("9003")
    );

    engine.pin_ingress(NODE_1, None).await.unwrap();
    assert_eq!(
        fake.calls().last(),
        Some(&Call::Select(
            node_tag(NODE_1),
            format!("{}{AUTO_SUFFIX}", node_tag(NODE_1))
        ))
    );
    let events = drain(&mut rx);
    assert_eq!(kinds(&events).last(), Some(&EventKind::NodeIngressPinned));
    assert!(matches!(
        events.last(),
        Some(Event::NodeIngressPinned { endpoint_key, .. }) if endpoint_key.is_empty()
    ));
    let status = engine.status();
    assert_eq!(status.nodes[0].pinned_endpoint_key, None);
    assert!(status.nodes[0].ingresses[0].active);
}

#[tokio::test]
async fn queries_come_from_the_profile_and_the_runtime() {
    let (engine, fake) = engine();
    assert!(engine.nodes().is_empty());
    assert_eq!(engine.selected_node(), None);
    assert!(engine.connections().is_empty());

    fake.set_traffic(RuntimeTraffic {
        upload_bytes: 10,
        download_bytes: 20,
    });
    let started = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
    let connection = |id, chain: Vec<String>| RuntimeConnection {
        id,
        inbound: "mixed".into(),
        chain,
        network: "tcp".into(),
        destination: "198.51.100.7:443".into(),
        upload_bytes: 1,
        download_bytes: 2,
        started,
    };
    fake.set_connections(vec![
        connection(
            7,
            vec![
                SELECTED_TAG.into(),
                node_tag(NODE_1),
                format!("{}{AUTO_SUFFIX}", node_tag(NODE_1)),
                ingress_tag(NODE_1, "9001"),
            ],
        ),
        connection(8, vec!["direct".into()]),
    ]);
    running(&engine).await;

    let nodes = engine.nodes();
    assert_eq!(nodes.len(), 2);
    assert_eq!(
        (
            nodes[1].protocol.as_str(),
            nodes[1].udp,
            nodes[1].entry_label.as_str()
        ),
        ("anytls", false, "CN Optimized")
    );
    assert_eq!(nodes[0].ingresses[1].label, "Tokyo relay");
    assert_eq!(engine.selected_node().unwrap().id, NODE_1);

    let traffic = engine.traffic();
    assert_eq!((traffic.upload_bytes, traffic.download_bytes), (10, 20));
    let connections = engine.connections();
    assert_eq!(connections.len(), 2);
    assert_eq!(
        (connections[0].id.as_str(), connections[0].node_id.as_str()),
        ("7", NODE_1)
    );
    assert_eq!(connections[1].node_id, "");
    assert_eq!(connections[0].started_at, DateTime::<Utc>::from(started));

    // Stopped: no connections; the counters stay as last read.
    engine.stop().await.unwrap();
    assert!(engine.connections().is_empty());
    assert_eq!(engine.traffic().upload_bytes, 10);
}

/// docs/backend-profile.md: checks go on while a node is pinned. The pin
/// leaves the node's fallback group unused, so sail's lazy tests would
/// pause; the Engine has the group test each interval while the pin lasts,
/// and no longer once unpinned or stopped.
#[tokio::test(start_paused = true)]
async fn a_pinned_node_keeps_its_group_checked() {
    let (engine, fake) = engine();
    running(&engine).await;
    let group = format!("{}{AUTO_SUFFIX}", node_tag(NODE_1));
    let checks = || {
        fake.calls()
            .iter()
            .filter(|c| matches!(c, Call::CheckGroup(_)))
            .cloned()
            .collect::<Vec<_>>()
    };
    let half = crate::translate::CHECK_INTERVAL / 2;
    let rounds = |n: u32| tokio::time::sleep(crate::translate::CHECK_INTERVAL * n);

    // Unpinned: sail's own tests, none asked by the Engine.
    tokio::time::sleep(half).await;
    rounds(2).await;
    assert_eq!(checks(), vec![]);

    // A single-ingress node has no group to check.
    engine.pin_ingress(NODE_1, Some("9002")).await.unwrap();
    engine.pin_ingress(NODE_2, Some("9003")).await.unwrap();
    rounds(2).await;
    assert_eq!(
        checks(),
        vec![
            Call::CheckGroup(group.clone()),
            Call::CheckGroup(group.clone())
        ]
    );

    engine.pin_ingress(NODE_1, None).await.unwrap();
    rounds(2).await;
    assert_eq!(checks().len(), 2, "unpinned");

    engine.pin_ingress(NODE_1, Some("9002")).await.unwrap();
    engine.stop().await.unwrap();
    rounds(2).await;
    assert_eq!(checks().len(), 2, "stopped");

    // The next start checks the pin it kept.
    engine.start().await.unwrap();
    tokio::time::sleep(half).await;
    rounds(1).await;
    assert_eq!(checks().len(), 3, "started again");
}
