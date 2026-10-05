//! The commands against an applied profile: a daemon in this process on
//! ppvpn-core's in-memory runtime (nothing runs, nothing touches the
//! network), driven through the same control channel as the real one. What
//! is checked is the CLI's side: what it asks core, what it prints, and
//! what it writes back to `settings.json`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ppvpn_cli::buildinfo::{BuildConfig, Profile};
use ppvpn_cli::client::Client;
use ppvpn_cli::control::Call;
use ppvpn_cli::daemon::Daemon;
use ppvpn_cli::env::{Env, Os};
use ppvpn_cli::paths::Paths;
use ppvpn_cli::Hooks;
use serde_json::{json, Value};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../testdata/profiles/multi-ingress.json"
);
const TOKYO: &str = "3f2c9a1e-0000-4000-8000-000000000001-128";
const SAN_JOSE: &str = "3f2c9a1e-0000-4000-8000-000000000002-129";

fn fixture() -> Value {
    serde_json::from_slice(&std::fs::read(FIXTURE).unwrap()).unwrap()
}

/// Seconds since the epoch as RFC 3339 (UTC).
fn rfc3339(unix: u64) -> String {
    let (days, rest) = (unix / 86_400, unix % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A daemon in this process and a CLI that talks to it.
struct Session {
    home: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    serve: Option<tokio::task::JoinHandle<ppvpn_cli::error::Result<()>>>,
    hooks: Hooks,
}

impl Session {
    fn new() -> Session {
        // A short home under /tmp: the control socket lives under it.
        let home = tempfile::Builder::new()
            .prefix("ppe-")
            .tempdir_in("/tmp")
            .unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut session = Session {
            home,
            runtime,
            serve: None,
            hooks: Hooks {
                // A dev build reads the profile from PPVPN_PROFILE_FILE, so
                // no backend and no login are involved.
                build: Some(BuildConfig {
                    profile: Profile::Dev,
                    api_base: "https://api.example.com".to_string(),
                }),
                store: None,
            },
        };
        let paths = session.paths();
        paths.ensure().unwrap();
        let engine = {
            let _guard = session.runtime.enter();
            // The local proxy's state (its credentials) is real; nothing
            // listens on the in-memory runtime.
            ppvpn_core::internal::engine_on_fake_runtime(
                ppvpn_core::EngineConfig::new(
                    ppvpn_core::Role::Standard,
                    ppvpn_core::Platform::Linux,
                    &paths.state_dir,
                )
                .with_local_proxy(ppvpn_core::LocalProxyConfig::new().with_preferred_port(0)),
            )
        };
        let (daemon, listener) = {
            let _guard = session.runtime.enter();
            Daemon::bind_engine(&paths, engine).unwrap()
        };
        session.serve = Some(session.runtime.spawn(daemon.serve(listener)));
        session
    }

    fn profile_file(&self) -> PathBuf {
        self.home.path().join("profile.json")
    }

    fn write_profile(&self, profile: &Value) {
        std::fs::write(self.profile_file(), profile.to_string()).unwrap();
    }

    fn env(&self) -> Env {
        let home = self.home.path().to_str().unwrap();
        let state = format!("{home}/state");
        let config = format!("{home}/config");
        let profile = self.profile_file();
        Env::with_vars(
            Os::current(),
            &[
                ("HOME", home),
                ("XDG_STATE_HOME", &state),
                ("XDG_CONFIG_HOME", &config),
                ("PPVPN_PROFILE_FILE", profile.to_str().unwrap()),
            ],
        )
    }

    fn paths(&self) -> Paths {
        Paths::resolve(&self.env()).unwrap()
    }

    /// Runs `ppvpn --json <args>`: the exit code and the one JSON value.
    fn run(&self, args: &[&str]) -> (i32, Value) {
        let argv: Vec<std::ffi::OsString> = ["ppvpn", "--json"]
            .iter()
            .chain(args)
            .map(|arg| (*arg).into())
            .collect();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = ppvpn_cli::run_with(&argv, &self.env(), &self.hooks, &mut stdout, &mut stderr);
        let text = String::from_utf8(stdout).unwrap();
        let value = serde_json::from_str(&text).unwrap_or_else(|_| {
            panic!(
                "{args:?} printed {text:?}; stderr: {}",
                String::from_utf8_lossy(&stderr)
            )
        });
        (code, value)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, value) = self.run(args);
        assert_eq!(code, 0, "{args:?}: {value}");
        assert_eq!(value["ok"], true, "{args:?}: {value}");
        value
    }

    fn settings(&self) -> Value {
        read_json(&self.paths().settings)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let paths = self.paths();
        if let Some(serve) = self.serve.take() {
            self.runtime.block_on(async {
                if let Ok(client) = Client::new(&paths) {
                    let _ = client.call(Call::Shutdown).await;
                }
                let _ = tokio::time::timeout(Duration::from_secs(15), serve).await;
            });
        }
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn rfc3339_matches_known_instants() {
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339(4_102_444_799), "2099-12-31T23:59:59Z");
}

#[test]
fn start_applies_the_profile_and_selections_are_saved_once_core_took_them() {
    let session = Session::new();
    session.write_profile(&fixture());

    let started = session.ok(&["start"]);
    assert_eq!(started["state"], "running", "{started}");
    assert_eq!(started["routing_mode"], "rules", "{started}");
    assert_eq!(started["selected_node_id"], TOKYO, "{started}");

    let nodes = session.ok(&["nodes"]);
    assert_eq!(nodes["selected_node_id"], TOKYO, "{nodes}");
    let ids: Vec<&str> = nodes["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [TOKYO, SAN_JOSE]);

    // Select: core takes it, then the CLI saves it.
    assert_eq!(
        session.ok(&["use", SAN_JOSE]),
        json!({"ok": true, "selected_node_id": SAN_JOSE})
    );
    assert_eq!(session.settings()["selected_node_id"], SAN_JOSE);
    assert_eq!(session.ok(&["nodes"])["selected_node_id"], SAN_JOSE);
    assert_eq!(session.ok(&["status"])["selected_node_id"], SAN_JOSE);

    // A node core does not know is an argument error and changes nothing.
    let (code, refused) = session.run(&["use", "no-such-node"]);
    assert_eq!(
        (code, refused["code"].as_str()),
        (2, Some("NODE_NOT_FOUND")),
        "{refused}"
    );
    assert_eq!(session.settings()["selected_node_id"], SAN_JOSE);
    assert_eq!(session.ok(&["status"])["selected_node_id"], SAN_JOSE);
}

#[test]
fn ingress_pins_follow_core_and_are_saved() {
    let session = Session::new();
    session.write_profile(&fixture());
    session.ok(&["start"]);

    let pinned = |session: &Session| {
        let shown = session.ok(&["ingress", TOKYO]);
        let nodes = shown["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1, "{shown}");
        assert_eq!(
            nodes[0]["ingresses"].as_array().unwrap().len(),
            2,
            "{shown}"
        );
        nodes[0]["pinned_endpoint_key"].clone()
    };
    assert_eq!(pinned(&session), Value::Null);

    assert_eq!(
        session.ok(&["ingress", "pin", TOKYO, "9002"]),
        json!({"ok": true, "node_id": TOKYO, "pinned_endpoint_key": "9002"})
    );
    assert_eq!(pinned(&session), "9002");
    assert_eq!(session.settings()["ingress_pins"], json!({TOKYO: "9002"}));

    // An ingress the node does not have: refused, and the pin stays.
    let (code, refused) = session.run(&["ingress", "pin", TOKYO, "9003"]);
    assert_eq!(
        (code, refused["code"].as_str()),
        (2, Some("INGRESS_NOT_FOUND")),
        "{refused}"
    );
    assert_eq!(pinned(&session), "9002");
    assert_eq!(session.settings()["ingress_pins"], json!({TOKYO: "9002"}));

    assert_eq!(
        session.ok(&["ingress", "auto", TOKYO]),
        json!({"ok": true, "node_id": TOKYO, "pinned_endpoint_key": null})
    );
    assert_eq!(pinned(&session), Value::Null);
    assert!(session.settings().get("ingress_pins").is_none());
}

#[test]
fn a_mode_change_reapplies_what_is_live() {
    let session = Session::new();
    session.write_profile(&fixture());
    session.ok(&["start"]);
    session.ok(&["use", SAN_JOSE]);
    session.ok(&["ingress", "pin", TOKYO, "9002"]);
    // The daemon holds the profile: the mode change must not need the file.
    std::fs::remove_file(session.profile_file()).unwrap();

    assert_eq!(
        session.ok(&["mode", "global"]),
        json!({"ok": true, "routing_mode": "global", "applied": true})
    );
    let status = session.ok(&["status"]);
    assert_eq!(status["state"], "running", "{status}");
    assert_eq!(status["routing_mode"], "global", "{status}");
    // The selection and the pin made after the first apply are kept.
    assert_eq!(status["selected_node_id"], SAN_JOSE, "{status}");
    let tokyo = status["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node_id"] == TOKYO)
        .unwrap_or_else(|| panic!("{status}"));
    assert_eq!(tokyo["pinned_endpoint_key"], "9002", "{status}");
    assert_eq!(
        session.settings(),
        json!({
            "routing_mode": "global",
            "selected_node_id": SAN_JOSE,
            "ingress_pins": {TOKYO: "9002"},
        })
    );

    assert_eq!(session.ok(&["mode", "rules"])["applied"], true);
    assert_eq!(session.ok(&["status"])["routing_mode"], "rules");
}

#[test]
fn a_mode_change_on_an_expired_profile_fetches_a_new_one() {
    let session = Session::new();
    let mut short_lived = fixture();
    short_lived["generated_at"] = json!(rfc3339(now() - 60));
    let expires = now() + 4;
    short_lived["expires_at"] = json!(rfc3339(expires));
    session.write_profile(&short_lived);
    let started = session.ok(&["start"]);
    assert_eq!(started["revision"], short_lived["revision"], "{started}");

    // What the next download returns.
    let mut renewed = fixture();
    renewed["revision"] = json!("2026-09-29T00:00:00Z#2");
    session.write_profile(&renewed);
    while now() <= expires {
        std::thread::sleep(Duration::from_millis(200));
    }

    assert_eq!(
        session.ok(&["mode", "global"]),
        json!({"ok": true, "routing_mode": "global", "applied": true})
    );
    let status = session.ok(&["status"]);
    assert_eq!(status["revision"], renewed["revision"], "{status}");
    assert_eq!(status["routing_mode"], "global", "{status}");
    assert_eq!(session.settings()["routing_mode"], "global");
}

#[test]
fn proxy_credentials_come_from_core() {
    let session = Session::new();
    let refused = |args: &[&str]| {
        let (code, value) = session.run(args);
        (code, value["code"].as_str().map(str::to_string))
    };

    // The routed credential does not depend on a profile; a node's does.
    let routed = session.ok(&["proxy", "credential"]);
    assert_eq!(routed["kind"], "routed");
    assert_eq!(routed["node_id"], "");
    for field in ["username", "password", "listen"] {
        assert!(
            routed[field].as_str().is_some_and(|v| !v.is_empty()),
            "{field} is missing"
        );
    }
    // Both URLs carry the same credential and address.
    let http = routed["http_url"].as_str().unwrap();
    let socks = routed["socks5_url"].as_str().unwrap();
    let address = format!(
        "@{}:{}",
        routed["listen"].as_str().unwrap(),
        routed["port"].as_u64().unwrap()
    );
    assert!(http.starts_with("http://") && http.ends_with(&address));
    assert!(socks.starts_with("socks5h://") && socks.ends_with(&address));
    assert!(http["http://".len()..] == socks["socks5h://".len()..]);
    assert_eq!(
        refused(&["proxy", "credential", TOKYO]),
        (2, Some("NODE_NOT_FOUND".to_string()))
    );

    session.write_profile(&fixture());
    session.ok(&["start"]);

    let node = session.ok(&["proxy", "credential", TOKYO]);
    assert_eq!(node["kind"], "node");
    assert_eq!(node["node_id"], TOKYO);
    assert!(node["username"] != routed["username"]);
    assert!(node["port"] == routed["port"]);
    assert_eq!(
        refused(&["proxy", "credential", "no-such-node"]),
        (2, Some("NODE_NOT_FOUND".to_string()))
    );

    // The endpoint list has no secrets: one entry per node, then routed.
    let listed = session.ok(&["proxy"]);
    let endpoints = listed["endpoints"].as_array().unwrap();
    let who: Vec<(&str, &str)> = endpoints
        .iter()
        .map(|e| (e["kind"].as_str().unwrap(), e["node_id"].as_str().unwrap()))
        .collect();
    assert_eq!(who, [("node", TOKYO), ("node", SAN_JOSE), ("routed", "")]);
    assert!(endpoints
        .iter()
        .all(|e| e.get("password").is_none() && e.get("username").is_none()));

    // The credential is the instance's: asking again gives the same one.
    let again = session.ok(&["proxy", "credential"]);
    assert!(again["username"] == routed["username"] && again["password"] == routed["password"]);
}

#[test]
fn probes_keep_core_s_preconditions_and_a_failed_probe_is_a_result() {
    let session = Session::new();
    let refused = |args: &[&str]| {
        let (code, value) = session.run(args);
        (code, value["code"].as_str().map(str::to_string))
    };
    let availability = |node: &'static str| {
        [
            "probe",
            node,
            "--type",
            "availability",
            "--target",
            "http://probe.example/",
            "--timeout",
            "2s",
        ]
    };

    let not_applied = (5, Some("PROFILE_NOT_APPLIED".to_string()));
    assert_eq!(refused(&["probe", "--all"]), not_applied);
    assert_eq!(refused(&availability(TOKYO)), not_applied);

    session.write_profile(&fixture());
    session.ok(&["start"]);

    // Unknown nodes are refused before anything is dialled.
    let not_found = (2, Some("NODE_NOT_FOUND".to_string()));
    assert_eq!(refused(&["probe", "no-such-node"]), not_found);
    assert_eq!(refused(&availability("no-such-node")), not_found);

    // The in-memory runtime's connection ends at once: the probe ran and
    // failed, which is a result, not a command error.
    let probed = session.ok(&availability(TOKYO));
    assert_eq!(probed["type"], "availability", "{probed}");
    assert_eq!(probed["result"]["node_id"], TOKYO, "{probed}");
    assert_eq!(probed["result"]["success"], false, "{probed}");
    assert!(
        probed["result"]["error_code"]
            .as_str()
            .is_some_and(|code| !code.is_empty()),
        "{probed}"
    );
}
