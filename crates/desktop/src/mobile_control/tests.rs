use super::*;

fn profile() -> ProfileSession {
    ProfileSession::prepare(ppvpn_core::ApplyRequest::new(
        include_bytes!("../../../../testdata/golden/contract/profiles/base.json").to_vec(),
    ))
    .unwrap()
    .0
}

#[tokio::test]
async fn offline_direct_probes_report_every_node_without_a_forwarder() {
    let control = MobileControl::default();
    control.network_changed(ppvpn_core::NetworkState::default());
    let profile = profile();
    let count = profile.nodes().len();
    let results = Mutex::new(Vec::new());
    control
        .probe_profile(profile, ProbeMethod::Tcp, Vec::new(), &|result| {
            results.lock().unwrap().push(result);
        })
        .await
        .unwrap();
    let results = results.into_inner().unwrap();
    assert_eq!(results.len(), count);
    assert!(results
        .iter()
        .all(|result| !result.success
            && result.error.as_ref().unwrap().detail == "NO_DEFAULT_INTERFACE"));
}

#[tokio::test]
async fn direct_tcp_uses_a_handshake_without_a_proxy_runtime() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = accepted.clone();
    let server = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    let mut raw: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../../testdata/golden/contract/profiles/base.json"
    ))
    .unwrap();
    let id = raw["nodes"][0]["id"].as_str().unwrap().to_string();
    for ingress in raw["nodes"][0]["ingresses"].as_array_mut().unwrap() {
        ingress["endpoint"]["ip"] = serde_json::json!("");
        ingress["endpoint"]["domain"] = serde_json::json!("localhost");
        if let Some(tls) = ingress.get_mut("tls") {
            tls["server_name"] = serde_json::json!("localhost");
        }
        ingress["endpoint"]["port"] = serde_json::json!(port);
    }
    let session = ProfileSession::prepare(ppvpn_core::ApplyRequest::new(
        serde_json::to_vec(&raw).unwrap(),
    ))
    .unwrap()
    .0;
    let results = Mutex::new(Vec::new());
    MobileControl::default()
        .probe_profile(session, ProbeMethod::Tcp, vec![id], &|result| {
            results.lock().unwrap().push(result);
        })
        .await
        .unwrap();
    server.abort();
    let results = results.into_inner().unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].success,
        "a bare TCP handshake must succeed without proxy authentication"
    );
    assert!(results[0].latency_ms.is_some());
    assert!(accepted.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn an_offline_request_reports_a_duplicate_node_once() {
    let control = MobileControl::default();
    control.network_changed(ppvpn_core::NetworkState::default());
    let profile = profile();
    let id = profile.selected_node().id;
    let results = Mutex::new(Vec::new());
    control
        .probe_profile(profile, ProbeMethod::Tcp, vec![id.clone(), id], &|result| {
            results.lock().unwrap().push(result);
        })
        .await
        .unwrap();
    assert_eq!(results.into_inner().unwrap().len(), 1);
}

#[tokio::test]
async fn a_protocol_test_never_uses_the_direct_probe_path() {
    let error = MobileControl::default()
        .probe_profile(profile(), ProbeMethod::Connect, Vec::new(), &|_| {
            panic!("no direct protocol result")
        })
        .await
        .unwrap_err();
    assert_eq!(error.info().unwrap().detail, "VPN_REQUIRED");
}

#[tokio::test]
async fn an_accidental_app_standard_start_fails_closed() {
    match NoAppEngine.launch().await {
        Ok(_) => panic!("mobile App must never launch a forwarder"),
        Err(error) => assert_eq!(error.detail, "MOBILE_APP_FORWARDING_DISABLED"),
    }
}
