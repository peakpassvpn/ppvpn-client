//! The daemon and its control channel: in process through the library, and
//! end to end with the `ppvpn` binary. Every test uses its own home, so the
//! real user's directories are never touched.

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use ppvpn_cli::client::Client;
use ppvpn_cli::control::Call;
use ppvpn_cli::daemon::Daemon;
use ppvpn_cli::env::{Env, Os};
use ppvpn_cli::paths::Paths;
use serde_json::Value;

/// A short home under /tmp: macOS's TMPDIR is long, and the control socket
/// lives under it.
fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ppc-")
        .tempdir_in("/tmp")
        .unwrap()
}

fn env_for(home: &Path) -> Env {
    let home = home.to_str().unwrap();
    let state = format!("{home}/state");
    let config = format!("{home}/config");
    Env::with_vars(
        Os::current(),
        &[
            ("HOME", home),
            ("XDG_STATE_HOME", &state),
            ("XDG_CONFIG_HOME", &config),
        ],
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_control_channel_serves_and_shuts_down() {
    let home = home();
    let env = env_for(home.path());
    let paths = Paths::resolve(&env).unwrap();
    let (daemon, listener) = Daemon::bind(&paths, env.os).await.unwrap();
    let serve = tokio::spawn(daemon.serve(listener));
    let client = Client::new(&paths).unwrap();

    let ping = client.call(Call::Ping).await.unwrap();
    assert_eq!(ping["pid"], std::process::id());
    assert!(ping["core"]["core_version"].is_string(), "{ping}");

    let status = client.call(Call::Status).await.unwrap();
    assert_eq!(status["state"], "stopped", "{status}");

    // A malformed profile is a validation error: exit 7, with core's code.
    let err = client
        .call(Call::Apply(ppvpn_core::ApplyRequest::new(
            b"{not json".to_vec(),
        )))
        .await
        .unwrap_err();
    assert_eq!(err.exit_code(), 7, "{err}");
    assert!(!err.code.is_empty());

    let err = client.call(Call::Start).await.unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.exit_code()),
        ("PROFILE_NOT_APPLIED", 5)
    );

    // Without the session secret nothing is served.
    std::fs::write(&paths.secret, "0".repeat(64)).unwrap();
    let err = Client::new(&paths)
        .unwrap()
        .call(Call::Status)
        .await
        .unwrap_err();
    assert_eq!(err.code, "CONTROL_UNAUTHENTICATED");

    client.call(Call::Shutdown).await.unwrap();
    serve.await.unwrap().unwrap();
    for leftover in [&paths.socket, &paths.secret, &paths.process_record] {
        assert!(!leftover.exists(), "{} left behind", leftover.display());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_daemon_for_the_same_directories_is_refused() {
    let home = home();
    let env = env_for(home.path());
    let paths = Paths::resolve(&env).unwrap();
    let (daemon, listener) = Daemon::bind(&paths, env.os).await.unwrap();
    let err = Daemon::bind(&paths, env.os)
        .await
        .err()
        .expect("second bind must fail");
    assert_eq!(
        (err.code.as_str(), err.exit_code()),
        ("DAEMON_ALREADY_RUNNING", 5)
    );
    let serve = tokio::spawn(daemon.serve(listener));
    Client::new(&paths)
        .unwrap()
        .call(Call::Shutdown)
        .await
        .unwrap();
    serve.await.unwrap().unwrap();
}

struct Cli<'a> {
    home: &'a Path,
}

impl Cli<'_> {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ppvpn"));
        let home = self.home.to_str().unwrap();
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("XDG_STATE_HOME", format!("{home}/state"))
            .env("XDG_CONFIG_HOME", format!("{home}/config"));
        command
    }

    fn run(&self, args: &[&str]) -> (i32, Value) {
        let Output {
            status,
            stdout,
            stderr,
        } = self.command(args).output().unwrap();
        let text = String::from_utf8(stdout).unwrap();
        let value = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("{args:?}: {text} {}", String::from_utf8_lossy(&stderr)));
        (status.code().unwrap_or(-1), value)
    }

    fn spawn_daemon(&self) -> Child {
        let child = self
            .command(&["daemon"])
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.run(&["--json", "status"]).1["daemon_running"] != true {
            assert!(Instant::now() < deadline, "the daemon did not come up");
            std::thread::sleep(Duration::from_millis(50));
        }
        child
    }
}

