//! #214 G8: what a Standard instance keeps in its `state_dir` survives a
//! restart and a new version.
//!
//! - A restart: an Engine in a process of its own, run twice on the same
//!   `state_dir`, hands out the credentials and port the state file holds,
//!   and the second run leaves the file as the first wrote it
//!   (`a_restarted_engine_keeps_its_local_proxy_credentials`).
//! - A new version: `testdata/golden/state_dir` is a state as this version
//!   writes it; every later version must hand out the same credentials and
//!   port from it (`the_stored_state_reads_the_same`). A change of the
//!   format fails here first, and calls for a migration.
//!
//! Credentials are compared without being printed or hashed: assertions
//! that involve one say what differs, not its value.

use std::path::{Path, PathBuf};
use std::process::Command;

use ppvpn_core::{ApplyRequest, Engine, EngineConfig, LocalProxyConfig, Platform, Role};
use serde_json::Value;

const STATE_FILE: &str = "local-proxies.json";
const CHILD_ENV: &str = "PPVPN_STATE_DIR_CHILD";

fn platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}

fn manifest(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

/// A Standard instance with the local proxy on `dir`, the preferred port
/// left to the state (0: the persisted one, else any free one).
async fn engine(dir: &Path) -> Engine {
    Engine::new(
        EngineConfig::new(Role::Standard, platform(), dir)
            .with_local_proxy(LocalProxyConfig::new().with_preferred_port(0)),
    )
    .await
    .expect("new")
}

fn state(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join(STATE_FILE)).expect("the state file"))
        .expect("the state decodes")
}

/// Whether nothing listens on `port` on loopback (the persisted port is
/// kept only when it is free).
fn free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Checks what `engine` hands out against the state file in `dir`: the
/// routed user's prefix, password and port, and a node's credential on
/// the contract golden's profile (the same port and password, its username
/// under the prefix). The port only when `check_port`.
async fn hands_out_the_state(engine: &Engine, dir: &Path, check_port: bool) {
    let stored = state(dir);
    let prefix = stored["prefix"].as_str().expect("a prefix");
    let password = stored["password"].as_str().expect("a password");
    let routed = engine.local_proxy_routed_credential().expect("routed");
    assert!(
        routed.username == prefix,
        "the routed username is not the stored prefix"
    );
    assert!(
        routed.password == password,
        "the routed password is not the stored one"
    );
    if check_port {
        assert_eq!(
            u64::from(routed.port),
            stored["port"].as_u64().unwrap(),
            "the stored port"
        );
    }
    let profile = std::fs::read(manifest(
        "../../testdata/golden/contract/profiles/base.json",
    ))
    .expect("profile");
    let node_id = serde_json::from_slice::<Value>(&profile).unwrap()["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    engine
        .apply(ApplyRequest::new(profile))
        .await
        .expect("apply");
    let node = engine
        .local_proxy_credential(&node_id)
        .expect("node credential");
    assert_eq!(node.port, routed.port, "one port for every node");
    assert!(
        node.password == password,
        "a node's password is not the stored one"
    );
    assert!(
        node.username.starts_with(prefix) && node.username != prefix,
        "a node's username is not under the stored prefix"
    );
}

/// One run of a host on the `state_dir` in PPVPN_STATE_DIR_CHILD: it
/// checks what it hands out against the state file, and shuts down. Started
/// by `a_restarted_engine_keeps_its_local_proxy_credentials`; without the
/// variable it returns at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_dir_child() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let engine = engine(&dir).await;
    hands_out_the_state(&engine, &dir, true).await;
    engine.shutdown().await.expect("shutdown");
}

fn run_child(dir: &Path) {
    let out = Command::new(std::env::current_exe().expect("this binary"))
        .args(["--exact", "state_dir_child", "--nocapture"])
        .env(CHILD_ENV, dir)
        .output()
        .expect("the child");
    assert!(
        out.status.success(),
        "the child failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_restarted_engine_keeps_its_local_proxy_credentials() {
    let dir = tempfile::tempdir().unwrap();
    run_child(dir.path());
    let first = std::fs::read(dir.path().join(STATE_FILE)).expect("the first run's state");
    run_child(dir.path());
    let second = std::fs::read(dir.path().join(STATE_FILE)).expect("the second run's state");
    assert!(first == second, "the second run changed the state file");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stored_state_reads_the_same() {
    let golden_path = manifest("../../testdata/golden/state_dir").join(STATE_FILE);
    let golden = std::fs::read(&golden_path).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(STATE_FILE);
    std::fs::write(&path, &golden).unwrap();
    // The engine renews credentials others can read.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let port = state(dir.path())["port"].as_u64().unwrap() as u16;
    let port_free = free(port);
    if !port_free {
        eprintln!("port {port} is taken here: its persistence is not checked");
    }

    let engine = engine(dir.path()).await;
    // Against the golden as stored, not as the engine may have rewritten it.
    let golden_dir = manifest("../../testdata/golden/state_dir");
    hands_out_the_state(&engine, &golden_dir, port_free).await;
    engine.shutdown().await.unwrap();

    // Written back as stored: nothing renewed, nothing dropped.
    let golden: Value = serde_json::from_slice(&golden).unwrap();
    let after = state(dir.path());
    for key in ["version", "prefix", "system_proxy_port"] {
        assert_eq!(after[key], golden[key], "{key}");
    }
    assert!(
        after["password"] == golden["password"],
        "the password was renewed"
    );
    if port_free {
        assert_eq!(after["port"], golden["port"], "port");
    }
}
