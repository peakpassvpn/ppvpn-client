//! The guard against a real sail TUN (Go: internal/runtime
//! tun_rules_linux_test.go). They change the network namespace's routing,
//! so they are ignored by default and run only where that is disposable: as
//! root with PPVPN_TEST_REAL_TUN=1, in a network namespace of their own (a
//! container, or `ip netns exec` as test/netns/run.sh does in CI), never in
//! the host's. Elsewhere they print SKIP and pass, as Go's skip.
//!
//!   sudo test/netns/run.sh --libtest <ppvpn_core test binary> tunrules::linux_tests::

use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sail::embed::{self, Options};
use tokio::sync::watch;

use super::guard::Tuning;
use super::{sweep, Guard, Scope, TunRoutingStatus};
use crate::runtime::sail::SailRuntime;
use crate::runtime::{Runtime, RuntimeState};

const TUN: &str = "ppvpn-test0";
/// The probe range (TEST-NET-1), rejected by the configuration: a probe
/// through the TUN is answered at once (sail's stack takes the connection,
/// then resets it), one that bypasses it goes out the namespace's uplink
/// and is never answered (the uplink namespace has no way out). Not a blackhole in main as in Go's test: sail's rules look
/// main up first for all but DNS ("not dport 53 lookup main
/// suppress_prefixlength 0"), so a blackhole there would win over the TUN.
const PROBE_NET: &str = "192.0.2.0/24";

fn scope() -> Scope {
    Scope {
        interface: TUN.into(),
        table: 2091,
        rule_start: 9091,
        rule_end: 9101,
    }
}

/// Why this cannot run here, if it cannot.
fn skip_reason() -> Option<&'static str> {
    if std::env::var("PPVPN_TEST_REAL_TUN").as_deref() != Ok("1")
        // SAFETY: no arguments, no memory.
        || unsafe { libc::geteuid() } != 0
    {
        return Some("needs root and PPVPN_TEST_REAL_TUN=1 (privileged container)");
    }
    if !own_net_namespace() {
        return Some("changes the network namespace's rules: runs only in a namespace of its own (container or ip netns)");
    }
    None
}

/// Whether this process runs in a network namespace that is not the
/// host's: inside a container (/.dockerenv; its PID 1 shares the
/// container's namespace), or in a namespace other than PID 1's, as with
/// `ip netns exec` on a host. Unknown counts as not.
fn own_net_namespace() -> bool {
    if std::path::Path::new("/.dockerenv").exists() {
        return true;
    }
    match (
        std::fs::read_link("/proc/self/ns/net"),
        std::fs::read_link("/proc/1/ns/net"),
    ) {
        (Ok(ours), Ok(first)) => ours != first,
        _ => false,
    }
}

