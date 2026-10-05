//! Linux enhanced mode end to end (CI job `desktop-linux-enhanced`): the real
//! privileged service (the engine in process), this crate's [`Client`] in
//! enhanced mode, a mock backend and a local Shadowsocks node. Checks that
//! connecting brings up the TUN with the engine's policy rules (priorities
//! 9091–9101) and routing table 2091, that traffic to an outside address
//! goes through the TUN and the node, and that the routing state is gone
//! after a disconnect, once the lease of a killed app lapses, and after
//! `kill -9` of the service once it starts again (its startup sweep).
//!
//! Runs only under `test/netns/run.sh --libtest` (it sets
//! `PPVPN_TEST_REAL_TUN=1` inside its namespace `ppvpn-t`, whose uplink
//! namespace `ppvpn-w` is 10.243.0.2), as root, with the installed layout:
//!
//! - this test binary as `/usr/lib/ppvpn/ppvpn` (the only client path the
//!   service accepts);
//! - `ppvpn-service` in `/usr/lib/ppvpn-service`;
//! - `PPVPN_E2E_FAKENODE`: the `test/fakenode` binary.
//!
//! The client runs in a child process (this binary again, test
//! [`client_role`]) so that "the app was killed" is a real `kill -9`, with no
//! client left to reconnect. Addresses: run.sh's 10.243.0.0/24, the TUN's
//! own, and 192.0.2.10 (TEST-NET-1) for the HTTP target behind the node.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::ops::RangeInclusive;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::test_backend::{self, Backend};
use crate::{
    AuthState, Client, ClientConfig, ClientListener, ClientSnapshot, ConnectionPhase,
    PlatformError, PlatformHooks, ProbeResult, ProfileStatus, StandardState, TrafficSample,
};

const APP_DIR: &str = "/usr/lib/ppvpn";
const SERVICE_DIR: &str = "/usr/lib/ppvpn-service";
const SERVICE_SOCKET: &str = "/run/ppvpn/service.sock";
const SERVICE_LOG_DIR: &str = "/var/log/ppvpn";
/// A system bus address nothing listens on (see [`Service::start`]).
const NO_SYSTEM_BUS: &str = "unix:path=/run/ppvpn-e2e-no-system-bus";

/// run.sh's uplink namespace and its address: the node listens there.
const UPLINK_NS: &str = "ppvpn-w";
const NODE_IP: &str = "10.243.0.2";
const NODE_PORT: u16 = 8388;
/// Shadowsocks 2022 key (16 bytes, base64), as internal/runtime's tests use.
const NODE_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAA==";
/// HTTP target behind the node: a TEST-NET-1 address on the uplink's
/// loopback, outside the test namespace's connected network, so traffic to
/// it takes the TUN's routes.
const TARGET_IP: &str = "192.0.2.10";
const TARGET_PORT: u16 = 8080;
const TARGET_BODY: &str = "ppvpn-e2e-ok";

/// The engine's TUN auto-route (crates/ppvpn-core/src/translate/tun.rs).
const RULE_PRIORITIES: RangeInclusive<u32> = 9091..=9101;
const ROUTE_TABLE: &str = "2091";

/// How long the routing state may take to go after a stop.
const CLEAN_WAIT: Duration = Duration::from_secs(30);
/// The service's lease (45 s) plus the watchdog and the shutdown.
const LEASE_WAIT: Duration = Duration::from_secs(70);

const ROLE_ENV: &str = "PPVPN_E2E_ROLE";
const API_ENV: &str = "PPVPN_E2E_API";

