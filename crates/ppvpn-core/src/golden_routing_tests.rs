//! The routing golden files (testdata/golden/routing) on a real sail: the
//! decisions Go 0.5.21 exported (frozen; the Go core is gone) for a
//! profile and a set of connections, checked against what sail decides
//! with the Rust translation. As the Go runner did: the TUN is a SOCKS inbound
//! carrying the TUN's tag (TLS or HTTP bytes after the CONNECT to sniff),
//! the local proxy and the system proxy are themselves, and every outbound
//! that dials is bound to loopback, so a dial fails at once and nothing
//! leaves the host. The decision is sail's Routed event for the
//! connection. `classifier` (Go's flow adapter, for comparison) has no Rust
//! counterpart and is not compared.

use std::collections::btree_map::{BTreeMap, Entry};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::config::Platform;
use crate::profile;
use crate::request::RoutingMode;
use crate::runtime::sail::SailRuntime;
use crate::runtime::{Routed, Runtime};
use crate::translate::{
    self, interface_name, LocalDns, LocalProxy, Options, Translation, Tun, DIRECT_TAG,
    SELECTED_TAG, TUN_INBOUND_TAG,
};

const WAIT: Duration = Duration::from_secs(5);

/// The time the Go runner built the profile at.
fn golden_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/golden/routing")
}

#[derive(Deserialize)]
struct RoutingFile {
    profile_ref: BTreeMap<String, String>,
    cases: Vec<Case>,
    expect: Vec<Value>,
}

/// One connection. `inbound`: `tun`, `proxy-routed` (the local proxy's
/// routed user), `proxy-node:<id>` (that node's user) or `system-proxy`.
#[derive(Deserialize)]
struct Case {
    name: String,
    inbound: String,
    #[serde(default)]
    routing_mode: String,
    host_ipv6_route: Option<bool>,
    destination: String,
    sniff: Option<Sniff>,
}

#[derive(Deserialize)]
struct Sniff {
    #[serde(default)]
    tls_server_name: String,
    #[serde(default)]
    http_host: String,
}

