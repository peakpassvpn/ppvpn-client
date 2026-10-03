//! `ppvpn-core-lab serve` end to end, as the lab scripts drive it: flags,
//! log lines, the session secret and Core API v1 over the Unix socket.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Serve {
    child: Child,
    dir: PathBuf,
    secret: String,
}

impl Serve {
    fn start(name: &str, extra: &[&str]) -> Serve {
        let dir = std::env::temp_dir().join(format!("pl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_ppvpn-core-lab"))
            .arg("serve")
            .args([
                "--socket",
                &path(&dir, "core.sock"),
                "--session-secret-file",
                &path(&dir, "secret"),
            ])
            .args([
                "--state-dir",
                &path(&dir, "state"),
                "--log-file",
                &path(&dir, "core.log"),
                "--exit-on-stdin-close",
            ])
            .args(extra)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !dir.join("secret").exists() || UnixStream::connect(dir.join("core.sock")).is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "serve did not come up: {}",
                log(&dir)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let secret = std::fs::read_to_string(dir.join("secret")).unwrap();
        Serve { child, dir, secret }
    }

    /// (status, body) of one request.
    fn call(&self, method: &str, path: &str, auth: Option<&str>, body: &str) -> (u16, String) {
        let mut stream = UnixStream::connect(self.dir.join("core.sock")).unwrap();
        let auth = auth
            .map(|a| format!("Authorization: Bearer {a}\r\n"))
            .unwrap_or_default();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: core\r\n{auth}X-Request-ID: t1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let status = response[9..12].parse().unwrap();
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    fn post(&self, method: &str, body: &str) -> (u16, String) {
        let secret = self.secret.clone();
        self.call("POST", &format!("/v1/{method}"), Some(&secret), body)
    }

    /// Closes stdin (--exit-on-stdin-close) and waits for the exit.
    fn stop(mut self) -> (std::process::ExitStatus, String, PathBuf) {
        drop(self.child.stdin.take());
        let status = self.child.wait().unwrap();
        (status, log(&self.dir), self.dir.clone())
    }
}

fn path(dir: &Path, name: &str) -> String {
    dir.join(name).to_string_lossy().into_owned()
}

fn log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("core.log")).unwrap_or_default()
}

