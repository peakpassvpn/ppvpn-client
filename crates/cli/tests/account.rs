//! Login, account, logout and the profile download against a local mock
//! backend, with an in-memory credential store. Tokens are made up at run
//! time; assertions never print them.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ppvpn_cli::buildinfo::{BuildConfig, Profile};
use ppvpn_cli::env::{Env, Os};
use ppvpn_cli::keystore::MemoryStore;
use ppvpn_cli::Hooks;
use serde_json::{json, Value};

/// A value that looks like a credential but is unique to this run.
fn fake(kind: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    // Padded: the client refuses credentials shorter than the real ones.
    format!(
        "{kind}-{:016}-{nanos:016}-{:016}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// An unsigned token with the CLI audience; the client checks `aud` and
/// reads `exp`, the backend verifies signatures.
fn access_token() -> String {
    let claims = json!({"aud": "cli", "exp": 4_102_444_800u64, "jti": fake("jti")});
    format!(
        "e30.{}.{}",
        URL_SAFE_NO_PAD.encode(claims.to_string()),
        fake("sig")
    )
}

fn token_set() -> (u16, String) {
    (200, json!({"access_token": access_token(), "refresh_token": fake("refresh"), "expires_in": 900}).to_string())
}

struct Backend {
    base: String,
    /// `METHOD /path` of each request, in order.
    requests: Arc<Mutex<Vec<String>>>,
    /// Lowercased header block of each request.
    heads: Arc<Mutex<Vec<String>>>,
}

impl Backend {
    /// Serves `responses` to successive requests, one connection each.
    fn serve(responses: impl FnOnce(&str) -> Vec<(u16, String)>) -> Backend {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let responses = responses(&base);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let heads = Arc::new(Mutex::new(Vec::new()));
        let (seen, seen_heads) = (requests.clone(), heads.clone());
        std::thread::spawn(move || {
            let mut responses = responses.into_iter();
            while let Ok((mut socket, _)) = listener.accept() {
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                while let Ok(read) = socket.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let mut parts = text.lines().next().unwrap_or_default().split(' ');
                let (method, path) = (
                    parts.next().unwrap_or_default(),
                    parts.next().unwrap_or_default(),
                );
                if method.is_empty() {
                    continue;
                }
                let Some((status, body)) = responses.next() else {
                    return;
                };
                seen.lock().unwrap().push(format!("{method} {path}"));
                seen_heads.lock().unwrap().push(
                    text.split("\r\n\r\n")
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                );
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        Backend {
            base,
            requests,
            heads,
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

struct Cli {
    home: tempfile::TempDir,
    hooks: Hooks,
    store: Arc<MemoryStore>,
}

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Outcome {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).expect("stdout is one JSON value")
    }
}

impl Cli {
    fn new(base: &str) -> Cli {
        let store = Arc::new(MemoryStore::default());
        let hooks = Hooks {
            build: Some(BuildConfig {
                profile: Profile::Dev,
                api_base: base.to_string(),
            }),
            store: Some(store.clone()),
        };
        Cli {
            home: tempfile::tempdir().unwrap(),
            hooks,
            store,
        }
    }

    fn env(&self) -> Env {
        let home = self.home.path().to_str().unwrap();
        Env::with_vars(Os::current(), &[("HOME", home), ("LANG", "zh_CN.UTF-8")])
    }

    fn run(&self, args: &[&str]) -> Outcome {
        let argv: Vec<std::ffi::OsString> = std::iter::once("ppvpn")
            .chain(args.iter().copied())
            .map(Into::into)
            .collect();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = ppvpn_cli::run_with(&argv, &self.env(), &self.hooks, &mut stdout, &mut stderr);
        Outcome {
            code,
            stdout: String::from_utf8(stdout).unwrap(),
            stderr: String::from_utf8(stderr).unwrap(),
        }
    }
}

fn device_code(base: &str) -> (u16, String) {
    let body = json!({
        "device_code": fake("device"),
        "user_code": "ABCD-EFGH",
        "verification_uri": format!("{base}/dashboard/device/authorize"),
        "verification_uri_complete": format!("{base}/dashboard/device/authorize?user_code=ABCD-EFGH"),
        "expires_in": 600,
        "interval": 1,
    });
    (200, body.to_string())
}

fn user() -> (u16, String) {
    (200, json!({"id": "u-1", "name": "Test User"}).to_string())
}

#[test]
fn login_account_and_logout() {
    let backend = Backend::serve(|base| {
        vec![
            // login
            device_code(base),
            (200, json!({"status": "authorization_pending"}).to_string()),
            (
                200,
                json!({"status": "authorized", "refresh_token": fake("pending")}).to_string(),
            ),
            token_set(),
            user(),
            // account: rotate the credential, then read the account
            (200, json!({"refresh_token": fake("prepared")}).to_string()),
            token_set(),
            user(),
            // logout
            (200, "{}".to_string()),
        ]
    });
    let cli = Cli::new(&backend.base);

    let out = cli.run(&["--json", "login", "--no-browser"]);
    assert_eq!(out.code, 0, "login failed: {}", out.json()["code"]);
    assert_eq!(
        out.json(),
        json!({"ok": true, "status": "authorized", "user_code": "ABCD-EFGH"})
    );
    assert!(out
        .stderr
        .contains("/dashboard/device/authorize?user_code=ABCD-EFGH"));
    assert!(out.stderr.contains("Confirmation code: ABCD-EFGH"));
    assert!(!cli.store.is_empty(), "the login was not saved");

    let out = cli.run(&["account"]);
    assert_eq!((out.code, out.stdout.as_str()), (0, "Test User (u-1)\n"));

    let out = cli.run(&["--json", "logout"]);
    assert_eq!(
        (out.code, out.json()),
        (0, json!({"ok": true, "local_removed": true}))
    );
    assert!(cli.store.is_empty(), "logout left the credential behind");

    assert_eq!(
        backend.requests(),
        [
            "POST /api/v1/auth/device/code",
            "POST /api/v1/auth/device/token",
            "POST /api/v1/auth/device/token",
            "POST /api/v1/auth/device/activate",
            "GET /api/v1/users/me",
            "POST /api/v1/auth/device/refresh",
            "POST /api/v1/auth/device/refresh/commit",
            "GET /api/v1/users/me",
            "POST /api/v1/auth/device/revoke",
        ]
    );
    let heads = backend.heads.lock().unwrap();
    // The CLI's device authorization is the header-less one.
    assert!(heads.iter().all(|head| !head.contains("x-product-aud")));
    assert!(heads[0].contains("accept-language: zh-cn"));
}

#[test]
fn commands_without_a_login_exit_3_without_calling_the_backend() {
    let backend = Backend::serve(|_| Vec::new());
    let cli = Cli::new(&backend.base);
    for args in [
        &["--json", "account"][..],
        &["--json", "logout"],
        &["--json", "start"],
    ] {
        let out = cli.run(args);
        assert_eq!(out.code, 3, "{args:?}");
        assert_eq!(out.json()["code"], "NOT_LOGGED_IN", "{args:?}");
    }
    assert!(backend.requests().is_empty());
}

#[test]
fn a_denied_authorization_is_reported_and_saves_nothing() {
    let backend = Backend::serve(|base| {
        vec![
            device_code(base),
            (200, json!({"status": "access_denied"}).to_string()),
        ]
    });
    let cli = Cli::new(&backend.base);
    let out = cli.run(&["--json", "login", "--no-browser"]);
    assert_eq!(out.code, 3);
    assert_eq!(out.json()["code"], "AUTH_DEVICE_DENIED");
    assert!(cli.store.is_empty());
}

#[test]
fn an_unreachable_backend_is_retryable_exit_4() {
    // Bind and drop a listener to get a port nothing listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cli = Cli::new(&format!("http://127.0.0.1:{port}"));
    let out = cli.run(&["--json", "login", "--no-browser"]);
    assert_eq!(out.code, 4);
    let value = out.json();
    assert_eq!(
        (value["code"].as_str(), value["retryable"].as_bool()),
        (Some("BACKEND_UNAVAILABLE"), Some(true))
    );
}

/// Saves a login in `store` by running the login flow against `backend`'s
/// first four responses.
fn logged_in(cli: &Cli) {
    assert_eq!(cli.run(&["login", "--no-browser"]).code, 0, "login failed");
}

fn login_responses(base: &str) -> Vec<(u16, String)> {
    vec![
        device_code(base),
        (
            200,
            json!({"status": "authorized", "refresh_token": fake("pending")}).to_string(),
        ),
        token_set(),
        user(),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn the_profile_download_refreshes_once_on_401() {
    let profile = json!({"schema_version": 1, "marker": fake("profile")}).to_string();
    let expected = profile.clone();
    let backend = Backend::serve(move |base| {
        let mut responses = login_responses(base);
        responses.extend([
            // restore: rotate, then the first download is rejected
            (200, json!({"refresh_token": fake("prepared")}).to_string()),
            token_set(),
            (
                401,
                json!({"code": "AUTH_TOKEN_EXPIRED", "status": 401}).to_string(),
            ),
            // refresh once more and retry
            (200, json!({"refresh_token": fake("prepared")}).to_string()),
            token_set(),
            (200, profile),
        ]);
        responses
    });
    let cli = Cli::new(&backend.base);
    let (env, hooks) = (cli.env(), cli.hooks.clone());
    tokio::task::spawn_blocking(move || logged_in(&cli))
        .await
        .unwrap();

    let store = hooks.store.clone().unwrap();
    let auth = ppvpn_cli::account::auth(hooks.build.as_ref().unwrap(), &env, store.clone());
    let downloaded = ppvpn_cli::account::download_profile(&auth).await.unwrap();
    assert!(
        downloaded == expected.as_bytes(),
        "the profile bytes were changed in transit"
    );
    assert!(
        store.load().unwrap().is_some(),
        "the login must survive a rotated token"
    );
    assert_eq!(
        &backend.requests()[4..],
        [
            "POST /api/v1/auth/device/refresh",
            "POST /api/v1/auth/device/refresh/commit",
            "GET /api/v1/me/proxy-profile",
            "POST /api/v1/auth/device/refresh",
            "POST /api/v1/auth/device/refresh/commit",
            "GET /api/v1/me/proxy-profile",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_subscription_is_exit_4_and_keeps_the_login() {
    let backend = Backend::serve(|base| {
        let mut responses = login_responses(base);
        responses.extend([
            (200, json!({"refresh_token": fake("prepared")}).to_string()),
            token_set(),
            (
                404,
                json!({"code": "SUBSCRIPTION_EXPIRED", "status": 404}).to_string(),
            ),
        ]);
        responses
    });
    let cli = Cli::new(&backend.base);
    let (env, hooks) = (cli.env(), cli.hooks.clone());
    tokio::task::spawn_blocking(move || logged_in(&cli))
        .await
        .unwrap();

    let store = hooks.store.clone().unwrap();
    let auth = ppvpn_cli::account::auth(hooks.build.as_ref().unwrap(), &env, store.clone());
    let err = ppvpn_cli::account::download_profile(&auth)
        .await
        .unwrap_err();
    assert_eq!(
        (err.exit_code(), err.code.as_str(), err.retryable),
        (4, "SUBSCRIPTION_EXPIRED", false)
    );
    assert!(
        store.load().unwrap().is_some(),
        "a 404 must not log the device out"
    );
}
