//! A failing instance on Windows leaves the system as it was while the host
//! lives on (#208), with our MSVC build of sail: the adapters, the routes
//! into the TUN's range and its own, its DNS servers and strict_route's WFP
//! filters are as before it started, and it starts again. sail's own test
//! (tests/test_teardown.rs) covers its build; this one ours.
//!
//! As administrator, with wintun.dll beside the test binary and the test
//! build's `fault-injection` feature (ci.yml's windows job):
//!
//!   cargo test -p ppvpn-core --features fault-injection -- --ignored runtime::windows_tests:: --test-threads 1
//!
//! strict_route routes 198.18.0.0/16 alone, so the runner's own traffic
//! stays out of the TUN.

use std::process::Command;
use std::time::{Duration, Instant};

use sail::embed::Options;
use sail::fault::{self, Point};

use super::sail::SailRuntime;
use super::{Runtime, RuntimeState};

const TUN: &str = "ppvpnwf0";

/// One at a time: the faults are the process's, and the TUN's name one.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn powershell(script: &str) -> String {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .expect("powershell");
    assert!(
        out.status.success(),
        "powershell: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// What an instance could leave (as sail's own test reads it): the
/// adapters, the routes into 198.18.0.0/16, the TUN's routes and DNS
/// servers, and the WFP filters strict_route names "sail".
fn system() -> String {
    let wfp = std::env::temp_dir().join(format!("ppvpn-wfp-{}.xml", std::process::id()));
    powershell(&format!(
        r#"$ErrorActionPreference = "SilentlyContinue"
"adapters: " + ((Get-NetAdapter -IncludeHidden | ForEach-Object Name | Sort-Object) -join ", ")
"routes into 198.18.0.0/16: " + ((Get-NetRoute -DestinationPrefix 198.18.0.0/16 | ForEach-Object {{ "$($_.InterfaceAlias) $($_.NextHop)" }}) -join ", ")
"routes of {TUN}: " + ((Get-NetRoute -InterfaceAlias {TUN} | ForEach-Object DestinationPrefix | Sort-Object) -join ", ")
"dns of {TUN}: " + ((Get-DnsClientServerAddress -InterfaceAlias {TUN} | ForEach-Object {{ $_.ServerAddresses }}) -join ", ")
netsh wfp show filters file="{wfp}" | Out-Null
"wfp filters named sail: " + (Select-String -Path "{wfp}" -Pattern "<name>sail" -SimpleMatch).Count"#,
        wfp = wfp.display()
    ))
}

/// The system once it is still: what a test before this one undid may
/// still be going.
fn settled() -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
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

pub(super) fn config() -> String {
    serde_json::json!({
        "log": { "level": "info" },
        "inbounds": [{
            "type": "tun", "tag": "tun", "interface_name": TUN,
            "address": ["172.31.236.1/30", "fdfe:236::1/126"],
            "auto_route": true, "strict_route": true,
            "route_address": ["198.18.0.0/16"]
        }],
        "outbounds": [{ "type": "direct", "tag": "direct" }]
    })
    .to_string()
}

pub(super) fn runtime(name: &str) -> SailRuntime {
    let dir = std::env::temp_dir().join(format!("ppvpn-win-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    SailRuntime::new(Options::new().data_dir(dir)).unwrap()
}

/// Running: the TUN is up with strict_route's filters.
fn routed(before: &str) {
    let now = system();
    assert_ne!(now, before, "routed once started");
    assert!(
        now.lines()
            .any(|line| line.starts_with("adapters: ") && line.contains(TUN)),
        "the TUN's adapter is up:\n{now}"
    );
    assert!(
        !now.contains("wfp filters named sail: 0"),
        "strict_route's filters are in place:\n{now}"
    );
}

/// The host, still the same process, starts it again and stops it: the
/// system is as before once more.
async fn starts_again(before: &str) {
    let again = runtime("again");
    again.start(&config()).await.expect("starts again");
    routed(before);
    again.stop().await.unwrap();
    assert!(
        again.stop_leftovers().is_empty(),
        "{:?}",
        again.stop_leftovers()
    );
    assert_eq!(
        settled(),
        before,
        "a stop after a failure left the system changed"
    );
}

/// G7 on Windows (R1): an instance's process killed outright (no stop, no
/// drop) leaves nothing either: the Wintun adapter and its routes and DNS
/// go with the process's handles, strict_route's WFP filters with its
/// dynamic session. A child process of this test binary
/// (runtime::windows_helper::instance) runs the instance; once it is
/// killed the system is as before it started, and an instance starts
/// again. The runtime rather than an Engine: an Engine's translation routes
/// everything into the TUN with strict_route, the runner's own traffic too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs administrator and wintun.dll beside the test: rust.yml's windows-msvc job"]
async fn a_killed_instance_leaves_the_system_as_it_was() {
    use std::io::BufRead;
    use std::process::Stdio;

    let _one = ONE_AT_A_TIME.lock().await;
    fault::disarm();
    let before = settled();
    let mut child = Command::new(std::env::current_exe().expect("this binary"))
        .args([
            "--exact",
            "runtime::windows_helper::instance",
            "--ignored",
            "--nocapture",
        ])
        .env("PPVPN_WINDOWS_INSTANCE", "1")
        .stdout(Stdio::piped())
        .spawn()
        .expect("the instance");
    let stdout = child.stdout.take().unwrap();
    let (running, ran) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if line.contains("instance running") {
                let _ = running.send(());
            }
        }
    });
    if ran.recv_timeout(Duration::from_secs(60)).is_err() {
        let _ = child.kill();
        panic!("the instance did not start within 60 s: {:?}", child.wait());
    }
    routed(&before);

    // TerminateProcess: no stop, no drop, no unwinding.
    child.kill().expect("kill");
    child.wait().expect("reaped");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut now = settled();
    while now != before && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        now = settled();
    }
    assert_eq!(now, before, "the killed instance left the system changed");
    starts_again(&before).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs administrator and wintun.dll beside the test: ci.yml's windows job"]
async fn a_failed_instance_leaves_the_system_as_it_was() {
    let _one = ONE_AT_A_TIME.lock().await;
    fault::disarm();
    let before = settled();
    let runtime = runtime("failed");
    runtime.start(&config()).await.unwrap();
    routed(&before);
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
    fault::disarm();
    assert!(failed.contains("fault injected"), "{failed}");
    runtime.stop().await.unwrap();
    assert!(
        runtime.stop_leftovers().is_empty(),
        "{:?}",
        runtime.stop_leftovers()
    );
    assert_eq!(settled(), before, "the failure left the system changed");
    starts_again(&before).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs administrator and wintun.dll beside the test: ci.yml's windows job"]
async fn a_start_that_fails_once_routed_leaves_the_system_as_it_was() {
    let _one = ONE_AT_A_TIME.lock().await;
    fault::disarm();
    let before = settled();
    let runtime = runtime("start-fails");
    fault::arm(Point::StartFails);
    let err = runtime
        .start(&config())
        .await
        .expect_err("started despite its fault");
    fault::disarm();
    assert!(err.message.contains("fault injected"), "{err:?}");
    assert!(
        runtime.stop_leftovers().is_empty(),
        "{:?}",
        runtime.stop_leftovers()
    );
    assert_eq!(
        settled(),
        before,
        "the failed start left the system changed"
    );
    starts_again(&before).await;
}
