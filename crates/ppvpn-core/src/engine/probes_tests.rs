//! Probes through the Engine on the fake runtime: the checks before them,
//! the default interface, the events.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::lifecycle_tests::{drain, engine, kinds, profile, running, NODE_1, NODE_2, R1};
use super::*;
use crate::config::{LocalProxyConfig, Platform};
use crate::probe::Net;
use crate::runtime::fake::{Call, FakeRuntime};
use crate::types::ProbeMethod;

/// Every handshake and echo succeeds; names resolve to a documentation
/// address. Counts what it was asked.
#[derive(Default)]
struct Reachable {
    calls: AtomicUsize,
}

#[async_trait]
impl Net for Reachable {
    async fn connect(&self, _to: SocketAddr) -> io::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn ping(&self, _to: IpAddr, _timeout: Duration) -> Result<Duration, &'static str> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Duration::from_millis(1))
    }

    async fn resolve(&self, _host: &str) -> io::Result<Vec<IpAddr>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))])
    }
}

/// A server that answers every request with 204.
async fn no_content() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            });
        }
    });
    addr
}

fn dials(fake: &FakeRuntime) -> Vec<String> {
    fake.calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::DialTcp(outbound, _) => Some(outbound),
            _ => None,
        })
        .collect()
}

/// Entrance probes need an applied profile, not a running instance (as
/// Go); one EntranceProbed per node; offline they fail at once and probe
/// nothing (Go: TestProbesFailFastWithoutDefaultInterface).
#[tokio::test]
async fn entrance_probes_on_the_applied_profile() {
    let (engine, _) = engine();
    let net = Reachable::default();
    let request = || ProbeEntrancesRequest::new(ProbeMethod::Tcp, 1000, 2);
    let err = engine.probe_entrances(request()).await.unwrap_err();
    assert_eq!(
        (err.code, err.retryable),
        (codes::PROFILE_NOT_APPLIED, false)
    );

    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    let mut rx = engine.subscribe(EventKind::ALL);
    let results = engine.inner.probe_entrances(request(), &net).await.unwrap();
    assert_eq!(
        results
            .iter()
            .map(|r| (r.node_id.as_str(), r.success))
            .collect::<Vec<_>>(),
        [(NODE_1, true), (NODE_2, true)]
    );
    let events = drain(&mut rx);
    assert_eq!(
        kinds(&events),
        [EventKind::EntranceProbed, EventKind::EntranceProbed]
    );
    match &events[0] {
        Event::EntranceProbed {
            revision,
            node_id,
            message,
            ..
        } => assert_eq!(
            (revision.as_str(), node_id.as_str(), message.as_str()),
            (R1, NODE_1, "success")
        ),
        other => panic!("{other:?}"),
    }

    let only = request().with_node_ids(vec![NODE_2.into()]);
    let results = engine.inner.probe_entrances(only, &net).await.unwrap();
    assert_eq!(results.len(), 1);
    let unknown = request().with_node_ids(vec!["missing".into()]);
    let err = engine
        .inner
        .probe_entrances(unknown, &net)
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::NODE_NOT_FOUND);
    drain(&mut rx);

    let probed = net.calls.load(Ordering::SeqCst);
    engine.on_network(None);
    let err = engine.probe_entrances(request()).await.unwrap_err();
    assert_eq!(
        (err.code, err.retryable),
        (codes::NO_DEFAULT_INTERFACE, true)
    );
    let err = engine
        .inner
        .probe_entrances(request(), &net)
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::NO_DEFAULT_INTERFACE);
    assert_eq!(net.calls.load(Ordering::SeqCst), probed, "probed offline");
    assert!(drain(&mut rx).is_empty());

    engine.on_network(Some(("eth0", 2)));
    assert_eq!(
        engine
            .inner
            .probe_entrances(request(), &net)
            .await
            .unwrap()
            .len(),
        2
    );
}

/// Availability goes through the node's outbound. Its checks, in Go's
/// order: LOCAL_PROXY_DISABLED (elsewhere), PROFILE_NOT_APPLIED,
/// CORE_NOT_RUNNING; offline NO_DEFAULT_INTERFACE without a dial.
#[tokio::test]
async fn availability_probes_through_the_node() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, tmp.path())
            .with_local_proxy(LocalProxyConfig::new().with_preferred_port(0)),
        fake.clone(),
    );
    let request = |node: &str| ProbeAvailabilityRequest::new(node, "http://example.com/", 5000);

    let err = engine
        .probe_availability(request(NODE_1))
        .await
        .unwrap_err();
    assert_eq!(
        (err.code, err.retryable),
        (codes::PROFILE_NOT_APPLIED, false)
    );
    engine.apply(ApplyRequest::new(profile(R1))).await.unwrap();
    let err = engine
        .probe_availability(request(NODE_1))
        .await
        .unwrap_err();
    assert_eq!((err.code, err.retryable), (codes::CORE_NOT_RUNNING, true));

    engine.start().await.unwrap();
    fake.route_tcp_to(no_content().await);
    let mut rx = engine.subscribe(&[EventKind::AvailabilityProbed]);
    let result = engine.probe_availability(request(NODE_2)).await.unwrap();
    assert!(result.success, "{result:?}");
    assert_eq!((result.node_id.as_str(), result.http_status), (NODE_2, 204));
    let node_tag = engine
        .inner
        .live()
        .applied
        .as_ref()
        .unwrap()
        .translation
        .node_tags[NODE_2]
        .clone();
    assert_eq!(dials(&fake), [node_tag]);
    match drain(&mut rx).as_slice() {
        [Event::AvailabilityProbed {
            node_id, message, ..
        }] => assert_eq!((node_id.as_str(), message.as_str()), (NODE_2, "success")),
        other => panic!("{other:?}"),
    }

    let err = engine
        .probe_availability(request("missing"))
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::NODE_NOT_FOUND);

    engine.on_network(None);
    let err = engine
        .probe_availability(request(NODE_1))
        .await
        .unwrap_err();
    assert_eq!(
        (err.code, err.retryable),
        (codes::NO_DEFAULT_INTERFACE, true)
    );
    assert_eq!(dials(&fake).len(), 1, "dialled offline");
    assert!(drain(&mut rx).is_empty());
}

/// A TUN instance has no local proxy: LOCAL_PROXY_DISABLED, running or not
/// (decided by the Core group, as Go).
#[tokio::test]
async fn a_tun_instance_does_not_probe_availability() {
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent"),
        Arc::new(FakeRuntime::default()),
    );
    let request = ProbeAvailabilityRequest::new(NODE_1, "http://example.com/", 1000);
    let err = engine
        .probe_availability(request.clone())
        .await
        .unwrap_err();
    assert_eq!(err.code, codes::LOCAL_PROXY_DISABLED);
    running(&engine).await;
    let err = engine.probe_availability(request).await.unwrap_err();
    assert_eq!(err.code, codes::LOCAL_PROXY_DISABLED);
}
