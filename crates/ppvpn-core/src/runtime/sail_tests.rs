//! SailRuntime against a real sail instance: a mixed inbound (the local
//! proxy's shape: SOCKS5 with users) and direct outbounds behind a selector,
//! dialling an echo server on loopback.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;

const WAIT: Duration = Duration::from_secs(5);

/// A password made for this run: none is written in the source.
fn password() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An echo server on loopback.
async fn echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

fn config(port: u16, users: &[(&str, &str)], extra_outbound: bool) -> String {
    let users: Vec<_> = users
        .iter()
        .map(|(u, p)| serde_json::json!({ "username": u, "password": p }))
        .collect();
    let mut outbounds = vec![
        serde_json::json!({ "type": "direct", "tag": "direct" }),
        serde_json::json!({ "type": "direct", "tag": "direct-b" }),
        serde_json::json!({ "type": "selector", "tag": "pick", "outbounds": ["direct", "direct-b"], "default": "direct" }),
    ];
    if extra_outbound {
        outbounds.push(serde_json::json!({ "type": "direct", "tag": "direct-c" }));
    }
    serde_json::json!({
        "log": { "level": "info" },
        "inbounds": [{ "type": "mixed", "tag": "local", "listen": "127.0.0.1", "listen_port": port, "users": users }],
        "outbounds": outbounds,
        "route": { "final": "pick" }
    })
    .to_string()
}

/// A SOCKS5 connection through the local proxy to `to`, as `user`; None
/// when the proxy refuses the user.
async fn socks(port: u16, user: &str, password: &str, to: SocketAddr) -> Option<TcpStream> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    s.write_all(&[5, 1, 2]).await.ok()?;
    let mut reply = [0u8; 2];
    s.read_exact(&mut reply).await.ok()?;
    assert_eq!(reply, [5, 2], "username/password method");
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(password.len() as u8);
    auth.extend_from_slice(password.as_bytes());
    s.write_all(&auth).await.ok()?;
    s.read_exact(&mut reply).await.ok()?;
    if reply != [1, 0] {
        return None;
    }
    let SocketAddr::V4(v4) = to else {
        unreachable!("loopback v4")
    };
    let mut connect = vec![5, 1, 0, 1];
    connect.extend_from_slice(&v4.ip().octets());
    connect.extend_from_slice(&v4.port().to_be_bytes());
    s.write_all(&connect).await.ok()?;
    let mut head = [0u8; 10];
    s.read_exact(&mut head).await.ok()?;
    (head[1] == 0).then_some(s)
}

/// A SOCKS5 CONNECT through the local proxy to `name`:`port`, as `user`:
/// the proxy resolves the name. Whether it connected.
async fn socks_to_domain(port: u16, user: &str, password: &str, name: &str, to: u16) -> bool {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await else {
        return false;
    };
    let mut reply = [0u8; 2];
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(password.len() as u8);
    auth.extend_from_slice(password.as_bytes());
    let mut connect = vec![5, 1, 0, 3, name.len() as u8];
    connect.extend_from_slice(name.as_bytes());
    connect.extend_from_slice(&to.to_be_bytes());
    let mut head = [0u8; 10];
    s.write_all(&[5, 1, 2]).await.is_ok()
        && s.read_exact(&mut reply).await.is_ok()
        && s.write_all(&auth).await.is_ok()
        && s.read_exact(&mut reply).await.is_ok()
        && reply == [1, 0]
        && s.write_all(&connect).await.is_ok()
        && tokio::time::timeout(WAIT, s.read_exact(&mut head))
            .await
            .is_ok_and(|r| r.is_ok())
        && head[1] == 0
}

async fn round_trip(s: &mut (impl AsyncReadExt + AsyncWriteExt + Unpin), text: &[u8]) {
    s.write_all(text).await.unwrap();
    let mut got = vec![0u8; text.len()];
    tokio::time::timeout(WAIT, s.read_exact(&mut got))
        .await
        .expect("echo in time")
        .unwrap();
    assert_eq!(got, text);
}