#[test]
#[ignore = "needs root and run.sh's namespaces: test/netns/run.sh --libtest"]
fn enhanced_mode_routes_and_cleanup() {
    if std::env::var("PPVPN_TEST_REAL_TUN").as_deref() != Ok("1") {
        println!("SKIP: not in run.sh's namespace (PPVPN_TEST_REAL_TUN unset)");
        return;
    }
    let exe = std::env::current_exe().unwrap();
    if exe != Path::new(APP_DIR).join("ppvpn") {
        println!(
            "SKIP: the test binary must run as {APP_DIR}/ppvpn, not {}",
            exe.display()
        );
        return;
    }
    let Some(fakenode) = std::env::var_os("PPVPN_E2E_FAKENODE") else {
        println!("SKIP: PPVPN_E2E_FAKENODE (the test/fakenode binary) unset");
        return;
    };
    let ipv6 = ipv6_enabled();
    if !ipv6 {
        println!("e2e: IPv6 is disabled in this namespace; the IPv6 rule checks are skipped");
    }

    let node = FakeNode::start(Path::new(&fakenode));
    let mut service = Service::start();
    let backend = Backend::new();
    *backend.profile.lock().unwrap() = Some(profile());
    let api = test_backend::serve(backend);

    // 1. Connect, check the routing state and that traffic takes the node.
    let mut app = App::start(&api);
    app.command("connect", ConnectionPhase::On);
    expect_routes_up(ipv6, "after connect");
    expect_traffic_through_node(&node);

    // 2. Disconnect: everything the engine installed is gone.
    app.command("disconnect", ConnectionPhase::Off);
    expect_clean(ipv6, "after disconnect", CLEAN_WAIT);

    // 3. The app is killed: the service stops the instance once its lease
    // lapses.
    app.command("connect", ConnectionPhase::On);
    expect_routes_up(ipv6, "after reconnect");
    app.kill();
    expect_clean(ipv6, "after kill -9 of the app (lease lapsed)", LEASE_WAIT);

    // 4. The service killed with the instance up (a stop timeout): it
    // sweeps what was left when it starts again.
    let mut app = App::start(&api);
    app.command("connect", ConnectionPhase::On);
    expect_routes_up(ipv6, "before killing the service");
    app.kill();
    service.kill();
    println!(
        "e2e: state left by the killed service:\n{}",
        routing_state()
    );
    let service = Service::start();
    expect_clean(ipv6, "after the service restarted", CLEAN_WAIT);

    service.stop();
    node.stop();
}

/// The client in a child process, driven over stdin by
/// [`enhanced_mode_routes_and_cleanup`]: lines `connect` / `disconnect`.
/// Prints `E2E ready`, `E2E phase <phase>` and `E2E done <command> <result>`.
/// Does nothing unless started by that test.
#[test]
#[ignore = "started by enhanced_mode_routes_and_cleanup"]
fn client_role() {
    if std::env::var(ROLE_ENV).as_deref() != Ok("client") {
        return;
    }
    let api_base = std::env::var(API_ENV).expect("PPVPN_E2E_API");
    let data_dir = test_backend::temp_dir("ppvpn-e2e-client");
    let config = ClientConfig {
        api_base,
        data_dir: data_dir.clone(),
        log_dir: data_dir,
        platform: "linux".into(),
        app_version: "0.0.0-e2e".into(),
    };
    let client = Client::new(
        config,
        E2eHooks::signed_in(),
        Arc::new(PhasePrinter::default()),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for line in std::io::stdin().lock().lines() {
        let command = line.unwrap();
        let result = match command.trim() {
            "connect" => runtime.block_on(client.connect()),
            "disconnect" => runtime.block_on(client.disconnect()),
            _ => break,
        };
        say(&format!(
            "done {} {:?}",
            command.trim(),
            result.map_err(|e| e.to_string())
        ));
    }
}

fn say(line: &str) {
    println!("E2E {line}");
}

// ---------------------------------------------------------------------------
// The client side
// ---------------------------------------------------------------------------

/// Signed in (a saved refresh token the mock backend accepts), the service
/// counted as installed; installing is not something this test can do.
struct E2eHooks {
    blob: Mutex<Option<Vec<u8>>>,
}

impl E2eHooks {
    fn signed_in() -> Arc<Self> {
        Arc::new(Self {
            blob: Mutex::new(Some(
                br#"{"version":1,"active_refresh":"jyr_old"}"#.to_vec(),
            )),
        })
    }
}

impl PlatformHooks for E2eHooks {
    fn credential_load(&self) -> Result<Option<Vec<u8>>, PlatformError> {
        Ok(self.blob.lock().unwrap().clone())
    }
    fn credential_save(&self, blob: Vec<u8>) -> Result<(), PlatformError> {
        *self.blob.lock().unwrap() = Some(blob);
        Ok(())
    }
    fn credential_delete(&self) -> Result<(), PlatformError> {
        *self.blob.lock().unwrap() = None;
        Ok(())
    }
    fn open_url(&self, _url: String) -> bool {
        false
    }
    fn privileged_service_installed(&self) -> bool {
        true
    }
    fn install_privileged_service(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Failed {
            message: "the e2e test runs the service itself".into(),
        })
    }
    fn uninstall_privileged_service(&self) -> Result<(), PlatformError> {
        self.install_privileged_service()
    }
}

