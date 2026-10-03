//! SailRuntime against a real sail instance: a mixed inbound (the local
//! proxy's shape: SOCKS5 with users) and direct outbounds behind a selector,
//! dialling an echo server on loopback.

use std::net::SocketAddr;

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

    // Groups: select moves a selector and the poll reports the switch.
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
    let switch = tokio::time::timeout(GROUP_POLL * 3, switches.recv())
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