async fn wait_for(states: &mut watch::Receiver<RuntimeState>, want: RuntimeState) {
    tokio::time::timeout(WAIT, async {
        while *states.borrow_and_update() != want {
            states.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("state {want:?}, have {:?}", *states.borrow()));
}

fn options(name: &str) -> Options {
    let dir = std::env::temp_dir().join(format!("ppvpn-core-sail-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Options::new().data_dir(dir).threads(embed::Threads::One)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_sail_through_the_local_proxy() {
    let echo = echo().await;
    let port = free_port();
    let (p1, p2) = (password(), password());
    let runtime = SailRuntime::new(options("chain")).unwrap();
    let mut states = runtime.states();
    let mut logs = runtime.logs();
    let mut switches = runtime.group_switches();
    assert_eq!(runtime.state(), RuntimeState::Idle);

    // The minimal chain: a hand-written configuration, one connection
    // through the local proxy.
    runtime
        .start(&config(port, &[("u1", &p1)], false))
        .await
        .unwrap();
    wait_for(&mut states, RuntimeState::Running).await;
    assert_eq!(runtime.tun_name(), None, "no tun inbound");
    let mut proxied = socks(port, "u1", &p1, echo)
        .await
        .expect("u1 through the proxy");
    round_trip(&mut proxied, b"through the local proxy").await;
    assert!(
        socks(port, "u1", &password(), echo).await.is_none(),
        "a wrong password is refused"
    );

    // Through one outbound, whatever the rules say.
    let mut dialled = runtime
        .dial_tcp("direct-b", Target::Addr(echo), WAIT)
        .await
        .unwrap();
    round_trip(&mut dialled, b"dialled").await;
    let err = runtime
        .dial_tcp("nope", Target::Addr(echo), WAIT)
        .await
        .err()
        .expect("no such outbound");
    assert_eq!(err.code, "not_found");

    let traffic = runtime.traffic().await.unwrap();
    assert!(
        traffic.upload_bytes > 0 && traffic.download_bytes > 0,
        "{traffic:?}"
    );
    let connections = runtime.connections().await.unwrap();
    let local = connections
        .iter()
        .find(|c| c.inbound == "local")
        .expect("the proxied connection");
    assert_eq!(
        (local.network.as_str(), local.destination.clone()),
        ("tcp", echo.to_string())
    );
    assert!(
        local.chain.contains(&"direct".to_string()),
        "{:?}",
        local.chain
    );
    assert!(now_secs() + 1 >= local.started.duration_since(UNIX_EPOCH).unwrap().as_secs());

    // Groups: select moves a selector and tells the switch (sail tells
    // only fallback and url-test switches).
    // (Fixing a fallback and unfix come with the Engine's pin, on the
    // translation's fallback groups.)
    let pick = runtime
        .groups()
        .await
        .unwrap()
        .into_iter()
        .find(|g| g.tag == "pick")
        .expect("pick");
    assert_eq!(
        (pick.now.as_str(), pick.fixed, pick.members.len()),
        ("direct", false, 2)
    );
    runtime.select("pick", "direct-b").await.unwrap();
    let switch = tokio::time::timeout(WAIT, switches.recv())
        .await
        .expect("a switch")
        .unwrap();
    assert_eq!(
        (
            switch.group.as_str(),
            switch.from.as_str(),
            switch.to.as_str(),
            switch.reason.as_str()
        ),
        ("pick", "direct", "direct-b", "selected")
    );
    assert!(runtime
        .groups()
        .await
        .unwrap()
        .iter()
        .any(|g| g.tag == "pick" && g.now == "direct-b" && !g.fixed));

    // Users replaced in place: the listener stays, the old user is refused.
    runtime
        .replace_inbound_users("local", vec![("u2".into(), p2.clone())])
        .await
        .unwrap();
    assert!(socks(port, "u1", &p1, echo).await.is_none(), "u1 is gone");
    let mut kept = socks(port, "u2", &p2, echo)
        .await
        .expect("u2 through the proxy");
    round_trip(&mut kept, b"as u2").await;

    // A reload keeps the listener and open connections; a failed one
    // changes nothing.
    runtime
        .reload(&config(port, &[("u2", &p2)], true))
        .await
        .unwrap();
    round_trip(&mut kept, b"after the reload").await;
    let err = runtime
        .reload("{\"outbounds\":[{\"type\":\"nope\"}]}")
        .await
        .unwrap_err();
    assert_eq!(err.code, "config");
    assert_eq!(runtime.state(), RuntimeState::Running);
    round_trip(&mut kept, b"after a failed reload").await;

    // sail's lines arrive as logfmt.
    let line = tokio::time::timeout(WAIT, logs.recv())
        .await
        .expect("a log line")
        .unwrap();
    assert!(
        line.contains(" level=") && line.ends_with(" source=sail"),
        "{line}"
    );

    runtime.stop().await.unwrap();
    wait_for(&mut states, RuntimeState::Stopped).await;
    assert!(
        TcpStream::connect(("127.0.0.1", port)).await.is_err(),
        "the listener is closed"
    );
    let err = runtime.traffic().await.unwrap_err();
    assert_eq!(err.code, "not_running");
    // Dials are taken while it starts (sail 47a1cc34), never once stopped.
    let err = runtime
        .dial_tcp("direct", Target::Addr(echo), WAIT)
        .await
        .err()
        .expect("stopped");
    assert_eq!(err.code, "not_running");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_configuration_that_does_not_build_does_not_start() {
    let runtime = SailRuntime::new(options("bad")).unwrap();
    let err = runtime
        .start("{\"outbounds\":[{\"type\":\"nope\"}]}")
        .await
        .unwrap_err();
    assert_eq!(err.code, "config");
    assert_ne!(runtime.state(), RuntimeState::Running);
    assert_eq!(
        err.to_error().code,
        crate::error::codes::CORE_OPERATION_FAILED
    );
}

/// A resolver that answers every query with its own question (QR set).
async fn mirror(addr: SocketAddr) -> Option<SocketAddr> {
    let socket = tokio::net::UdpSocket::bind(addr).await.ok()?;
    let bound = socket.local_addr().ok()?;
    tokio::spawn(async move {
        let mut buffer = [0u8; 1500];
        while let Ok((n, peer)) = socket.recv_from(&mut buffer).await {
            buffer[2] |= 0x80;
            let _ = socket.send_to(&buffer[..n], peer).await;
        }
    });
    Some(bound)
}

// dns-local's queries go through sail's direct outbound (its dialer binds
// the default interface): IPv4, IPv6 and a mixed list, and a link-local
// server by its scope where the host has one (macOS's lo0 has fe80::1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dns_local_dials_through_the_direct_outbound() {
    use crate::localdns::exchange::{exchange, Dial, RuntimeDial};
    use hickory_proto::op::{Message, MessageType, OpCode, Query};
    use hickory_proto::rr::{Name, RecordType};

    let runtime: Arc<SailRuntime> = Arc::new(SailRuntime::new(options("dial-dns")).unwrap());
    runtime
        .start(&config(free_port(), &[], false))
        .await
        .unwrap();
    let dial: Arc<dyn Dial> = Arc::new(RuntimeDial {
        runtime: runtime.clone(),
        outbound: "direct".into(),
    });
    let mut query = Message::new(0x4b4b, MessageType::Query, OpCode::Query);
    query.add_query(Query::query(
        Name::from_ascii("dial.example.").unwrap(),
        RecordType::A,
    ));

    let v4 = mirror("127.0.0.1:0".parse().unwrap())
        .await
        .expect("v4 loopback");
    let (_, from) = exchange(&dial, &[v4], &query).await.unwrap();
    assert_eq!(from, v4);
    if let Some(v6) = mirror("[::1]:0".parse().unwrap()).await {
        let (_, from) = exchange(&dial, &[v6], &query).await.unwrap();
        assert_eq!(from, v6);
        let (_, from) = exchange(&dial, &[v6, v4], &query).await.unwrap();
        assert_eq!(from, v6, "in order");
    }
    // A link-local resolver by its scope id, where loopback has one.
    #[cfg(target_os = "macos")]
    if let Some(scoped) = mirror("[fe80::1%1]:0".parse().unwrap()).await {
        let (_, from) = exchange(&dial, &[scoped], &query).await.unwrap();
        assert_eq!(from, scoped);
    }
    runtime.stop().await.unwrap();
}

// The network as sail sees it (instance.network()), and its changes as
// sail::embed's network events (here a wake, which sail announces whatever
// the state: the same interface, another network, so `moved`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_changes_are_sails_own() {
    use sail::net::network::ChangeReason;

    let runtime = SailRuntime::new(options("network")).unwrap();
    let mut changes = runtime.network_changes();
    assert_eq!(runtime.network(), None, "not running");
    runtime
        .start(&config(free_port(), &[], false))
        .await
        .unwrap();
    // Settled at start (sail b3533615): the default interface, or offline,
    // at generation 1, told by no event.
    assert!(runtime.network().is_some());
    assert_eq!(*changes.borrow_and_update(), None, "no change yet");

    // The subscription follows the start (sail subscribes once the run is
    // up): announce until a change comes through.
    tokio::time::timeout(WAIT, async {
        loop {
            runtime
                .instance
                .manager()
                .unwrap()
                .network()
                .announce(ChangeReason::Wake);
            if tokio::time::timeout(Duration::from_millis(200), changes.changed())
                .await
                .is_ok()
            {
                return;
            }
        }
    })
    .await
    .expect("a change");
    let change = changes.borrow_and_update().clone().expect("the change");
    assert_eq!(
        (change.change.as_str(), change.reason.as_str()),
        ("moved", "wake")
    );
    assert!(change.generation >= 2, "after the start's: {change:?}");
    assert_eq!(change.old, change.new, "announced: the state did not move");

    runtime.stop().await.unwrap();
    assert_eq!(runtime.network(), None);
}

/// A SOCKS5 connection without authentication through `port` to `to`.
async fn socks_open(port: u16, to: SocketAddr) -> Option<TcpStream> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    s.write_all(&[5, 1, 0]).await.ok()?;
    let mut reply = [0u8; 2];
    s.read_exact(&mut reply).await.ok()?;
    let SocketAddr::V4(v4) = to else {
        unreachable!("loopback v4")
    };
    let mut connect = vec![5, 1, 0, 1];
    connect.extend_from_slice(&v4.ip().octets());
    connect.extend_from_slice(&v4.port().to_be_bytes());
    s.write_all(&connect).await.ok()?;
    let mut head = [0u8; 10];
    s.read_exact(&mut head).await.ok()?;
    (head[1] == 0).then_some(s)
}

// An inbound added while running listens (a reload would not add it); its
// removal stops the listener and closes its own connections only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbounds_added_and_removed_while_running() {
    let echo = echo().await;
    let (main_port, extra_port) = (free_port(), free_port());
    let runtime = SailRuntime::new(options("inbounds")).unwrap();
    runtime.start(&config(main_port, &[], false)).await.unwrap();
    let mut kept = socks_open(main_port, echo)
        .await
        .expect("the configured inbound");
    round_trip(&mut kept, b"configured").await;

    let extra = serde_json::json!({ "type": "mixed", "tag": "extra", "listen": "127.0.0.1", "listen_port": extra_port });
    runtime.add_inbound(&extra.to_string()).await.unwrap();
    let mut through_extra = socks_open(extra_port, echo)
        .await
        .expect("the added inbound listens");
    round_trip(&mut through_extra, b"added").await;
    assert!(
        runtime.add_inbound(&extra.to_string()).await.is_err(),
        "a tag in use"
    );

    runtime.remove_inbound("extra").await.unwrap();
    assert!(
        TcpStream::connect(("127.0.0.1", extra_port)).await.is_err(),
        "no longer listening"
    );
    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(WAIT, through_extra.read(&mut buf))
        .await
        .expect("closed in time");
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "its connection is closed: {read:?}"
    );
    round_trip(&mut kept, b"the other inbound's connection stays").await;
    assert!(
        runtime.remove_inbound("extra").await.is_err(),
        "gone already"
    );
    runtime.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_connection_is_told() {
    let port = free_port();
    let p1 = password();
    let runtime = SailRuntime::new(options("dial-failed")).unwrap();
    let mut failures = runtime.dial_failures();
    runtime
        .start(&config(port, &[("u1", &p1)], false))
        .await
        .unwrap();

    // Nothing listens there: the direct outbound's connect is refused.
    let closed: SocketAddr = ([127, 0, 0, 1], free_port()).into();
    let _ = socks(port, "u1", &p1, closed).await;
    let failed = tokio::time::timeout(WAIT, failures.recv())
        .await
        .expect("a failure in time")
        .unwrap();
    assert_eq!(
        (failed.stage.as_str(), failed.error.as_str()),
        ("dial", "ConnectionRefused"),
        "{failed:?}"
    );
    // The route's outbound, then the member the selector took (2eb3fe47);
    // a selector has no other member to try.
    assert_eq!(
        (failed.chain.as_str(), failed.more_to_try),
        ("pick>direct", false),
        "{failed:?}"
    );
    assert!(failed.count >= 1, "{failed:?}");
    runtime.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_routed_connection_is_told_once_taken() {
    let echo = echo().await;
    let port = free_port();
    let p1 = password();
    let runtime = SailRuntime::new(options("routed")).unwrap();
    let mut routes = runtime.routes();
    runtime
        .start(&config(port, &[("u1", &p1)], false))
        .await
        .unwrap();
    let mut proxied = socks(port, "u1", &p1, echo)
        .await
        .expect("through the proxy");
    round_trip(&mut proxied, b"routed").await;
    let routed = tokio::time::timeout(WAIT, async {
        loop {
            let r = routes.recv().await.expect("the channel");
            if r.destination == echo.to_string() {
                return r;
            }
        }
    })
    .await
    .expect("routed in time");
    // route.final is the selector pick, on direct: outermost first, the
    // last the outbound that carried it.
    assert_eq!(
        (
            routed.network.as_str(),
            routed.inbound.as_str(),
            routed.action.as_str(),
            routed.rule,
            routed.chain.clone(),
            routed.error.as_deref(),
        ),
        (
            "tcp",
            "local",
            "outbound",
            None,
            vec!["pick".to_owned(), "direct".to_owned()],
            None
        ),
        "{routed:?}"
    );
    assert!(routed.connect_ms.is_some(), "{routed:?}");
    runtime.stop().await.unwrap();
    assert!(runtime.stop_leftovers().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_instance_s_own_dns_query_is_told_once_taken() {
    let port = free_port();
    let p1 = password();
    let runtime = SailRuntime::new(options("dns-exchange")).unwrap();
    let mut exchanges = runtime.dns_exchanges();
    runtime
        .start(&config(port, &[("u1", &p1)], false))
        .await
        .unwrap();
    // A name the proxy dials goes through sail's DNS (the system's
    // resolver, as the configuration has no dns section); .invalid never
    // resolves, so the exchange is told whatever the network.
    let _ = socks_to_domain(port, "u1", &p1, "dns-exchange.invalid", 80).await;
    let exchange = tokio::time::timeout(WAIT, async {
        loop {
            let e = exchanges.recv().await.expect("the channel");
            if e.name == "dns-exchange.invalid" {
                return e;
            }
        }
    })
    .await
    .expect("told in time");
    assert!(exchange.for_instance, "{exchange:?}");
    assert!(
        exchange.rcode.is_some() || exchange.error.is_some(),
        "answered or failed: {exchange:?}"
    );
    runtime.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chains_are_outermost_first_through_nested_groups() {
    let echo = echo().await;
    let port = free_port();
    let p1 = password();
    // A selector in a selector: outer takes pick, pick takes direct.
    let config = serde_json::json!({
        "log": { "level": "info" },
        "inbounds": [{ "type": "mixed", "tag": "local", "listen": "127.0.0.1", "listen_port": port,
                       "users": [{ "username": "u1", "password": p1 }] }],
        "outbounds": [
            { "type": "direct", "tag": "direct" },
            { "type": "selector", "tag": "pick", "outbounds": ["direct"], "default": "direct" },
            { "type": "selector", "tag": "outer", "outbounds": ["pick"], "default": "pick" }
        ],
        "route": { "final": "outer" }
    })
    .to_string();
    let runtime = SailRuntime::new(options("nested")).unwrap();
    let mut routes = runtime.routes();
    runtime.start(&config).await.unwrap();
    let mut proxied = socks(port, "u1", &p1, echo)
        .await
        .expect("through the proxy");
    round_trip(&mut proxied, b"nested").await;
    let want = ["outer", "pick", "direct"].map(String::from).to_vec();

    // connections(): sail's Clash-shaped chains, turned.
    let open = runtime.connections().await.unwrap();
    let c = open
        .iter()
        .find(|c| c.destination == echo.to_string())
        .expect("the proxied connection");
    assert_eq!(c.chain, want, "connections()");

    // Routed: the same order.
    let routed = tokio::time::timeout(WAIT, async {
        loop {
            let r = routes.recv().await.expect("the channel");
            if r.destination == echo.to_string() {
                return r;
            }
        }
    })
    .await
    .expect("routed in time");
    assert_eq!(routed.chain, want, "routed");
    runtime.stop().await.unwrap();
}

/// `config` with one more mixed inbound, `extra` on `port`.
fn with_extra(config: &str, port: u16) -> String {
    let mut config: serde_json::Value = serde_json::from_str(config).unwrap();
    config["inbounds"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "type": "mixed", "tag": "extra", "listen": "127.0.0.1", "listen_port": port }));
    config.to_string()
}

// sail's reload runs the configuration's inbounds (2f967b1a): compared by
// tag with those running, an inbound added at run time (add_inbound) among
// them. The Engine's reloads carry what it added (the local and system
// proxy listeners are in its translation while open).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_runs_the_configuration_s_inbounds() {
    let (port, extra_port) = (free_port(), free_port());
    let p1 = password();
    let base = config(port, &[("u1", &p1)], false);
    let extra = serde_json::json!({ "type": "mixed", "tag": "extra", "listen": "127.0.0.1", "listen_port": extra_port });

    // 1. The reload names the added inbound: untouched, still listening.
    let runtime = SailRuntime::new(options("reload-with")).unwrap();
    runtime.start(&base).await.unwrap();
    runtime.add_inbound(&extra.to_string()).await.unwrap();
    let report = runtime
        .reload(&with_extra(&base, extra_port))
        .await
        .expect("a reload that names the added inbound");
    assert!(
        report
            .inbounds
            .contains(&("extra".into(), "untouched".into())),
        "{report:?}"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", extra_port)).await.is_ok(),
        "the added listener stays"
    );
    runtime.stop().await.unwrap();

    // 2. The reload leaves it out: it is removed, the rest goes on.
    let runtime = SailRuntime::new(options("reload-without")).unwrap();
    runtime.start(&base).await.unwrap();
    runtime.add_inbound(&extra.to_string()).await.unwrap();
    let report = runtime
        .reload(&base)
        .await
        .expect("a reload without the added inbound");
    assert!(
        report
            .inbounds
            .contains(&("extra".into(), "removed".into()))
            && report
                .inbounds
                .contains(&("local".into(), "untouched".into())),
        "{report:?}"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", extra_port)).await.is_err(),
        "the removed listener is closed"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", port)).await.is_ok(),
        "the configured one goes on"
    );
    runtime.stop().await.unwrap();

    // 3. Removed first: the reload without it has nothing to remove.
    let runtime = SailRuntime::new(options("reload-removed")).unwrap();
    runtime.start(&base).await.unwrap();
    runtime.add_inbound(&extra.to_string()).await.unwrap();
    runtime.remove_inbound("extra").await.unwrap();
    let report = runtime
        .reload(&base)
        .await
        .expect("a reload after remove_inbound, without it");
    assert_eq!(report.inbounds, [("local".into(), "untouched".into())]);
    runtime.stop().await.unwrap();
}

// sail waits for a host's dials when it stops (e7bfe39c: they are tasks of
// the instance's scope): a dial that hangs must not hold the stop. The dial
// goes through a socks outbound whose server accepts and never answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hanging_dial_does_not_hold_the_stop() {
    let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_port = silent.local_addr().unwrap().port();
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((conn, _)) = silent.accept().await {
            open.push(conn);
        }
    });
    let port = free_port();
    let p1 = password();
    let mut config: serde_json::Value =
        serde_json::from_str(&config(port, &[("u1", &p1)], false)).unwrap();
    config["outbounds"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "type": "socks", "tag": "silent", "server": "127.0.0.1", "server_port": silent_port }));
    let runtime = Arc::new(SailRuntime::new(options("hanging-dial")).unwrap());
    runtime.start(&config.to_string()).await.unwrap();

    let dialler = runtime.clone();
    let dial = tokio::spawn(async move {
        dialler
            .dial_tcp(
                "silent",
                Target::Domain("example.invalid".into(), 80),
                Duration::from_secs(60),
            )
            .await
            .map(|_| ())
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!dial.is_finished(), "the dial hangs on the silent server");

    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(10), runtime.stop())
        .await
        .expect("stop returns in time")
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    let dialled = tokio::time::timeout(WAIT, dial)
        .await
        .expect("the dial ends with the stop")
        .unwrap();
    assert!(dialled.is_err(), "the hanging dial fails: {dialled:?}");
    held.abort();
}

/// A reload that changes the route's interface options takes effect for
/// new connections (sail says reload takes them; #221): bound to an
/// interface that does not exist, a direct dial fails; unbound again, it
/// connects.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_changes_the_default_interface() {
    let echo = echo().await;
    let runtime = SailRuntime::new(options("default-interface")).unwrap();
    let port = free_port();
    let plain = config(port, &[], false);
    runtime.start(&plain).await.unwrap();
    assert!(runtime
        .dial_tcp("direct", Target::Addr(echo), WAIT)
        .await
        .is_ok());

    let mut bound: serde_json::Value = serde_json::from_str(&plain).unwrap();
    bound["route"]["default_interface"] = "ppvpn-none0".into();
    runtime.reload(&bound.to_string()).await.unwrap();
    assert!(
        runtime
            .dial_tcp("direct", Target::Addr(echo), WAIT)
            .await
            .is_err(),
        "a dial after the reload binds to the missing interface"
    );

    runtime.reload(&plain).await.unwrap();
    assert!(runtime
        .dial_tcp("direct", Target::Addr(echo), WAIT)
        .await
        .is_ok());
    runtime.stop().await.unwrap();
}