/// Prints connection phase changes, and `ready` once signed in with a
/// profile and the standard core running.
#[derive(Default)]
struct PhasePrinter {
    phase: Mutex<Option<ConnectionPhase>>,
    ready: Mutex<bool>,
}

impl ClientListener for PhasePrinter {
    fn on_snapshot(&self, snapshot: ClientSnapshot) {
        let phase = snapshot.connection.phase;
        let mut last = self.phase.lock().unwrap();
        if *last != Some(phase) {
            *last = Some(phase);
            match &snapshot.connection.reason {
                Some(reason) => say(&format!("phase {phase:?} ({reason:?})")),
                None => say(&format!("phase {phase:?}")),
            }
        }
        let mut ready = self.ready.lock().unwrap();
        if !*ready
            && snapshot.auth == AuthState::SignedIn
            && snapshot.profile_status == ProfileStatus::Ready
            && matches!(snapshot.standard, StandardState::Ready { .. })
        {
            *ready = true;
            say("ready");
        }
    }
    fn on_probe_result(&self, _result: ProbeResult) {}
    fn on_traffic(&self, _sample: TrafficSample) {}
}

/// One node with one Shadowsocks ingress at the fake node; private
/// destinations (the mock backend on loopback) go direct, the rest through
/// the node.
fn profile() -> String {
    serde_json::json!({
        "schema_version": 1,
        "revision": "e2e0000000000000000000000000000000000000000000000000000000000001",
        "generated_at": "2026-01-01T00:00:00Z",
        "expires_at": "2099-01-01T00:00:00Z",
        "nodes": [{
            "id": "e2e-node",
            "name": "e2e",
            "entry_key": "standard",
            "exit": { "ip": "203.0.113.10", "region": "e2e" },
            "capabilities": { "tcp": true, "udp": true },
            "ingresses": [{
                "role": "primary",
                "endpoint_key": "e2e-1",
                "replica_ordinal": 0,
                "protocol": "shadowsocks",
                "endpoint": { "domain": NODE_IP, "port": NODE_PORT },
                "credentials": { "shadowsocks": {
                    "method": "2022-blake3-aes-128-gcm",
                    "user_key": NODE_KEY
                }},
                "capabilities": { "tcp": true, "udp": true }
            }]
        }],
        "selection": { "mode": "manual", "default_node_id": "e2e-node" },
        "routing": {
            "rules": [{
                "id": "bypass-private",
                "match": { "ip_is_private": true },
                "action": { "type": "direct" }
            }],
            "final": { "type": "proxy", "target": "selected" }
        }
    })
    .to_string()
}

/// The client child process.
struct App {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl App {
    /// Starts the client and waits until it is signed in with a profile.
    fn start(api_base: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "--nocapture",
                "e2e_linux::client_role",
            ])
            .env(ROLE_ENV, "client")
            .env(API_ENV, api_base)
            .env("PPVPN_TEST_TRUST_LOCAL_BACKEND", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the client");
        let stdin = child.stdin.take().unwrap();
        let lines = forward_lines("app", child.stdout.take().unwrap());
        let mut app = Self {
            child,
            stdin,
            lines,
        };
        app.wait_for("E2E ready", Duration::from_secs(60));
        app
    }

    /// Sends `command` and waits for the connection to reach `phase`.
    fn command(&mut self, command: &str, phase: ConnectionPhase) {
        println!("e2e: {command}");
        writeln!(self.stdin, "{command}").unwrap();
        self.wait_for(&format!("E2E phase {phase:?}"), Duration::from_secs(90));
    }

