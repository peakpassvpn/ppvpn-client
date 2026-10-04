//! #214 G8: what a Standard instance keeps in its `state_dir` survives a
//! restart and a new version.
//!
//! - A restart: an Engine in a process of its own, run twice on the same
//!   `state_dir`, gives the same local proxy credentials and port each time
//!   (`a_restarted_engine_keeps_its_local_proxy_credentials`).
//! - A new version: `testdata/golden/state_dir` is a state as this version
//!   writes it; every later version must read the same credentials and
//!   ports from it (`the_stored_state_reads_the_same`). A change of the
//!   format fails here first, and calls for a migration.
//!
//! Passwords are compared by a hash of them: a failing assertion prints no
//! secret.

use std::path::{Path, PathBuf};
use std::process::Command;

use ppvpn_core::{ApplyRequest, Engine, EngineConfig, Platform, Role};
use serde_json::Value;
use sha2::{Digest, Sha256};

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

/// The first 8 bytes of the secret's SHA-256: enough to tell two apart.
fn hash(secret: &str) -> u64 {
    let digest = Sha256::digest(secret.as_bytes());
    u64::from_be_bytes(digest[..8].try_into().unwrap())
}

fn manifest(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

/// What the child process reports: the routed user's port, username and
/// password (hashed), and the first node's credential once the contract
/// golden's profile is applied.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    port: u16,
    username: String,
    password: u64,
    node_username: String,
    node_password: u64,
}

/// An Engine on the `state_dir` in PPVPN_STATE_DIR_CHILD, as one run of a
/// host: it says what it sees on one line and shuts down. Started by
/// `a_restarted_engine_keeps_its_local_proxy_credentials`; without the
/// variable it returns at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_dir_child() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return;
    };
    let engine = Engine::new(EngineConfig::new(Role::Standard, platform(), &dir))
        .await
        .expect("child: new");
    let routed = engine
        .local_proxy_routed_credential()
        .expect("child: routed");
    let profile = std::fs::read(manifest(
        "../../testdata/golden/contract/profiles/base.json",
    ))
    .expect("child: profile");
    let node_id = serde_json::from_slice::<Value>(&profile).unwrap()["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    engine
        .apply(ApplyRequest::new(profile))
        .await
        .expect("child: apply");
    let node = engine
        .local_proxy_credential(&node_id)
        .expect("child: node credential");
    println!(
        "SEEN {} {} {} {} {}",
        routed.port,
        routed.username,
        hash(&routed.password),
        node.username,
        hash(&node.password)
    );
    engine.shutdown().await.expect("child: shutdown");
}

fn run_child(dir: &Path) -> Seen {
    let out = Command::new(std::env::current_exe().expect("this binary"))
        .args(["--exact", "state_dir_child", "--nocapture"])
        .env(CHILD_ENV, dir)
        .output()
        .expect("the child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the child failed: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("SEEN "))
        .unwrap_or_else(|| panic!("the child said nothing: {stdout}"));
    let fields: Vec<&str> = line.split(' ').collect();
    Seen {
        port: fields[0].parse().unwrap(),
        username: fields[1].into(),
        password: fields[2].parse().unwrap(),
        node_username: fields[3].into(),
        node_password: fields[4].parse().unwrap(),
    }
}

#[test]
fn a_restarted_engine_keeps_its_local_proxy_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let first = run_child(dir.path());
    assert!(first.port != 0 && !first.username.is_empty(), "{first:?}");
    let second = run_child(dir.path());
    assert_eq!(second, first, "the second run of the same state_dir");
}

/// Whether nothing listens on `port` on loopback (the persisted port is
/// kept only when it is free).
fn free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stored_state_reads_the_same() {
    let golden_path = manifest("../../testdata/golden/state_dir").join(STATE_FILE);
    let golden: Value = serde_json::from_slice(&std::fs::read(&golden_path).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(STATE_FILE);
    std::fs::copy(&golden_path, &path).unwrap();
    // The engine renews credentials others can read.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let port = golden["port"].as_u64().unwrap() as u16;
    let port_free = free(port);

    let engine = Engine::new(EngineConfig::new(Role::Standard, platform(), dir.path()))
        .await
        .expect("new on the stored state");
    let routed = engine.local_proxy_routed_credential().unwrap();
    assert_eq!(routed.username, golden["prefix"].as_str().unwrap());
    assert_eq!(
        hash(&routed.password),
        hash(golden["password"].as_str().unwrap()),
        "the stored password"
    );
    if port_free {
        assert_eq!(routed.port, port, "the stored port");
    } else {
        eprintln!("port {port} is taken here: its persistence is not checked");
    }
    engine.shutdown().await.unwrap();

    // Read and written back as stored: nothing renewed, nothing dropped.
    let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for key in ["version", "prefix", "system_proxy_port"] {
        assert_eq!(after[key], golden[key], "{key}");
    }
    assert_eq!(
        hash(after["password"].as_str().unwrap()),
        hash(golden["password"].as_str().unwrap()),
        "password"
    );
    if port_free {
        assert_eq!(after["port"], golden["port"], "port");
    }
}
