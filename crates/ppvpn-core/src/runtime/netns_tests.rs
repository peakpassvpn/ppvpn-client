//! The runtime against real interfaces, as root in test/netns/run.sh's
//! namespaces (ppvpn-t, this test's, with pt0 to ppvpn-w's pw0):
//!
//!   sudo test/netns/run.sh --libtest <ppvpn_core test binary> runtime::netns_tests::
//!
//! Ignored by default and SKIP elsewhere, as tunrules::linux_tests.

use std::process::{Child, Command};
use std::time::Duration;

use sail::embed::Options;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::sail::SailRuntime;
use super::{Runtime, Target};
use crate::tunrules::linux_tests::skip_reason;

const UPLINK: &str = "ppvpn-w";
/// The far end, on the uplink namespace's loopback: reachable through
/// either link, by a route per link in this namespace.
const FAR: &str = "10.243.9.9";
const ECHO_PORT: u16 = 7000;
/// What each connection moves both ways: enough to stand out of the
/// links' background (neighbour discovery and the like).
const LOAD: usize = 2 << 20;

fn ip(args: &[&str]) {
    let status = Command::new("ip").args(args).status().expect("ip");
    assert!(status.success(), "ip {}", args.join(" "));
}

fn tx_bytes(link: &str) -> u64 {
    std::fs::read_to_string(format!("/sys/class/net/{link}/statistics/tx_bytes"))
        .expect("tx_bytes")
        .trim()
        .parse()
        .expect("a count")
}

/// A second link, pt1 to the uplink's pw1, beside run.sh's pt0; FAR on the
/// uplink's loopback with a route through each link here; and the echo
/// there. Dropped: the echo ends and pt1 goes (its peer with it).
struct Links {
    echo: Child,
}

impl Links {
    fn up() -> Links {
        ip(&[
            "link", "add", "pt1", "type", "veth", "peer", "name", "pw1", "netns", UPLINK,
        ]);
        ip(&["addr", "add", "10.243.1.1/24", "dev", "pt1"]);
        ip(&["link", "set", "pt1", "up"]);
        ip(&["-n", UPLINK, "addr", "add", "10.243.1.2/24", "dev", "pw1"]);
        ip(&["-n", UPLINK, "link", "set", "pw1", "up"]);
        ip(&[
            "-n",
            UPLINK,
            "addr",
            "add",
            &format!("{FAR}/32"),
            "dev",
            "lo",
        ]);
        ip(&[
            "route",
            "add",
            &format!("{FAR}/32"),
            "via",
            "10.243.0.2",
            "dev",
            "pt0",
            "metric",
            "10",
        ]);
        ip(&[
            "route",
            "add",
            &format!("{FAR}/32"),
            "via",
            "10.243.1.2",
            "dev",
            "pt1",
            "metric",
            "20",
        ]);
        let echo = Command::new("ip")
            .args(["netns", "exec", UPLINK])
            .arg(std::env::current_exe().expect("this binary"))
            .args([
                "--exact",
                "runtime::netns_helper::echo",
                "--ignored",
                "--nocapture",
            ])
            .env("PPVPN_NETNS_ECHO", format!("0.0.0.0:{ECHO_PORT}"))
            .spawn()
            .expect("the echo");
        Links { echo }
    }
}

impl Drop for Links {
    fn drop(&mut self) {
        let _ = self.echo.kill();
        let _ = self.echo.wait();
        let _ = Command::new("ip").args(["link", "del", "pt1"]).status();
        let _ = Command::new("ip")
            .args([
                "-n",
                UPLINK,
                "addr",
                "del",
                &format!("{FAR}/32"),
                "dev",
                "lo",
            ])
            .status();
    }
}

fn config(interface: &str) -> String {
    serde_json::json!({
        "log": { "level": "info" },
        "outbounds": [{ "type": "direct", "tag": "direct" }],
        "route": { "final": "direct", "default_interface": interface }
    })
    .to_string()
}