    fn wait_for(&mut self, prefix: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.starts_with(prefix) => return,
                Ok(line) if line.starts_with("E2E done") && line.contains("Err(") => {
                    panic!("client: {line}\n{}", routing_state())
                }
                Ok(_) => {}
                Err(_) => panic!(
                    "no \"{prefix}\" from the client within {timeout:?}\n{}\n{}",
                    routing_state(),
                    service_log_tail()
                ),
            }
        }
    }

    /// `kill -9`, like an app that crashed: nothing is released.
    fn kill(mut self) {
        println!("e2e: kill -9 the client (pid {})", self.child.id());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// The service and the node
// ---------------------------------------------------------------------------

struct Service(Child);

impl Service {
    /// Starts `ppvpn-service` (in this namespace) and waits for its socket.
    ///
    /// Without a system bus: a network namespace does not separate D-Bus, so
    /// the core's per-link DNS would reach the host's systemd-resolved with
    /// this namespace's interface index, i.e. on whatever host link has that
    /// number, and outlive a killed core there. On a real install the link
    /// is the TUN and goes away with it.
    fn start() -> Self {
        let child = Command::new(Path::new(SERVICE_DIR).join("ppvpn-service"))
            .env("DBUS_SYSTEM_BUS_ADDRESS", NO_SYSTEM_BUS)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start ppvpn-service");
        println!("e2e: ppvpn-service started (pid {})", child.id());
        let deadline = Instant::now() + Duration::from_secs(30);
        while UnixStream::connect(SERVICE_SOCKET).is_err() {
            assert!(
                Instant::now() < deadline,
                "ppvpn-service does not answer on {SERVICE_SOCKET}\n{}",
                service_log_tail()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        Self(child)
    }

    /// `kill -9` (systemd's stop timeout): nothing is cleaned up.
    fn kill(&mut self) {
        println!("e2e: kill -9 ppvpn-service (pid {})", self.0.id());
        let _ = self.0.kill();
        let _ = self.0.wait();
    }

    /// SIGTERM, then waits: the service stops its instance on the way out.
    fn stop(mut self) {
        signal(self.0.id(), "TERM");
        let _ = self.0.wait();
    }
}

struct FakeNode {
    child: Child,
    seen: Arc<Mutex<Vec<String>>>,
}

impl FakeNode {
    fn start(binary: &Path) -> Self {
        let target = format!("{TARGET_IP}/32");
        run(
            "ip",
            &["-n", UPLINK_NS, "addr", "add", target.as_str(), "dev", "lo"],
        );
        let (ss, http) = (
            format!("{NODE_IP}:{NODE_PORT}"),
            format!("{TARGET_IP}:{TARGET_PORT}"),
        );
        let mut child = Command::new("ip")
            .args(["netns", "exec", UPLINK_NS])
            .arg(binary)
            .args(["-ss", ss.as_str(), "-key", NODE_KEY, "-http", http.as_str()])
            .stdout(Stdio::piped())
            .spawn()
            .expect("start fakenode");
        let lines = forward_lines("node", child.stdout.take().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let ready = Arc::new(Mutex::new(false));
        {
            let (seen, ready) = (seen.clone(), ready.clone());
            std::thread::spawn(move || {
                for line in lines {
                    if line.starts_with("ready ") {
                        *ready.lock().unwrap() = true;
                    }
                    seen.lock().unwrap().push(line);
                }
            });
        }
        wait_until("fakenode ready", Duration::from_secs(20), || {
            *ready.lock().unwrap()
        });
        Self { child, seen }
    }

    fn routed(&self, destination: &str) -> bool {
        let wanted = format!("conn {destination}");
        self.seen.lock().unwrap().contains(&wanted)
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

fn expect_routes_up(ipv6: bool, when: &str) {
    wait_until(
        &format!("routes up ({when})"),
        Duration::from_secs(30),
        || {
            !tun_links().is_empty()
                && rules(false)
                    .iter()
                    .any(|rule| rule.contains(&format!("lookup {ROUTE_TABLE}")))
                && !table_routes(false).is_empty()
                && (!ipv6 || !rules(true).is_empty())
        },
    );
    println!("e2e: routes up ({when}): TUN {:?}", tun_links());
}

fn expect_clean(ipv6: bool, when: &str, wait: Duration) {
    wait_until(&format!("clean ({when})"), wait, || {
        tun_links().is_empty()
            && rules(false).is_empty()
            && table_routes(false).is_empty()
            && (!ipv6 || (rules(true).is_empty() && table_routes(true).is_empty()))
    });
    println!("e2e: clean ({when})");
}

/// A request to the target behind the node succeeds, and the node routed
/// it: it went into the TUN and out through the node, not direct.
fn expect_traffic_through_node(node: &FakeNode) {
    let destination = format!("{TARGET_IP}:{TARGET_PORT}");
    let deadline = Instant::now() + Duration::from_secs(30);
    let body = loop {
        match http_get(&destination) {
            Ok(body) => break body,
            Err(error) if Instant::now() < deadline => {
                println!("e2e: GET http://{destination}/: {error}; retrying");
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(error) => panic!("GET http://{destination}/: {error}\n{}", routing_state()),
        }
    };
    assert!(body.contains(TARGET_BODY), "unexpected answer: {body:?}");
    wait_until(
        "the node routed the request",
        Duration::from_secs(10),
        || node.routed(&destination),
    );
    println!("e2e: http://{destination}/ answered through the node");
}

fn http_get(destination: &str) -> std::io::Result<String> {
    let mut stream =
        TcpStream::connect_timeout(&destination.parse().unwrap(), Duration::from_secs(10))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(stream, "GET / HTTP/1.0\r\nHost: {destination}\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

/// TUN devices, whatever the engine names them (`ppvpn0`, ...).
fn tun_links() -> Vec<String> {
    output("ip", &["-o", "-d", "link", "show"])
        .lines()
        .filter(|line| line.contains(" tun type tun "))
        .filter_map(|line| line.split(": ").nth(1).map(str::to_string))
        .collect()
}

/// Policy rules at the core's priorities.
fn rules(v6: bool) -> Vec<String> {
    output("ip", &[family(v6), "rule", "show"])
        .lines()
        .filter(|line| {
            line.split(':')
                .next()
                .and_then(|priority| priority.trim().parse::<u32>().ok())
                .is_some_and(|priority| RULE_PRIORITIES.contains(&priority))
        })
        .map(str::to_string)
        .collect()
}

/// Routes in the core's table (none when the table does not exist).
fn table_routes(v6: bool) -> Vec<String> {
    output("ip", &[family(v6), "route", "show", "table", ROUTE_TABLE])
        .lines()
        .map(str::to_string)
        .collect()
}

fn family(v6: bool) -> &'static str {
    if v6 {
        "-6"
    } else {
        "-4"
    }
}

fn ipv6_enabled() -> bool {
    std::fs::read_to_string("/proc/sys/net/ipv6/conf/all/disable_ipv6")
        .is_ok_and(|value| value.trim() == "0")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Everything the checks look at, for failure messages.
fn routing_state() -> String {
    let mut state = String::new();
    for (title, program, args) in [
        ("ip -4 rule", "ip", &["-4", "rule", "show"][..]),
        ("ip -6 rule", "ip", &["-6", "rule", "show"][..]),
        (
            "table 2091 (v4)",
            "ip",
            &["-4", "route", "show", "table", ROUTE_TABLE][..],
        ),
        (
            "table 2091 (v6)",
            "ip",
            &["-6", "route", "show", "table", ROUTE_TABLE][..],
        ),
        ("links", "ip", &["-o", "-br", "link", "show"][..]),
    ] {
        state.push_str(&format!("## {title}\n{}", output(program, args)));
    }
    state
}

fn service_log_tail() -> String {
    let path = Path::new(SERVICE_LOG_DIR).join("ppvpn-service.log");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let tail: Vec<&str> = text.lines().rev().take(40).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    format!("## {} (last lines)\n{}", path.display(), tail.join("\n"))
}

/// Stdout of a command; empty when it fails (e.g. a table that does not
/// exist).
fn output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program).args(args).status().unwrap();
    assert!(status.success(), "{program} {args:?}: {status}");
}

fn signal(pid: u32, name: &str) {
    let _ = Command::new("kill")
        .args([format!("-{name}"), pid.to_string()])
        .status();
}

/// Lines of `pipe`, echoed with `tag` and passed on.
fn forward_lines(tag: &'static str, pipe: impl Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            println!("[{tag}] {line}");
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}

fn wait_until(what: &str, timeout: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}\n{}\n{}",
            routing_state(),
            service_log_tail()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}
