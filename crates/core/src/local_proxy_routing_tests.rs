//! The shared local proxy on a real sail, with the translated
//! configuration (the contract's base profile, two nodes behind one port):
//! the username picks the node, over HTTP and SOCKS5, and credentials it
//! does not know are refused. As in `golden_routing_tests`, every node's
//! outbound dials a closed port on loopback (`bind_loopback`), so nothing
//! leaves the host; the node a connection went to is read off sail's
//! Routed event, told once the dial ends.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::golden_routing_tests::{bind_loopback, connect_open, free_port, golden_now, password};
use crate::profile::{self, Profile};
use crate::runtime::sail::SailRuntime;
use crate::runtime::{Routed, Runtime};
use crate::translate::{self, LocalProxy, Options, Translation, LOCAL_PROXY_INBOUND_TAG};

const WAIT: Duration = Duration::from_secs(5);

fn base() -> Profile {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/golden/contract/profiles/base.json");
    let p = profile::parse(&std::fs::read(path).unwrap()).unwrap();
    profile::validate(&p, golden_now()).unwrap();
    p
}

/// The translation of `p` with `proxy`, and its configuration with every
/// dial bound to fail on loopback.
fn translated(p: &Profile, proxy: &LocalProxy) -> (Translation, String) {
    let options = Options {
        local_proxy: Some(proxy.clone()),
        ..Options::default()
    };
    let translation = translate::translate(p, &options).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&translation.json).unwrap();
    bind_loopback(&mut config);
    (translation, config.to_string())
}