fn ip(args: &[&str]) -> String {
    let out = Command::new("ip").args(args).output().expect("run ip");
    assert!(
        out.status.success(),
        "ip {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The TUN's rules (priorities 9091..9101, both families) and the routes of
/// its table, as `ip` prints them, sorted.
fn tun_rules() -> Vec<String> {
    let mut out = Vec::new();
    for family in ["-4", "-6"] {
        for line in ip(&[family, "rule", "show"]).lines() {
            let Some((priority, _)) = line.split_once(':') else {
                continue;
            };
            if priority
                .parse::<u32>()
                .is_ok_and(|p| (9091..=9101).contains(&p))
            {
                out.push(format!("{family} {}", line.trim()));
            }
        }
        for line in ip(&[family, "route", "show", "table", "2091"]).lines() {
            if !line.trim().is_empty() {
                out.push(format!("{family} route {}", line.trim()));
            }
        }
    }
    out.sort();
    out
}

fn delete_rules(priorities: impl IntoIterator<Item = u32> + Clone) {
    for family in ["-4", "-6"] {
        for priority in priorities.clone() {
            let priority = priority.to_string();
            while Command::new("ip")
                .args([family, "rule", "del", "priority", &priority])
                .output()
                .is_ok_and(|out| out.status.success())
            {}
        }
    }
}

async fn wait_rules(want: &[String], within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        let got = tun_rules();
        if got == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "TUN rules not back within {within:?}:\ngot\n{}\nwant\n{}",
            got.join("\n"),
            want.join("\n")
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A sail with a real TUN in this namespace, and the guard over it.
struct RealTun {
    runtime: SailRuntime,
    guard: Option<Guard>,
    status: watch::Receiver<TunRoutingStatus>,
    log: Arc<Mutex<String>>,
    probes: AtomicU32,
}

impl RealTun {
    async fn start(tuning: Tuning) -> RealTun {
        sweep(&scope()).expect("sweep");
        let dir = std::env::temp_dir().join(format!("ppvpn-core-tunrules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let runtime =
            SailRuntime::new(Options::new().data_dir(dir).threads(embed::Threads::One)).unwrap();
        let log = Arc::new(Mutex::new(String::new()));
        let mut lines = runtime.logs();
        let collected = log.clone();
        tokio::spawn(async move {
            while let Some(line) = lines.recv().await {
                let mut log = collected.lock().unwrap();
                log.push_str(&line);
                log.push('\n');
            }
        });
        let config = serde_json::json!({
            "log": { "level": "info" },
            "inbounds": [{
                "type": "tun", "tag": "tun", "interface_name": TUN,
                "address": ["10.60.159.89/30", "fde2:ec40:9312:c7fd::1/126"],
                "auto_route": true, "strict_route": true,
                "iproute2_table_index": 2091, "iproute2_rule_index": 9091
            }],
            "outbounds": [{ "type": "direct", "tag": "direct" }],
            "route": {
                "rules": [{ "ip_cidr": [PROBE_NET], "action": "reject" }],
                "final": "direct",
                "auto_detect_interface": true
            }
        });
        runtime
            .start(&config.to_string())
            .await
            .expect("start sail");
        let mut states = runtime.states();
        tokio::time::timeout(Duration::from_secs(10), async {
            while *states.borrow_and_update() != RuntimeState::Running {
                states.changed().await.unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| panic!("sail not running: {:?}", runtime.state()));
        let (guard, status) = Guard::start_with(scope(), tuning);
        assert_eq!(*status.borrow(), TunRoutingStatus::Ok, "after start");
        RealTun {
            runtime,
            guard: Some(guard),
            status,
            log,
            probes: AtomicU32::new(0),
        }
    }

    fn logged(&self) -> String {
        self.log.lock().unwrap().clone()
    }

    /// Dials a new probe address and reports whether it was answered, i.e.
    /// whether it went through the TUN.
    async fn routed(&self) -> bool {
        let n = self.probes.fetch_add(1, Ordering::Relaxed) + 1;
        let destination = format!("192.0.2.{}:{}", n % 250 + 1, 9000 + n);
        let dial = tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(&destination),
        )
        .await;
        match dial {
            Ok(Ok(_)) => true,
            Ok(Err(e)) => matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
            ),
            Err(_) => false,
        }
    }

    /// Waits for a status sent since the last wait (or the start) that
    /// `want` accepts.
    async fn wait_status(
        &mut self,
        within: Duration,
        want: impl Fn(&TunRoutingStatus) -> bool,
    ) -> TunRoutingStatus {
        let status = &mut self.status;
        let found = tokio::time::timeout(within, async {
            loop {
                status.changed().await.expect("the guard stopped");
                let now = status.borrow_and_update().clone();
                if want(&now) {
                    return now;
                }
            }
        })
        .await;
        found.unwrap_or_else(|_| {
            panic!(
                "status {:?} after {within:?}\n{}",
                *self.status.borrow(),
                self.logged()
            )
        })
    }

    /// Stops the guard, then sail: sail's cleanup must stay undone.
    async fn stop(mut self) {
        if let Some(guard) = self.guard.take() {
            guard.stop();
        }
        self.runtime.stop().await.expect("stop sail");
        assert_eq!(tun_rules(), Vec::<String>::new(), "left after the stop");
    }
}

/// Go: TestTUNRulesRestoredAfterDeletion. Deletes the TUN's policy routing
/// the ways seen in the field (everything, as networkd does on a link down;
/// just the goto target; the table's routes) and requires each to be put
/// back, identical, within a second, with traffic in the TUN again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs root in a network namespace of its own: test/netns/run.sh"]
async fn tun_rules_restored_after_deletion() {
    if let Some(why) = skip_reason() {
        eprintln!("SKIP: {why}");
        return;
    }
    let mut tun = RealTun::start(Tuning::default()).await;
    let want = tun_rules();
    assert!(!want.is_empty(), "sail installed no rules");
    assert!(
        tun.routed().await,
        "probe not routed through the TUN at start:\n{}",
        tun.logged()
    );
    let cases: [(&str, fn()); 3] = [
        ("all rules", || delete_rules(9091..=9101)),
        ("goto target", || delete_rules([9101])),
        ("table routes", || {
            for family in ["-4", "-6"] {
                let _ = Command::new("ip")
                    .args([family, "route", "flush", "table", "2091"])
                    .output();
            }
        }),
    ];
    for (name, delete) in cases {
        eprintln!("deleting {name}");
        delete();
        wait_rules(&want, Duration::from_secs(1)).await;
        assert!(
            tun.routed().await,
            "{name}: probe bypassed the TUN after the restore:\n{}",
            tun.logged()
        );
        let status = tun
            .wait_status(Duration::from_secs(1), |s| {
                matches!(s, TunRoutingStatus::Restored { .. })
            })
            .await;
        let TunRoutingStatus::Restored { missing } = status else {
            unreachable!()
        };
        assert!(!missing.is_empty(), "{name}: restored nothing");
    }
    tun.stop().await;
}

/// Go: TestTUNRulesBrokenIsReported. Rules that stay missing set the status
/// to Broken. Root can always add them back, so a failing restore cannot be
/// staged; the guard runs with restoring off, where it reports what it
/// would otherwise fix. Put back by hand, a check reports Restored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs root in a network namespace of its own: test/netns/run.sh"]
async fn tun_rules_broken_is_reported() {
    if let Some(why) = skip_reason() {
        eprintln!("SKIP: {why}");
        return;
    }
    let mut tun = RealTun::start(Tuning {
        restore: false,
        ..Tuning::default()
    })
    .await;
    let want = tun_rules();
    delete_rules([9101]);
    let status = tun
        .wait_status(Duration::from_secs(2), |s| {
            matches!(s, TunRoutingStatus::Broken { .. })
        })
        .await;
    let TunRoutingStatus::Broken { missing, error } = status else {
        unreachable!()
    };
    assert!(
        missing.iter().any(|m| m == "9101/v4 nop") && missing.iter().any(|m| m == "9101/v6 nop"),
        "missing {missing:?}"
    );
    assert!(!error.is_empty());

    for family in ["-4", "-6"] {
        ip(&[family, "rule", "add", "priority", "9101", "nop"]);
    }
    assert_eq!(tun_rules(), want);
    tun.guard.as_ref().unwrap().check("test");
    tun.wait_status(Duration::from_secs(1), |s| {
        *s == TunRoutingStatus::Restored {
            missing: Vec::new(),
        }
    })
    .await;
    tun.stop().await;
}