/// Moves LOAD each way over a direct dial's connection and says which link
/// carried it: the one whose sent bytes grew by at least LOAD.
async fn carried_by(stream: &mut (impl AsyncReadExt + AsyncWriteExt + Unpin)) -> &'static str {
    let (a, b) = (tx_bytes("pt0"), tx_bytes("pt1"));
    let block = vec![7u8; 64 << 10];
    let mut back = vec![0u8; 64 << 10];
    let mut moved = 0;
    while moved < LOAD {
        stream.write_all(&block).await.expect("write");
        stream.read_exact(&mut back).await.expect("echo");
        moved += block.len();
    }
    let (da, db) = (tx_bytes("pt0") - a, tx_bytes("pt1") - b);
    match (da >= LOAD as u64, db >= LOAD as u64) {
        (true, false) => "pt0",
        (false, true) => "pt1",
        _ => panic!("pt0 sent {da} bytes, pt1 {db}: not one link"),
    }
}

/// sail's route.default_interface, changed by a reload, is what new direct
/// connections leave by; one already open stays on the link it has.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs root in a network namespace of its own: test/netns/run.sh"]
async fn a_reload_moves_new_connections_to_the_new_default_interface() {
    if let Some(why) = skip_reason() {
        eprintln!("SKIP: {why}");
        return;
    }
    let _links = Links::up();
    let dir = std::env::temp_dir().join(format!("ppvpn-netns-reload-{}", std::process::id()));
    let runtime = SailRuntime::new(Options::new().data_dir(&dir)).unwrap();
    runtime.start(&config("pt0")).await.unwrap();
    let far = Target::Addr(format!("{FAR}:{ECHO_PORT}").parse().unwrap());
    let wait = Duration::from_secs(5);

    // The echo comes up in the uplink namespace: retry the first dial.
    let mut before = None;
    for _ in 0..50 {
        if let Ok(stream) = runtime.dial_tcp("direct", far.clone(), wait).await {
            before = Some(stream);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut before = before.expect("a direct dial to the echo");
    assert_eq!(carried_by(&mut before).await, "pt0", "before the reload");

    runtime
        .reload(&config("pt1"))
        .await
        .expect("a reload of the route's interface");
    let mut after = runtime
        .dial_tcp("direct", far, wait)
        .await
        .expect("a dial after the reload");
    assert_eq!(
        carried_by(&mut after).await,
        "pt1",
        "a new connection after the reload"
    );
    assert_eq!(
        carried_by(&mut before).await,
        "pt0",
        "the connection opened before stays"
    );

    drop((before, after));
    runtime.stop().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// An instance made to fail on purpose (sail's fault points, feature
/// `fault-injection`) undoes what it changed in the system while this
/// process lives on: the host's side of #208.
#[cfg(feature = "fault-injection")]
mod failures {
    use std::time::{Duration, Instant};

    use sail::embed::Options;
    use sail::fault::{self, Point};

    use super::super::sail::SailRuntime;
    use super::super::{Runtime, RuntimeState};
    use crate::tunrules::linux_tests::skip_reason;
    use crate::types::LeftoverKind;

    /// sail names its nftables table after the TUN (auto_redirect).
    const TUN: &str = "ppvpnft0";
    const TABLE: &str = "sail_ppvpnft0";

    fn run(program: &str, args: &str) -> String {
        let out = std::process::Command::new(program)
            .args(args.split_whitespace())
            .output()
            .expect(program);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// What an instance could leave in this namespace (as sail's own
    /// teardown test): rules, routes of every table, nftables tables, links.
    fn system() -> String {
        [
            ("ip", "-4 rule"),
            ("ip", "-6 rule"),
            ("ip", "-4 route show table all"),
            ("ip", "-6 route show table all"),
            ("nft", "list tables"),
            ("ip", "-o link"),
        ]
        .iter()
        .map(|(p, a)| format!("$ {p} {a}\n{}", run(p, a)))
        .collect()
    }

    /// The system once it is still: an earlier test's teardown may still
    /// be going.
    fn settled() -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = system();
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let now = system();
            if now == last || Instant::now() >= deadline {
                return now;
            }
            last = now;
        }
    }

    fn config(redirect: bool) -> String {
        serde_json::json!({
            "log": { "level": "info" },
            "inbounds": [{
                "type": "tun", "tag": "tun", "interface_name": TUN,
                "address": ["172.31.235.1/30", "fdfe:235::1/126"],
                "auto_route": true, "auto_redirect": redirect
            }],
            "outbounds": [{ "type": "direct", "tag": "direct" }]
        })
        .to_string()
    }

    fn runtime(name: &str) -> SailRuntime {
        let dir = std::env::temp_dir().join(format!("ppvpn-netns-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        SailRuntime::new(Options::new().data_dir(dir)).unwrap()
    }

    /// Calls `stop` twice on an instance that no longer runs: each returns
    /// at once (no second teardown), with the same report.
    async fn stopped_twice_at_once(runtime: &SailRuntime) {
        let mut reports = Vec::new();
        for attempt in 1..=2 {
            let started = Instant::now();
            runtime.stop().await.unwrap();
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "stop {attempt} took {:?}",
                started.elapsed()
            );
            reports.push(runtime.stop_leftovers());
        }
        assert_eq!(reports[0], reports[1], "the same report twice");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs root in a network namespace of its own: test/netns/run.sh"]
    async fn a_failed_instance_leaves_the_system_as_it_was() {
        if let Some(why) = skip_reason() {
            eprintln!("SKIP: {why}");
            return;
        }
        fault::disarm();
        let before = settled();
        let runtime = runtime("failed");
        runtime.start(&config(false)).await.unwrap();
        assert_ne!(system(), before, "routed once started");
        fault::arm(Point::EssentialTask);
        let mut states = runtime.states();
        let failed = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let RuntimeState::Failed { message, .. } = states.borrow_and_update().clone() {
                    return message;
                }
                states.changed().await.unwrap();
            }
        })
        .await
        .expect("failed within 15 s");
        assert!(failed.contains("fault injected"), "{failed}");
        stopped_twice_at_once(&runtime).await;
        assert!(
            runtime.stop_leftovers().is_empty(),
            "{:?}",
            runtime.stop_leftovers()
        );
        assert_eq!(settled(), before, "the failure left the system changed");
        fault::disarm();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs root in a network namespace of its own: test/netns/run.sh"]
    async fn a_start_that_fails_once_routed_leaves_the_system_as_it_was() {
        if let Some(why) = skip_reason() {
            eprintln!("SKIP: {why}");
            return;
        }
        fault::disarm();
        let before = settled();
        let runtime = runtime("start-fails");
        fault::arm(Point::StartFails);
        let error = runtime.start(&config(false)).await.unwrap_err();
        assert!(error.message.contains("fault injected"), "{error}");
        stopped_twice_at_once(&runtime).await;
        assert_eq!(
            settled(),
            before,
            "the failed start left the system changed"
        );
        fault::disarm();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "needs root in a network namespace of its own: test/netns/run.sh"]
    async fn a_step_that_panics_is_left_with_its_kind_and_how_to_clear_it() {
        if let Some(why) = skip_reason() {
            eprintln!("SKIP: {why}");
            return;
        }
        fault::disarm();
        let before = settled();
        let runtime = runtime("step-panics");
        runtime.start(&config(true)).await.unwrap();
        fault::arm(Point::TeardownStep(format!("nft table inet {TABLE}")));
        runtime.stop().await.unwrap();
        let left = runtime.stop_leftovers();
        let table = left
            .iter()
            .find(|l| l.name.contains(TABLE))
            .unwrap_or_else(|| panic!("the table is reported: {left:?}"));
        assert_eq!(table.kind, LeftoverKind::Rule, "{table:?}");
        let clear = table
            .detail
            .split("clear it with `")
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .unwrap_or_else(|| panic!("how to clear it: {table:?}"));
        let status = std::process::Command::new("sh")
            .args(["-c", clear])
            .status()
            .unwrap();
        assert!(status.success(), "{clear}");
        assert_eq!(settled(), before, "more than the table was left");
        fault::disarm();
    }
}
