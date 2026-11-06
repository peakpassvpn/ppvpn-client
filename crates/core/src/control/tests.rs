use super::*;

const NODE: &str = "3f2c9a1e-0000-4000-8000-000000000001-128";
fn request() -> ApplyRequest {
    ApplyRequest::new(
        include_bytes!("../../../../testdata/golden/contract/profiles/base.json").to_vec(),
    )
}

#[test]
fn control_keeps_profile_rules_selection_and_valid_pins_without_an_engine() {
    let mut request = request();
    request.selected_node_id = Some("removed-node".into());
    request.pins = vec![Pin::new(NODE, "9001"), Pin::new("removed", "gone")];
    let raw = request.profile.clone();
    let (mut session, result) = ProfileSession::prepare(request).unwrap();
    assert!(result.selection_reset);
    assert_eq!(result.cleared_pins.len(), 1);
    assert_eq!(session.selected_node().id, NODE);
    assert_eq!(session.pins(), &[Pin::new(NODE, "9001")]);
    let other = session.nodes()[1].id.clone();
    session.select_node(&other).unwrap();
    session.set_routing_mode(RoutingMode::Global);
    session.pin_ingress(NODE, "9002").unwrap();
    assert_eq!(session.pins(), &[Pin::new(NODE, "9002")]);
    let canonical = session.apply_request();
    assert_eq!(
        canonical.profile, raw,
        "rules and profile are retained verbatim"
    );
    assert_eq!(canonical.selected_node_id.as_deref(), Some(other.as_str()));
    assert_eq!(canonical.routing_mode, RoutingMode::Global);
    let (restored, _) = ProfileSession::prepare(canonical).unwrap();
    assert_eq!(restored.selected_node().id, other);
    assert_eq!(restored.pins(), session.pins());
    let before = session.apply_request();
    assert!(session.select_node("absent").is_err());
    assert!(session.pin_ingress(NODE, "absent").is_err());
    assert_eq!(
        session.apply_request(),
        before,
        "failed controls do not mutate state"
    );
}

#[tokio::test]
async fn offline_control_probe_fails_without_starting_a_forwarder() {
    let (session, _) = ProfileSession::prepare(request()).unwrap();
    let error = session
        .probe_entrances(
            ProbeEntrancesRequest::new(crate::ProbeMethod::Tcp, 100, 2),
            ProbeNetwork::Offline,
            &ProbeCancellation::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, codes::NO_DEFAULT_INTERFACE);
}

#[tokio::test]
async fn canceled_control_probe_does_not_need_a_runtime() {
    let (session, _) = ProfileSession::prepare(request()).unwrap();
    let cancellation = ProbeCancellation::default();
    cancellation.cancel();
    let result = session
        .probe_entrances(
            ProbeEntrancesRequest::new(crate::ProbeMethod::Tcp, 100, 2),
            ProbeNetwork::Online,
            &cancellation,
        )
        .await
        .unwrap();
    assert!(result
        .iter()
        .all(|r| !r.success && r.error_code == "CANCELED"));
}

#[cfg(feature = "forwarder")]
#[tokio::test]
async fn control_and_engine_reconcile_selection_and_pins_the_same_way() {
    let mut request = request();
    request.selected_node_id = Some("gone".into());
    request.pins = vec![Pin::new(NODE, "gone"), Pin::new("gone", "9001")];
    let (session, expected) = ProfileSession::prepare(request.clone()).unwrap();
    let fake = std::sync::Arc::new(crate::runtime::fake::FakeRuntime::default());
    let engine = crate::Engine::with_runtime(
        crate::EngineConfig::new(crate::Role::Standard, crate::Platform::Macos, "/unused"),
        fake.clone(),
    );
    let actual = engine.apply(request).await.unwrap();
    assert_eq!(expected, actual);
    assert_eq!(engine.nodes(), session.nodes());
    assert!(fake.calls().is_empty());
    engine.shutdown().await.unwrap();
}
