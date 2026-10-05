//! Class B rows of docs/rust-parity.md that need more than one routing
//! decision, on a real sail with the translated configuration (the
//! contract's base profile): what a reload or a select leaves to the
//! connections already open, the system proxy's traffic and listener, a
//! local rule set on the TUN route, and sail's reverse mapping across a
//! reload. As in `golden_routing_tests`, nothing leaves the host: the
//! nodes' outbounds dial a closed port on loopback, the TUN is a SOCKS
//! inbound carrying its tag, and what a connection did is read off sail's
//! Routed event, told once its dial ended.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::time::Duration;

use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;

use crate::config::Platform;
use crate::golden_routing_tests::{
    bind_loopback, client_hello, free_port, golden_now, password, socks_open, stand_in_for_the_tun,
};
use crate::profile::{self, Profile, RoutingAction, RoutingMatch, RoutingRule, RuleSet};
use crate::request::RoutingMode;
use crate::runtime::sail::SailRuntime;
use crate::runtime::{Routed, Runtime};
use crate::translate::{
    self, interface_name, LocalDns, LocalProxy, Options, RuleSetFile, Translation, Tun, DIRECT_TAG,
    LOCAL_PROXY_INBOUND_TAG, SELECTED_TAG, SYSTEM_PROXY_INBOUND_TAG, TUN_INBOUND_TAG,
};

const WAIT: Duration = Duration::from_secs(5);

/// The name the fake DNS server answers for, and its addresses.
const DUAL: &str = "dual.example";
const DUAL_V4: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 50);
const DUAL_V6: Ipv6Addr = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x50);

fn base() -> Profile {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/golden/contract/profiles/base.json");
    let p = profile::parse(&std::fs::read(path).unwrap()).unwrap();
    profile::validate(&p, golden_now()).unwrap();
    p
}

fn local_proxy(prefix: &str) -> LocalProxy {
    LocalProxy {
        listen: "127.0.0.1".into(),
        port: free_port(),
        prefix: prefix.into(),
        password: password(),
    }
}

/// The TUN of a Linux desktop.
fn tun(no_host_ipv6_route: bool, local_dns: LocalDns) -> Tun {
    Tun {
        desktop: true,
        ipv6: true,
        no_host_ipv6_route,
        interface_name: interface_name(Platform::Linux).into(),
        local_dns,
    }
}

/// Every node's outbound dials `closed` on loopback, but `stand_in`'s,
/// which becomes a direct outbound under the node's tag: a node that
/// carries a connection to an echo server on loopback. `direct` is left as
/// it is: the tests that use this dial loopback alone.
fn fail_the_nodes(config: &str, stand_in: Option<&str>, closed: u16) -> String {
    let mut config: Value = serde_json::from_str(config).unwrap();
    for outbound in config["outbounds"].as_array_mut().unwrap() {
        if let Some(tag) = stand_in.filter(|tag| outbound["tag"] == *tag) {
            // An option the real direct lacks: sail shares one handler
            // between outbounds of identical options, and a connection
            // routed to `direct` would be reported under this tag.
            *outbound = json!({ "type": "direct", "tag": tag, "connect_timeout": "5s" });
            continue;
        }
        match outbound["type"].as_str().unwrap_or_default() {
            "selector" | "fallback" | "urltest" | "block" | "dns" | "direct" => {}
            _ => {
                outbound["server"] = "127.0.0.1".into();
                outbound["server_port"] = closed.into();
            }
        }
    }
    config.to_string()
}

struct Sail {
    runtime: SailRuntime,
    routes: mpsc::Receiver<Routed>,
    /// sail's data directory, removed after the stop.
    dir: PathBuf,
}