#[test]
fn status_and_stop_follow_the_daemon() {
    let home = home();
    let cli = Cli { home: home.path() };

    assert_eq!(
        cli.run(&["--json", "status"]),
        (
            0,
            serde_json::json!({"ok": true, "daemon_running": false, "state": "stopped"})
        )
    );
    assert_eq!(
        cli.run(&["--json", "stop"]),
        (0, serde_json::json!({"ok": true, "stopped": false}))
    );

    let mut child = cli.spawn_daemon();
    let (code, status) = cli.run(&["--json", "status"]);
    assert_eq!(code, 0);
    assert_eq!(status["pid"], child.id(), "{status}");
    assert_eq!(status["state"], "stopped", "nothing applied yet: {status}");

    let (code, second) = cli.run(&["--json", "daemon"]);
    assert_eq!(
        (code, second["code"].as_str()),
        (5, Some("DAEMON_ALREADY_RUNNING"))
    );

    assert_eq!(
        cli.run(&["--json", "stop"]),
        (0, serde_json::json!({"ok": true, "stopped": true}))
    );
    assert!(child.wait().unwrap().success());
    assert_eq!(cli.run(&["--json", "status"]).1["daemon_running"], false);
}

#[test]
fn a_killed_daemon_leaves_nothing_that_blocks_the_next_one() {
    let home = home();
    let cli = Cli { home: home.path() };
    let mut child = cli.spawn_daemon();
    // Like a crash or power loss: the record, secret and socket stay behind.
    child.kill().unwrap();
    child.wait().unwrap();
    let paths = Paths::resolve(&env_for(home.path())).unwrap();
    assert!(paths.process_record.exists() && paths.socket.exists());

    assert_eq!(
        cli.run(&["--json", "status"]).1["daemon_running"],
        false,
        "a dead record must not count"
    );
    assert_eq!(cli.run(&["--json", "stop"]).1["stopped"], false);

    let mut child = cli.spawn_daemon();
    assert_eq!(cli.run(&["--json", "stop"]).1["stopped"], true);
    assert!(child.wait().unwrap().success());
}

#[test]
fn commands_reach_a_daemon_that_has_no_profile_yet() {
    let home = home();
    let cli = Cli { home: home.path() };
    let mut child = cli.spawn_daemon();
    let paths = Paths::resolve(&env_for(home.path())).unwrap();

    assert_eq!(
        cli.run(&["--json", "nodes"]),
        (
            0,
            serde_json::json!({"ok": true, "selected_node_id": null, "nodes": []})
        )
    );
    let (code, traffic) = cli.run(&["--json", "traffic"]);
    assert_eq!(
        (code, &traffic["upload_bytes"], &traffic["download_bytes"]),
        (0, &serde_json::json!(0), &serde_json::json!(0)),
        "{traffic}"
    );
    assert_eq!(
        cli.run(&["--json", "connections"]),
        (0, serde_json::json!({"ok": true, "connections": []}))
    );
    assert_eq!(
        cli.run(&["--json", "ingress"]),
        (0, serde_json::json!({"ok": true, "nodes": []}))
    );
    let (code, missing) = cli.run(&["--json", "ingress", "hk-1"]);
    assert_eq!(
        (code, missing["code"].as_str(), missing["field"].as_str()),
        (2, Some("NODE_NOT_FOUND"), Some("node_id"))
    );

    // Core refuses selections and pins without a profile, and what core
    // refused is not saved.
    for args in [
        &["--json", "use", "hk-1"][..],
        &["--json", "ingress", "pin", "hk-1", "9002"],
        &["--json", "ingress", "auto", "hk-1"],
    ] {
        let (code, refused) = cli.run(args);
        assert_eq!(
            (code, refused["code"].as_str()),
            (5, Some("PROFILE_NOT_APPLIED")),
            "{args:?}"
        );
    }
    assert!(!paths.settings.exists(), "a refused choice was saved");

    // Nothing is applied, so a new mode is saved for the next start.
    assert_eq!(
        cli.run(&["--json", "mode", "global"]),
        (
            0,
            serde_json::json!({"ok": true, "routing_mode": "global", "applied": false})
        )
    );
    assert_eq!(cli.run(&["--json", "mode"]).1["routing_mode"], "global");

    assert_eq!(cli.run(&["--json", "stop"]).1["stopped"], true);
    assert!(child.wait().unwrap().success());
}