/// The Routed event of the connection to `destination`.
async fn routed_to(routes: &mut mpsc::Receiver<Routed>, destination: &str) -> Routed {
    tokio::time::timeout(WAIT, async {
        loop {
            let r = routes.recv().await.expect("the channel");
            if r.destination == destination {
                return r;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{destination} routed in time"))
}

/// The node a routed connection went to, by the translation's tag map.
fn node_of(t: &Translation, routed: &Routed) -> Option<String> {
    if routed.action != "outbound" {
        return None;
    }
    routed
        .chain
        .iter()
        .find_map(|tag| t.outbound_nodes.get(tag))
        .cloned()
}

/// Whether an HTTP CONNECT to `destination` (as `user`:`password`, or
/// without credentials) is answered with a 407.
async fn http_challenged(port: u16, credentials: Option<(&str, &str)>, destination: &str) -> bool {
    let credentials = credentials.map(|(user, password)| format!("{user}:{password}"));
    let mut stream = connect_open(port, credentials.as_deref(), destination, &[])
        .await
        .expect("connected");
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(WAIT, stream.read_to_end(&mut answer))
        .await
        .expect("answered and closed in time");
    answer.starts_with(b"HTTP/1.1 407 ")
}

/// A SOCKS5 greeting and username/password authentication as `user`, then,
/// once authenticated, a CONNECT to `destination`: the RFC 1929 status and
/// the stream.
async fn socks_connect(
    port: u16,
    user: &str,
    password: &str,
    destination: SocketAddrV4,
) -> (u8, TcpStream) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(&[5, 1, 2]).await.unwrap();
    let mut reply = [0u8; 2];
    tokio::time::timeout(WAIT, s.read_exact(&mut reply))
        .await
        .expect("the method in time")
        .unwrap();
    assert_eq!(reply, [5, 2], "username/password method");
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(password.len() as u8);
    auth.extend_from_slice(password.as_bytes());
    s.write_all(&auth).await.unwrap();
    tokio::time::timeout(WAIT, s.read_exact(&mut reply))
        .await
        .expect("the authentication's status in time")
        .unwrap();
    if reply[1] == 0 {
        let mut connect = vec![5, 1, 0, 1];
        connect.extend_from_slice(&destination.ip().octets());
        connect.extend_from_slice(&destination.port().to_be_bytes());
        s.write_all(&connect).await.unwrap();
    }
    (reply[1], s)
}

/// Go: internal/runtime TestSharedLocalProxyRoutesByUsername.
///
/// Two nodes behind one port: each node's username routes to that node,
/// over HTTP CONNECT and SOCKS5, whatever the profile's rules say; a wrong
/// or empty password, an unknown node, another prefix and no credentials
/// are refused; after a reload without a node, that node's username is
/// refused and the other's still routes. What needs a node that answers is
/// left out: the data through the node, the traffic counted per node, the
/// backup ingress taking over, and the availability probe.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shared_local_proxy_routes_by_username() {
    let p = base();
    let (alpha, beta) = (p.nodes[0].id.clone(), p.nodes[1].id.clone());
    let proxy = LocalProxy {
        listen: "127.0.0.1".into(),
        port: free_port(),
        prefix: "lp0".into(),
        password: password(),
    };
    let port = proxy.port;
    let (translation, config) = translated(&p, &proxy);
    let dir = std::env::temp_dir().join(format!(
        "ppvpn-core-local-proxy-routing-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let runtime = SailRuntime::new(
        sail::embed::Options::new()
            .data_dir(dir.clone())
            .threads(sail::embed::Threads::One),
    )
    .unwrap();
    let mut routes = runtime.routes();
    runtime.start(&config).await.unwrap();

    // Each connection its own destination (documentation addresses: the
    // node outbounds dial loopback, never these), to find its event by.
    let mut next = 20_000u16;
    let mut destination = || {
        next += 1;
        SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), next)
    };

    // The username picks the node, over either protocol.
    for (node, http) in [
        (&alpha, true),
        (&beta, false),
        (&beta, true),
        (&alpha, false),
    ] {
        let to = destination();
        let user = proxy.username(node);
        let _held = if http {
            let credentials = format!("{user}:{}", proxy.password);
            connect_open(port, Some(&credentials), &to.to_string(), &[])
                .await
                .expect("connected")
        } else {
            let (status, stream) = socks_connect(port, &user, &proxy.password, to).await;
            assert_eq!(status, 0, "SOCKS5 as {node}: authenticated");
            stream
        };
        let routed = routed_to(&mut routes, &to.to_string()).await;
        let protocol = if http { "HTTP" } else { "SOCKS5" };
        assert_eq!(
            (
                routed.inbound.as_str(),
                node_of(&translation, &routed).as_deref()
            ),
            (LOCAL_PROXY_INBOUND_TAG, Some(node.as_str())),
            "{protocol} as {node}: {routed:?}"
        );
    }

    // Credentials the proxy does not know.
    let alpha_user = proxy.username(&alpha);
    let refused = [
        ("wrong password", alpha_user.clone(), password()),
        ("empty password", alpha_user.clone(), String::new()),
        (
            "unknown node",
            proxy.username("missing"),
            proxy.password.clone(),
        ),
        (
            "other prefix",
            format!("zzzzz-{alpha}"),
            proxy.password.clone(),
        ),
    ];
    for (name, user, secret) in &refused {
        assert!(
            http_challenged(
                port,
                Some((user.as_str(), secret.as_str())),
                &destination().to_string()
            )
            .await,
            "HTTP {name}"
        );
        if secret.is_empty() {
            continue; // RFC 1929 cannot carry an empty password portably
        }
        let (status, _) = socks_connect(port, user, secret, destination()).await;
        assert_ne!(status, 0, "SOCKS5 {name}");
    }
    assert!(
        http_challenged(port, None, &destination().to_string()).await,
        "HTTP without credentials"
    );

    // A reload without beta: its username is refused, alpha keeps the same
    // port and credentials.
    let mut without_beta = p.clone();
    without_beta.nodes.retain(|n| n.id != beta);
    without_beta
        .routing
        .rules
        .retain(|r| r.action.node_id != beta);
    profile::validate(&without_beta, golden_now()).unwrap();
    let (after, config) = translated(&without_beta, &proxy);
    assert!(
        !after.outbound_nodes.values().any(|n| *n == beta),
        "{after:?}"
    );
    runtime.reload(&config).await.unwrap();
    let beta_user = proxy.username(&beta);
    assert!(
        http_challenged(
            port,
            Some((beta_user.as_str(), proxy.password.as_str())),
            &destination().to_string()
        )
        .await,
        "HTTP as the removed node"
    );
    let (status, _) = socks_connect(port, &beta_user, &proxy.password, destination()).await;
    assert_ne!(status, 0, "SOCKS5 as the removed node");
    let to = destination();
    let (status, _held) = socks_connect(port, &alpha_user, &proxy.password, to).await;
    assert_eq!(status, 0, "alpha after the reload");
    let routed = routed_to(&mut routes, &to.to_string()).await;
    assert_eq!(
        node_of(&after, &routed).as_deref(),
        Some(alpha.as_str()),
        "{routed:?}"
    );

    runtime.stop().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