#[test]
fn serves_core_api_v1_as_the_go_core() {
    let serve = Serve::start("api", &[]);
    assert_eq!(serve.secret.len(), 43);

    let (status, body) = serve.call("POST", "/v1/get-status", None, "{}");
    assert_eq!(status, 401);
    assert!(
        body.contains(r#""code":"UNAUTHENTICATED""#) && body.contains(r#""request_id":"t1""#),
        "{body}"
    );
    let (status, _) = serve.call("POST", "/v1/get-status", Some("wrong"), "{}");
    assert_eq!(status, 401);

    let (status, body) = serve.post("get-version", "{}");
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""ok":true"#) && body.contains(r#""core_api_version":1"#),
        "{body}"
    );

    let (status, body) = serve.post("get-status", "{}");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""state":"stopped""#), "{body}");

    let (status, body) = serve.post("start", "{}");
    assert_eq!(status, 400);
    assert!(
        body.contains(r#""code":"PROFILE_NOT_APPLIED""#) && body.contains(r#""retryable":false"#),
        "{body}"
    );

    let (status, body) = serve.post(
        "apply-profile",
        r#"{"profile":{},"routing_mode":"sideways"}"#,
    );
    assert_eq!(status, 400);
    assert!(
        body.contains(r#""code":"ROUTING_MODE_INVALID""#)
            && body.contains(r#""field":"routing_mode""#),
        "{body}"
    );

    for bad in ["{", r#"{"nope":1}"#, "{} {}"] {
        let (status, body) = serve.post("apply-profile", bad);
        assert_eq!(status, 400, "{bad}");
        assert!(
            body.contains(r#""code":"REQUEST_INVALID""#),
            "{bad}: {body}"
        );
    }

    let (status, body) = serve.post("no-such-method", "{}");
    assert_eq!(status, 404);
    assert!(body.contains(r#""code":"API_NOT_FOUND""#), "{body}");

    // Forwarded to the engine, its error as it is (D2).
    let (status, body) = serve.post("select-node", r#"{"node_id":"jp"}"#);
    assert_eq!(status, 400);
    assert!(body.contains(r#""code":"PROFILE_NOT_APPLIED""#), "{body}");
    for method in ["list-nodes", "get-connections"] {
        let (status, body) = serve.post(method, "{}");
        assert_eq!(
            (status, body.trim()),
            (200, r#"{"request_id":"t1","ok":true,"data":[]}"#)
        );
    }
    let (status, body) = serve.post("get-traffic", "{}");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""upload_bytes":0"#), "{body}");

    // Core API v1's own checks, before the engine.
    for (method, request, code, field) in [
        (
            "pin-ingress",
            r#"{"node_id":"jp","endpoint_key":""}"#,
            "INGRESS_NOT_FOUND",
            "endpoint_key",
        ),
        ("set-system-proxy", "{}", "REQUEST_INVALID", "enabled"),
        (
            "get-local-proxy-credential",
            r#"{"kind":"nope"}"#,
            "REQUEST_INVALID",
            "kind",
        ),
        (
            "get-local-proxy-credential",
            r#"{"kind":"routed","node_id":"jp"}"#,
            "REQUEST_INVALID",
            "node_id",
        ),
        (
            "probe-entrances",
            r#"{"method":"udp"}"#,
            "PROBE_METHOD_UNSUPPORTED",
            "method",
        ),
    ] {
        let (status, body) = serve.post(method, request);
        assert_eq!(status, 400, "{method}: {body}");
        assert!(
            body.contains(&format!(r#""code":"{code}""#))
                && body.contains(&format!(r#""field":"{field}""#)),
            "{method}: {body}"
        );
    }

    let (status, log, dir) = serve.stop();
    assert!(status.success());
    let lines: Vec<&str> = log.lines().collect();
    let starting = lines
        .iter()
        .find(|l| l.contains("msg=\"serve starting\""))
        .expect("serve starting");
    for field in [
        " level=info ",
        " platform=desktop ",
        " tun=false ",
        " tun_stack=mixed ",
        " local_proxy=true ",
        " log_level=info ",
    ] {
        assert!(starting.contains(field), "{field} in {starting}");
    }
    assert!(
        lines
            .iter()
            .any(|l| l.contains("msg=\"serve ready\" socket=")),
        "{log}"
    );
    assert!(lines.iter().any(|l| l.contains("level=info msg=\"request rejected\" path=/v1/start request_id=t1 code=PROFILE_NOT_APPLIED")), "{log}");
    assert!(
        lines
            .iter()
            .any(|l| l.ends_with("msg=\"serve stopping\" reason=\"stdin closed\"")),
        "{log}"
    );
    assert!(
        !dir.join("secret").exists(),
        "the session secret is removed on exit"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn flags_are_checked_as_the_go_core_checks_them() {
    let out = Command::new(env!("CARGO_BIN_EXE_ppvpn-core-lab"))
        .args(["serve", "--nope"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("flag provided but not defined: -nope"));

    let out = Command::new(env!("CARGO_BIN_EXE_ppvpn-core-lab"))
        .args(["serve", "--socket", "/x"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("serve requires --socket, --session-secret-file and --state-dir"),
        "{stderr}"
    );
    assert!(
        stderr.contains("msg=\"serve failed\""),
        "logged too: {stderr}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_ppvpn-core-lab"))
        .args(["serve", "--local-dns-servers", "10.0.0.1"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("--local-dns-servers requires --tun"));

    let out = Command::new(env!("CARGO_BIN_EXE_ppvpn-core-lab"))
        .arg("version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"core_version\""));
}

#[test]
fn a_socket_path_that_is_a_file_is_left_alone() {
    let dir = std::env::temp_dir().join(format!("pl-notsock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("not-a-socket");
    std::fs::write(&file, b"keep me").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ppvpn-core-lab"))
        .arg("serve")
        .args([
            "--socket",
            &path(&dir, "not-a-socket"),
            "--session-secret-file",
            &path(&dir, "secret"),
        ])
        .args(["--state-dir", &path(&dir, "state")])
        .output()
        .unwrap();
    assert!(!out.status.success(), "binding over a file fails");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"keep me",
        "the file is not removed"
    );
    let _ = std::fs::remove_dir_all(dir);
}