async fn start(name: &str, config: &str) -> Sail {
    let dir = std::env::temp_dir().join(format!("ppvpn-core-parity-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let runtime = SailRuntime::new(
        sail::embed::Options::new()
            .data_dir(dir.clone())
            .threads(sail::embed::Threads::One),
    )
    .unwrap();
    let routes = runtime.routes();
    runtime.start(config).await.unwrap();
    Sail {
        runtime,
        routes,
        dir,
    }
}

impl Sail {
    async fn stop(self) {
        self.runtime.stop().await.unwrap();
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    /// The Routed event of the connection to `port` (each connection its
    /// own).
    async fn routed_on(&mut self, port: u16) -> Routed {
        let suffix = format!(":{port}");
        tokio::time::timeout(WAIT, async {
            loop {
                let r = self.routes.recv().await.expect("the channel");
                if r.destination.ends_with(&suffix) {
                    return r;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the connection to port {port} routed in time"))
    }

    /// The chain `connections()` lists for the connection `id`.
    async fn chain_of(&self, id: u64) -> Vec<String> {
        self.runtime
            .connections()
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("connection {id} listed"))
            .chain
    }
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

/// An echo server on loopback.
async fn echo() -> SocketAddrV4 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let SocketAddr::V4(addr) = listener.local_addr().unwrap() else {
        unreachable!("loopback v4")
    };
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
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

async fn round_trip(s: &mut TcpStream, text: &[u8]) {
    s.write_all(text).await.unwrap();
    let mut got = vec![0u8; text.len()];
    tokio::time::timeout(WAIT, s.read_exact(&mut got))
        .await
        .expect("echo in time")
        .unwrap();
    assert_eq!(got, text);
}

/// A SOCKS5 CONNECT to `to` (as `user`, `password` when given): the
/// reply's status (0xff where none came) and the stream.
async fn socks5(port: u16, credentials: Option<(&str, &str)>, to: SocketAddrV4) -> (u8, TcpStream) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let method = if credentials.is_some() { 2 } else { 0 };
    s.write_all(&[5, 1, method]).await.unwrap();
    let mut reply = [0u8; 2];
    tokio::time::timeout(WAIT, s.read_exact(&mut reply))
        .await
        .expect("the method in time")
        .unwrap();
    assert_eq!(reply, [5, method], "the SOCKS5 method");
    if let Some((user, password)) = credentials {
        let mut auth = vec![1, user.len() as u8];
        auth.extend_from_slice(user.as_bytes());
        auth.push(password.len() as u8);
        auth.extend_from_slice(password.as_bytes());
        s.write_all(&auth).await.unwrap();
        tokio::time::timeout(WAIT, s.read_exact(&mut reply))
            .await
            .expect("the authentication's status in time")
            .unwrap();
        assert_eq!(reply, [1, 0], "authenticated as {user}");
    }
    let mut connect = vec![5, 1, 0, 1];
    connect.extend_from_slice(&to.ip().octets());
    connect.extend_from_slice(&to.port().to_be_bytes());
    s.write_all(&connect).await.unwrap();
    let mut head = [0u8; 10];
    let status = match tokio::time::timeout(WAIT, s.read_exact(&mut head))
        .await
        .expect("the CONNECT reply in time")
    {
        Ok(_) => head[1],
        Err(_) => 0xff,
    };
    (status, s)
}

/// Go: internal/runtime TestRoutedLocalProxyUserFollowsProfileRules.
///
/// What the routing golden leaves out: select-node and routing_mode change
/// new connections only. Through the local proxy's routed user, a
/// connection to an echo server on loopback goes direct (the profile's
/// bypass-private rule); a reload into the global mode, which drops that
/// rule, leaves it open and direct, while a new connection there goes to
/// the selected node. Then, with the second node standing in as a direct
/// outbound under its tag, a connection through it stays on it, with the
/// same chain, across a select of the first node, which new connections
/// take. What needs a node that answers (traffic through it) is left out;
/// the routing decisions themselves are the golden's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_node_and_routing_mode_change_new_connections_only() {
    let p = base();
    let (alpha, beta) = (p.nodes[0].id.clone(), p.nodes[1].id.clone());
    let proxy = local_proxy("lp1");
    let routed_user = proxy.username("");
    let credentials = Some((routed_user.as_str(), proxy.password.as_str()));
    let closed = free_port();
    let build = |mode: RoutingMode| {
        let options = Options {
            mode,
            local_proxy: Some(proxy.clone()),
            ..Options::default()
        };
        let t = translate::translate(&p, &options).unwrap();
        let config = fail_the_nodes(&t.json, Some(t.node_tags[&beta].as_str()), closed);
        (t, config)
    };
    let (rules, config) = build(RoutingMode::Rules);
    let (alpha_tag, beta_tag) = (
        rules.node_tags[&alpha].clone(),
        rules.node_tags[&beta].clone(),
    );
    assert!(
        !rules.groups.contains_key(&beta_tag),
        "the stand-in is a node of one ingress: {rules:?}"
    );
    let mut sail = start("modes", &config).await;

    // The rules mode: loopback goes direct.
    let first = echo().await;
    let (status, mut kept_direct) = socks5(proxy.port, credentials, first).await;
    assert_eq!(status, 0, "direct to the echo server");
    let routed = sail.routed_on(first.port()).await;
    assert_eq!(routed.inbound, LOCAL_PROXY_INBOUND_TAG, "{routed:?}");
    assert_eq!(routed.chain, [DIRECT_TAG], "{routed:?}");
    let direct_id = routed.id.expect("an open connection");
    round_trip(&mut kept_direct, b"direct, before the reload").await;

    // The global mode: the rule is gone for new connections alone.
    let (global, config) = build(RoutingMode::Global);
    let report = sail.runtime.reload(&config).await.unwrap();
    assert_eq!(report.path, "full", "{report:?}");
    round_trip(&mut kept_direct, b"direct, after the reload").await;
    assert_eq!(sail.chain_of(direct_id).await, [DIRECT_TAG]);
    let second = echo().await;
    let (_, _failed) = socks5(proxy.port, credentials, second).await;
    let routed = sail.routed_on(second.port()).await;
    assert_eq!(
        (
            routed.chain.first().map(String::as_str),
            node_of(&global, &routed).as_deref()
        ),
        (Some(SELECTED_TAG), Some(alpha.as_str())),
        "{routed:?}"
    );

    // select-node: the stand-in node, then back to the first.
    sail.runtime.select(SELECTED_TAG, &beta_tag).await.unwrap();
    let third = echo().await;
    let (status, mut kept_node) = socks5(proxy.port, credentials, third).await;
    assert_eq!(status, 0, "through the stand-in node");
    let routed = sail.routed_on(third.port()).await;
    assert_eq!(
        routed.chain,
        [SELECTED_TAG, beta_tag.as_str()],
        "{routed:?}"
    );
    let node_id = routed.id.expect("an open connection");
    round_trip(&mut kept_node, b"through the stand-in").await;
    sail.runtime.select(SELECTED_TAG, &alpha_tag).await.unwrap();
    round_trip(&mut kept_node, b"still through the stand-in").await;
    assert_eq!(
        sail.chain_of(node_id).await,
        [SELECTED_TAG, beta_tag.as_str()]
    );
    let fourth = echo().await;
    let (_, _failed) = socks5(proxy.port, credentials, fourth).await;
    let routed = sail.routed_on(fourth.port()).await;
    assert_eq!(
        node_of(&global, &routed).as_deref(),
        Some(alpha.as_str()),
        "{routed:?}"
    );
    round_trip(&mut kept_direct, b"direct, at the end").await;

    sail.stop().await;
}

/// Go: internal/runtime TestSystemProxyFollowsSelectedNodeAndRules.
///
/// What the routing golden leaves out: the system proxy, turned on by an
/// inbounds-only reload, carries a connection (direct to an echo server on
/// loopback, the profile's bypass-private rule) whose bytes count in
/// `traffic()` and in its `connections()` entry; turned off the same way,
/// its listener stops accepting connections, while a connection through
/// the local proxy keeps working across both toggles. What needs a node
/// that answers (traffic through the selected node) is left out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_system_proxy_counts_its_traffic_and_closes_when_turned_off() {
    let p = base();
    let proxy = local_proxy("lp2");
    let routed_user = proxy.username("");
    let credentials = Some((routed_user.as_str(), proxy.password.as_str()));
    let (system_port, closed) = (free_port(), free_port());
    let build = |system_proxy_port: Option<u16>| {
        let options = Options {
            local_proxy: Some(proxy.clone()),
            system_proxy_port,
            ..Options::default()
        };
        let t = translate::translate(&p, &options).unwrap();
        fail_the_nodes(&t.json, None, closed)
    };
    let mut sail = start("system-proxy", &build(None)).await;
    let local_echo = echo().await;
    let (status, mut kept) = socks5(proxy.port, credentials, local_echo).await;
    assert_eq!(status, 0, "through the local proxy");
    round_trip(&mut kept, b"before the system proxy").await;

    let report = sail
        .runtime
        .reload(&build(Some(system_port)))
        .await
        .unwrap();
    assert_eq!(report.path, "inbounds_only", "{report:?}");
    assert!(
        report
            .inbounds
            .contains(&(SYSTEM_PROXY_INBOUND_TAG.into(), "added".into())),
        "{report:?}"
    );
    let before = sail.runtime.traffic().await.unwrap();
    let system_echo = echo().await;
    let (status, mut through) = socks5(system_port, None, system_echo).await;
    assert_eq!(status, 0, "through the system proxy");
    let routed = sail.routed_on(system_echo.port()).await;
    assert_eq!(routed.inbound, SYSTEM_PROXY_INBOUND_TAG, "{routed:?}");
    assert_eq!(routed.chain, [DIRECT_TAG], "{routed:?}");
    let id = routed.id.expect("an open connection");
    const SIZE: u64 = 4096;
    round_trip(&mut through, &[0x5a; SIZE as usize]).await;
    // Counted once the write to the client returns, which can be just
    // after the client has read the bytes.
    let counted = tokio::time::timeout(WAIT, async {
        loop {
            let total = sail.runtime.traffic().await.unwrap();
            let connection = sail
                .runtime
                .connections()
                .await
                .unwrap()
                .into_iter()
                .find(|c| c.id == id);
            if total.upload_bytes >= before.upload_bytes + SIZE
                && total.download_bytes >= before.download_bytes + SIZE
                && connection
                    .as_ref()
                    .is_some_and(|c| c.upload_bytes >= SIZE && c.download_bytes >= SIZE)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        counted.is_ok(),
        "traffic not counted: before {before:?}, now {:?}, connections {:?}",
        sail.runtime.traffic().await,
        sail.runtime.connections().await
    );
    round_trip(&mut kept, b"with the system proxy").await;

    let report = sail.runtime.reload(&build(None)).await.unwrap();
    assert_eq!(report.path, "inbounds_only", "{report:?}");
    assert!(
        report
            .inbounds
            .contains(&(SYSTEM_PROXY_INBOUND_TAG.into(), "removed".into())),
        "{report:?}"
    );
    // sail aborts a removed inbound's listener task without waiting for it,
    // so the socket closes shortly after the reload returns.
    tokio::time::timeout(WAIT, async {
        while TcpStream::connect(("127.0.0.1", system_port)).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the system proxy no longer accepts connections");
    round_trip(&mut kept, b"after the system proxy").await;

    sail.stop().await;
}

/// Go: internal/runtime TestTUNRuleSetDomainGoesDirect.
///
/// A local binary rule set (rulesets testdata `domains.srs`: the suffix
/// `cn.example`) named by a profile rule whose action is direct, on the TUN
/// route: a connection whose sniffed TLS server name is in the set goes
/// direct by its address, decided by that rule; one whose name is not
/// goes to the selected node, which is asked for the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tun_domain_in_a_direct_rule_set_goes_direct() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rulesets/testdata/domains.srs");
    let sha256: String = Sha256::digest(std::fs::read(&path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut p = base();
    let alpha = p.selection.default_node_id.clone();
    p.routing.rule_sets.push(RuleSet {
        id: "direct-sites".into(),
        url: "https://rules.example/direct-sites.srs".into(),
        sha256,
        update_interval_seconds: 0,
    });
    p.routing.rules.insert(
        0,
        RoutingRule {
            id: "direct-sites".into(),
            matcher: RoutingMatch {
                rule_set_ids: vec!["direct-sites".into()],
                ..RoutingMatch::default()
            },
            action: RoutingAction {
                kind: "direct".into(),
                ..RoutingAction::default()
            },
            baseline: false,
        },
    );
    profile::validate(&p, golden_now()).unwrap();
    let options = Options {
        tun: Some(tun(false, LocalDns::System)),
        rule_sets: HashMap::from([(
            "direct-sites".to_owned(),
            RuleSetFile {
                path: path.to_str().unwrap().to_owned(),
                mirror_dns: true,
            },
        )]),
        ..Options::default()
    };
    let t = translate::translate(&p, &options).unwrap();
    let mut config: Value = serde_json::from_str(&t.json).unwrap();
    let socks_port = free_port();
    stand_in_for_the_tun(&mut config, socks_port);
    bind_loopback(&mut config);
    let mut sail = start("rule-set", &config.to_string()).await;

    // In the set: direct, by the address (documentation addresses; direct
    // is bound to loopback and gets nowhere).
    let inside = "192.0.2.10:20101";
    let _held = socks_open(socks_port, inside, &client_hello("www.cn.example")).await;
    let routed = sail.routed_on(20101).await;
    assert_eq!(routed.inbound, TUN_INBOUND_TAG, "{routed:?}");
    assert_eq!(
        routed.domain.as_deref(),
        Some("www.cn.example"),
        "{routed:?}"
    );
    assert_eq!(
        routed.rule.map(|i| t.rule_ids[i].as_str()),
        Some("direct-sites"),
        "{routed:?}"
    );
    assert_eq!(routed.chain, [DIRECT_TAG], "{routed:?}");
    assert_eq!(
        routed.request_destination.as_deref(),
        Some(inside),
        "{routed:?}"
    );

    // Not in the set: the selected node, asked for the name.
    let _held = socks_open(
        socks_port,
        "203.0.113.8:20102",
        &client_hello("elsewhere.example"),
    )
    .await;
    let routed = sail.routed_on(20102).await;
    assert_eq!(
        (
            routed.chain.first().map(String::as_str),
            node_of(&t, &routed).as_deref()
        ),
        (Some(SELECTED_TAG), Some(alpha.as_str())),
        "{routed:?}"
    );
    assert_eq!(
        routed.request_destination.as_deref(),
        Some("elsewhere.example:20102"),
        "{routed:?}"
    );

    sail.stop().await;
}

/// Go: internal/domaindest TestRestoreFallsBackToTheSharedReverseMapping.
///
/// Go kept the reverse mapping in a store shared across kernel switches;
/// sail keeps its own across a reload (rust-parity X1 ②). A TUN on a host
/// without an IPv6 path (`override_destination: proxy_and_direct` for
/// global IPv6, the profile's final direct): DNS through the TUN, hijacked
/// and answered by dns-local (a fake server on loopback), gives a name a
/// global IPv6 address; a connection to that address with nothing to sniff
/// is dialled direct by the name, its domain reverse-mapped, before a full
/// reload and after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reverse_mapping_survives_a_reload() {
    let dns = fake_dns().await;
    let mut p = base();
    p.routing.final_action = RoutingAction {
        kind: "direct".into(),
        ..RoutingAction::default()
    };
    profile::validate(&p, golden_now()).unwrap();
    let socks_port = free_port();
    let build = |p: &Profile| {
        let options = Options {
            tun: Some(tun(true, LocalDns::Servers(vec![dns]))),
            ..Options::default()
        };
        let t = translate::translate(p, &options).unwrap();
        assert!(t.direct_ipv6_hand_off, "{t:?}");
        let mut config: Value = serde_json::from_str(&t.json).unwrap();
        stand_in_for_the_tun(&mut config, socks_port);
        bind_loopback(&mut config);
        config.to_string()
    };
    let mut sail = start("reverse-mapping", &build(&p)).await;

    let answer = resolve_through_the_tun(socks_port, RecordType::AAAA).await;
    assert!(
        answer
            .answers
            .iter()
            .any(|r| r.data == RData::AAAA(AAAA(DUAL_V6))),
        "{answer:?}"
    );
    dialled_by_its_name(&mut sail, socks_port, 20201).await;

    // A profile update: a full reload.
    let mut updated = p.clone();
    updated.routing.rules.push(RoutingRule {
        id: "reload".into(),
        matcher: RoutingMatch {
            domain_suffixes: vec!["reload.example".into()],
            ..RoutingMatch::default()
        },
        action: RoutingAction {
            kind: "direct".into(),
            ..RoutingAction::default()
        },
        baseline: false,
    });
    profile::validate(&updated, golden_now()).unwrap();
    let report = sail.runtime.reload(&build(&updated)).await.unwrap();
    assert_eq!(report.path, "full", "{report:?}");
    dialled_by_its_name(&mut sail, socks_port, 20202).await;

    sail.stop().await;
}

/// A connection through the TUN's stand-in to DUAL's IPv6 address on
/// `port`, with bytes no sniffer names: dialled direct by DUAL, which the
/// reverse mapping gave.
async fn dialled_by_its_name(sail: &mut Sail, socks_port: u16, port: u16) {
    let destination = SocketAddr::from((DUAL_V6, port)).to_string();
    let _held = socks_open(socks_port, &destination, b"SSH-2.0-parity\r\n").await;
    let routed = sail.routed_on(port).await;
    assert_eq!(
        (routed.domain.as_deref(), routed.domain_source.as_deref()),
        (Some(DUAL), Some("reverse_mapping")),
        "{routed:?}"
    );
    assert_eq!(routed.chain, [DIRECT_TAG], "{routed:?}");
    assert_eq!(
        routed.request_destination,
        Some(format!("{DUAL}:{port}")),
        "{routed:?}"
    );
}

/// A DNS query for DUAL over TCP through the TUN's stand-in (to a
/// documentation address: port 53 is hijacked), and its answer.
async fn resolve_through_the_tun(socks_port: u16, kind: RecordType) -> Message {
    let mut query = Message::new(0x5151, MessageType::Query, OpCode::Query);
    query.metadata.recursion_desired = true;
    query.add_query(Query::query(
        Name::from_ascii(format!("{DUAL}.")).unwrap(),
        kind,
    ));
    let body = query.to_vec().unwrap();
    let mut framed = (body.len() as u16).to_be_bytes().to_vec();
    framed.extend(body);
    let mut stream = socks_open(socks_port, "192.0.2.53:53", &framed)
        .await
        .expect("the TUN's stand-in");
    let mut length = [0u8; 2];
    tokio::time::timeout(WAIT, stream.read_exact(&mut length))
        .await
        .expect("an answer in time")
        .unwrap();
    let mut answer = vec![0u8; u16::from_be_bytes(length) as usize];
    tokio::time::timeout(WAIT, stream.read_exact(&mut answer))
        .await
        .expect("the whole answer in time")
        .unwrap();
    Message::from_vec(&answer).unwrap()
}

/// A DNS server on loopback (UDP) that answers DUAL with DUAL_V4 and
/// DUAL_V6, and any other name with no records.
async fn fake_dns() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 65535];
        while let Ok((n, peer)) = socket.recv_from(&mut buffer).await {
            let Ok(query) = Message::from_vec(&buffer[..n]) else {
                continue;
            };
            let Some(question) = query.queries.first().cloned() else {
                continue;
            };
            let mut answer = Message::new(query.metadata.id, MessageType::Response, OpCode::Query);
            answer.metadata.recursion_desired = query.metadata.recursion_desired;
            answer.metadata.recursion_available = true;
            let ours = question
                .name()
                .to_ascii()
                .trim_end_matches('.')
                .eq_ignore_ascii_case(DUAL);
            let rdata = match question.query_type() {
                RecordType::A if ours => Some(RData::A(A(DUAL_V4))),
                RecordType::AAAA if ours => Some(RData::AAAA(AAAA(DUAL_V6))),
                _ => None,
            };
            if let Some(rdata) = rdata {
                answer.add_answer(Record::from_rdata(question.name().clone(), 60, rdata));
            }
            answer.add_query(question);
            if let Ok(bytes) = answer.to_vec() {
                let _ = socket.send_to(&bytes, peer).await;
            }
        }
    });
    addr
}