/// One sail: a TUN build or a local proxy build, for one routing mode and
/// host IPv6 state, as the desktop runs them.
struct Instance {
    runtime: SailRuntime,
    routes: mpsc::Receiver<Routed>,
    translation: Translation,
    /// The TUN's stand-in, or the local proxy and the system proxy.
    socks_port: u16,
    proxy: Option<LocalProxy>,
    system_port: u16,
    /// sail's data directory, removed after the stop.
    dir: PathBuf,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routing_matches_the_go_golden() {
    let mut files: Vec<_> = std::fs::read_dir(golden_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no routing golden files");
    let mut failures = Vec::new();
    for path in files {
        let file: RoutingFile = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(file.cases.len(), file.expect.len(), "{}", path.display());
        let profile_bytes = resolve(&file.profile_ref);
        let mut instances: BTreeMap<String, Instance> = BTreeMap::new();
        for (case, expect) in file.cases.iter().zip(&file.expect) {
            let mode = match case.routing_mode.as_str() {
                "" | "rules" => RoutingMode::Rules,
                "global" => RoutingMode::Global,
                other => panic!("{}: routing mode {other}", case.name),
            };
            let tun = case.inbound == "tun";
            let host_ipv6 = case.host_ipv6_route.unwrap_or(true);
            let key = format!("{tun}/{mode:?}/{host_ipv6}");
            let instance = match instances.entry(key.clone()) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => {
                    e.insert(start(&profile_bytes, tun, mode, host_ipv6, &key).await)
                }
            };
            let got = instance.decide(case).await;
            let mut want = expect.clone();
            want.as_object_mut().unwrap().remove("classifier");
            if got != want {
                failures.push(format!("{}:\n  got  {got}\n  want {want}", case.name));
            }
        }
        for instance in instances.into_values() {
            instance.runtime.stop().await.unwrap();
            let _ = std::fs::remove_dir_all(&instance.dir);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The profile a file names: `base` alone (the only form the routing files
/// use).
fn resolve(reference: &BTreeMap<String, String>) -> Vec<u8> {
    assert_eq!(
        reference.keys().map(String::as_str).collect::<Vec<_>>(),
        ["base"],
        "profile_ref"
    );
    std::fs::read(golden_dir().join(&reference["base"])).unwrap()
}

/// A password made for this run: none is written in the source.
fn password() -> String {
    let mut buf = [0u8; 12];
    getrandom::fill(&mut buf).unwrap();
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn start(
    profile_bytes: &[u8],
    tun: bool,
    mode: RoutingMode,
    host_ipv6: bool,
    key: &str,
) -> Instance {
    let p = profile::parse(profile_bytes).unwrap();
    profile::validate(&p, golden_now()).unwrap();
    let mut options = Options {
        mode,
        ..Options::default()
    };
    let (socks_port, system_port) = (free_port(), free_port());
    if tun {
        options.tun = Some(Tun {
            desktop: true,
            ipv6: true,
            no_host_ipv6_route: !host_ipv6,
            interface_name: interface_name(Platform::Linux).into(),
            local_dns: LocalDns::System,
        });
    } else {
        options.local_proxy = Some(LocalProxy {
            listen: "127.0.0.1".into(),
            port: free_port(),
            prefix: "gold0".into(),
            password: password(),
        });
        options.system_proxy_port = Some(system_port);
    }
    let translation = translate::translate(&p, &options).unwrap();
    let mut config: Value = serde_json::from_str(&translation.json).unwrap();
    if tun {
        stand_in_for_the_tun(&mut config, socks_port);
    }
    bind_loopback(&mut config);
    let dir = std::env::temp_dir().join(format!(
        "ppvpn-core-golden-routing-{}-{}",
        key.replace('/', "-"),
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
    let routes = runtime.routes();
    runtime.start(&config.to_string()).await.unwrap();
    Instance {
        runtime,
        routes,
        translation,
        socks_port,
        proxy: options.local_proxy,
        system_port,
        dir,
    }
}

/// The TUN inbound becomes a SOCKS inbound on loopback with the TUN's tag:
/// the rules that name the TUN (sniff, override_destination, the floors)
/// apply to it. What only a TUN has goes with it.
fn stand_in_for_the_tun(config: &mut Value, port: u16) {
    let inbounds = config["inbounds"].as_array_mut().unwrap();
    let tun = inbounds
        .iter_mut()
        .find(|i| i["tag"] == TUN_INBOUND_TAG)
        .expect("the TUN inbound");
    *tun = json!({ "type": "socks", "tag": TUN_INBOUND_TAG, "listen": "127.0.0.1", "listen_port": port });
    if let Some(route) = config["route"].as_object_mut() {
        route.remove("auto_detect_interface");
        route.remove("default_interface");
    }
}

/// Binds every outbound that dials to the loopback interface: a dial to
/// anywhere else fails at once, after routing has decided.
fn bind_loopback(config: &mut Value) {
    for outbound in config["outbounds"].as_array_mut().unwrap() {
        let kind = outbound["type"].as_str().unwrap_or_default();
        if !matches!(kind, "selector" | "fallback" | "urltest" | "block" | "dns") {
            outbound["bind_interface"] = "lo".into();
        }
    }
}

impl Instance {
    async fn decide(&mut self, case: &Case) -> Value {
        let port = case.destination.rsplit(':').next().unwrap();
        let suffix = format!(":{port}");
        let stream = self.open(case).await;
        let routed = tokio::time::timeout(WAIT, async {
            loop {
                let r = self.routes.recv().await.expect("the channel");
                if r.destination.ends_with(&suffix) {
                    return r;
                }
            }
        })
        .await;
        drop(stream);
        match routed {
            Ok(routed) => self.interpret(&routed),
            Err(_) => json!({ "action": "NONE" }),
        }
    }

    /// Connects through the case's inbound and sends the bytes to sniff.
    async fn open(&self, case: &Case) -> Option<TcpStream> {
        let payload = match &case.sniff {
            Some(s) if !s.tls_server_name.is_empty() => client_hello(&s.tls_server_name),
            Some(s) if !s.http_host.is_empty() => {
                format!("GET / HTTP/1.1\r\nHost: {}\r\n\r\n", s.http_host).into_bytes()
            }
            _ => Vec::new(),
        };
        let user = |node: &str| {
            let proxy = self.proxy.as_ref().expect("a local proxy build");
            format!("{}:{}", proxy.username(node), proxy.password)
        };
        let proxy_port = self.proxy.as_ref().map(|p| p.port).unwrap_or_default();
        match case.inbound.as_str() {
            "tun" => socks_open(self.socks_port, &case.destination, &payload).await,
            "system-proxy" => {
                connect_open(self.system_port, None, &case.destination, &payload).await
            }
            "proxy-routed" => {
                connect_open(proxy_port, Some(&user("")), &case.destination, &payload).await
            }
            other => {
                let node = other
                    .strip_prefix("proxy-node:")
                    .unwrap_or_else(|| panic!("inbound {other}"));
                connect_open(proxy_port, Some(&user(node)), &case.destination, &payload).await
            }
        }
    }

    /// Go's interpretation of the decision: the direct outbound is DIRECT,
    /// the selector and a node's outbounds are that node.
    fn interpret(&self, routed: &Routed) -> Value {
        if routed.action != "outbound" {
            return json!({ "action": "REJECT" });
        }
        let mut out = Map::new();
        let t = &self.translation;
        let head = routed.chain.first().map(String::as_str).unwrap_or_default();
        let node = routed
            .chain
            .iter()
            .find_map(|tag| t.outbound_nodes.get(tag));
        let action = match (head, node) {
            (DIRECT_TAG, _) => "DIRECT".to_owned(),
            (SELECTED_TAG, Some(_)) | (_, Some(_)) => "PROXY".to_owned(),
            _ => format!("OUTBOUND:{head}"),
        };
        out.insert("action".into(), action.clone().into());
        if action == "PROXY" {
            out.insert("node_id".into(), node.unwrap().clone().into());
        }
        if let Some(target) = &routed.request_destination {
            let host = target.rsplit_once(':').map_or(target.as_str(), |(h, _)| h);
            let is_ip = host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .is_ok();
            let kind = if is_ip { "ip" } else { "domain" };
            out.insert("target".into(), target.clone().into());
            out.insert("target_kind".into(), kind.into());
            if action == "DIRECT" && t.direct_ipv6_hand_off && !is_ip {
                out.insert("ipv6_hand_off".into(), true.into());
            }
        }
        Value::Object(out)
    }
}

/// A SOCKS5 greeting and CONNECT (IP or domain); once answered, the
/// payload to sniff, as an application sends it. sail answers a sniffed
/// connection before it dials.
async fn socks_open(port: u16, destination: &str, payload: &[u8]) -> Option<TcpStream> {
    let (host, port_text) = destination.rsplit_once(':').unwrap();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let to: u16 = port_text.parse().unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    let mut request = vec![5, 1, 0, 5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            request.push(1);
            request.extend(ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            request.push(4);
            request.extend(ip.octets());
        }
        Err(_) => {
            request.extend([3, host.len() as u8]);
            request.extend(host.as_bytes());
        }
    }
    request.extend(to.to_be_bytes());
    stream.write_all(&request).await.ok()?;
    // Method reply, then the CONNECT reply (VER REP RSV ATYP ADDR PORT).
    let mut reply = [0u8; 6];
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut reply)).await;
    if !matches!(read, Ok(Ok(_))) || reply[3] != 0 {
        return Some(stream); // refused before answering
    }
    let mut bound = vec![0u8; if reply[5] == 4 { 16 + 2 } else { 4 + 2 }];
    stream.read_exact(&mut bound).await.ok()?;
    if !payload.is_empty() {
        stream.write_all(payload).await.ok()?;
    }
    Some(stream)
}

/// An HTTP CONNECT (Basic credentials `user:password` when given) and the
/// payload.
async fn connect_open(
    port: u16,
    credentials: Option<&str>,
    destination: &str,
    payload: &[u8],
) -> Option<TcpStream> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    let mut request = format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n");
    if let Some(credentials) = credentials {
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
        request.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    request.push_str("\r\n");
    let mut bytes = request.into_bytes();
    bytes.extend_from_slice(payload);
    stream.write_all(&bytes).await.ok()?;
    Some(stream)
}

/// A TLS 1.2 ClientHello record carrying `server_name` (SNI), enough for a
/// sniffer.
fn client_hello(server_name: &str) -> Vec<u8> {
    let name = server_name.as_bytes();
    let mut sni = Vec::new();
    sni.extend(((name.len() + 3) as u16).to_be_bytes()); // server name list
    sni.push(0); // host_name
    sni.extend((name.len() as u16).to_be_bytes());
    sni.extend(name);
    let mut extensions = Vec::new();
    extensions.extend([0, 0]); // server_name
    extensions.extend((sni.len() as u16).to_be_bytes());
    extensions.extend(sni);
    let mut body = vec![3, 3]; // TLS 1.2
    body.extend([0x42; 32]); // random
    body.push(0); // session id
    body.extend([0, 4, 0xc0, 0x2f, 0x13, 0x01]); // cipher suites
    body.extend([1, 0]); // compression: null
    body.extend((extensions.len() as u16).to_be_bytes());
    body.extend(extensions);
    let mut handshake = vec![1]; // ClientHello
    handshake.extend(&(body.len() as u32).to_be_bytes()[1..]);
    handshake.extend(body);
    let mut record = vec![0x16, 3, 1];
    record.extend((handshake.len() as u16).to_be_bytes());
    record.extend(handshake);
    record
}
