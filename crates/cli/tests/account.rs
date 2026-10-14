//! Login, account, logout and the profile download against a local mock
//! backend, with an in-memory credential store. Tokens are made up at run
//! time; assertions never print them.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ppvpn_account::auth::{CredentialStore, StoreFailure};
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

/// `GET /me/teams`: a personal team, the current one, and a disabled one.
fn teams() -> (u16, String) {
    (
        200,
        json!({"items": [
            {"id": "t-1", "name": "Personal", "is_personal": true, "is_default": false, "status": "active"},
            {"id": "t-2", "name": "Quiet Quartz", "is_personal": false, "is_default": true, "status": "active"},
            {"id": "t-3", "name": "Old Team", "is_personal": false, "is_default": false, "status": "disabled"},
        ]})
        .to_string(),
    )
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
            teams(),
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
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (0, "Test User (u-1)\nTeam: Quiet Quartz (t-2)\n")
    );

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
            "GET /api/v1/me/teams",
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
        &["--json", "team"],
        &["--json", "team", "use", "t-1"],
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

/// A secret store that cannot be used: none on the machine, or locked.
struct BrokenStore {
    locked: bool,
}

impl BrokenStore {
    fn failure(&self) -> StoreFailure {
        StoreFailure {
            locked: self.locked,
            message: "no secret store in this test".into(),
        }
    }
}

impl CredentialStore for BrokenStore {
    fn load(&self) -> Result<Option<Vec<u8>>, StoreFailure> {
        Err(self.failure())
    }
    fn save(&self, _: Vec<u8>) -> Result<(), StoreFailure> {
        Err(self.failure())
    }
    fn delete(&self) -> Result<(), StoreFailure> {
        Err(self.failure())
    }
}

#[test]
fn login_without_a_usable_secret_store_fails_before_the_backend() {
    let backend = Backend::serve(|base| vec![device_code(base)]);
    for (locked, code) in [
        (false, "CREDENTIAL_STORE_UNAVAILABLE"),
        (true, "CREDENTIAL_STORE_LOCKED"),
    ] {
        let mut cli = Cli::new(&backend.base);
        cli.hooks.store = Some(Arc::new(BrokenStore { locked }));
        let out = cli.run(&["--json", "login", "--no-browser"]);
        assert_eq!(out.code, 8, "locked: {locked}");
        assert_eq!(out.json()["code"], code);
        assert!(!out.stderr.contains("Confirmation code"));
    }
    // No device authorization was started that nobody could finish.
    assert!(backend.requests().is_empty());
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
async fn a_profile_404_says_there_is_no_usable_subscription() {
    // The backend's problem+json for an account without an active
    // subscription (or whose nodes are not published yet).
    let backend = Backend::serve(|base| {
        let mut responses = login_responses(base);
        responses.extend([
            (200, json!({"refresh_token": fake("prepared")}).to_string()),
            token_set(),
            (
                404,
                json!({"type": "about:blank", "title": "Not Found", "status": 404,
                       "code": "NOT_FOUND", "detail": "no active subscription"})
                .to_string(),
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
    let auth = ppvpn_cli::account::auth(hooks.build.as_ref().unwrap(), &env, store);
    let err = ppvpn_cli::account::download_profile(&auth)
        .await
        .unwrap_err();
    assert_eq!(
        (err.exit_code(), err.code.as_str(), err.retryable),
        (4, "NOT_FOUND", false)
    );
    assert_eq!(
        err.message,
        "no active subscription, or its nodes are not ready yet (try again later)"
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

/// The error `download_profile` gives when the backend answers the profile
/// request with `status` and `body`.
/// `after` answers the requests that follow it (the team lookup).
async fn profile_error(
    status: u16,
    body: Value,
    after: Vec<(u16, String)>,
) -> ppvpn_cli::error::CliError {
    let backend = Backend::serve(move |base| {
        let mut responses = login_responses(base);
        responses.extend([
            (200, json!({"refresh_token": fake("prepared")}).to_string()),
            token_set(),
            (status, body.to_string()),
        ]);
        responses.extend(after);
        responses
    });
    let cli = Cli::new(&backend.base);
    let (env, hooks) = (cli.env(), cli.hooks.clone());
    tokio::task::spawn_blocking(move || logged_in(&cli))
        .await
        .unwrap();
    let store = hooks.store.clone().unwrap();
    let auth = ppvpn_cli::account::auth(hooks.build.as_ref().unwrap(), &env, store);
    ppvpn_cli::account::download_profile(&auth)
        .await
        .unwrap_err()
}

#[tokio::test(flavor = "multi_thread")]
async fn subscription_codes_pick_the_message_and_keep_the_code_and_exit() {
    for (code, message, retryable) in [
        ("SUBSCRIPTION_REQUIRED", "no active subscription", false),
        (
            "PROXY_NODES_NOT_READY",
            "the subscription's nodes are not ready yet (try again later)",
            true,
        ),
        (
            "SUBSCRIPTION_EXPIRED",
            "the subscription has expired",
            false,
        ),
        ("SUBSCRIPTION_NOT_FOUND", "no active subscription", false),
    ] {
        let err = profile_error(
            404,
            json!({"type": "about:blank", "title": "Not Found", "status": 404,
                   "code": code, "detail": message}),
            Vec::new(),
        )
        .await;
        assert_eq!(
            (
                err.exit_code(),
                err.code.as_str(),
                err.message.as_str(),
                err.retryable
            ),
            (4, code, message, retryable)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_profile_carries_the_backend_reason_and_the_team() {
    let err = profile_error(
        404,
        json!({"type": "about:blank", "title": "Not Found", "status": 404,
               "code": "NOT_FOUND", "detail": "Subscription is\n being refunded"}),
        vec![teams()],
    )
    .await;
    assert_eq!((err.exit_code(), err.code.as_str()), (4, "NOT_FOUND"));
    assert_eq!(
        err.message,
        "no active subscription, or its nodes are not ready yet (try again later) \
         (backend: Subscription is being refunded; team: Quiet Quartz (t-2)); \
         ppvpn team lists your teams"
    );

    // Without the team list the error stands as it is.
    let err = profile_error(
        404,
        json!({"code": "SUBSCRIPTION_REQUIRED", "detail": "no active subscription"}),
        vec![(403, json!({"code": "403012"}).to_string())],
    )
    .await;
    assert_eq!(err.message, "no active subscription");
}

#[test]
fn team_lists_and_switches() {
    let switched = |id: &str, name: &str| {
        (
            200,
            json!({"id": id, "name": name, "is_personal": id == "t-1", "token": access_token()})
                .to_string(),
        )
    };
    let backend = Backend::serve(|base| {
        let mut responses = login_responses(base);
        let refresh = || {
            [
                (200, json!({"refresh_token": fake("prepared")}).to_string()),
                token_set(),
            ]
        };
        // team
        responses.extend(refresh());
        responses.push(teams());
        // team use Personal
        responses.extend(refresh());
        responses.push(teams());
        responses.push(switched("t-1", "Personal"));
        // team use t-2: already the current team
        responses.extend(refresh());
        responses.push(teams());
        // team use t-3: disabled
        responses.extend(refresh());
        responses.push(teams());
        // team use nobody
        responses.extend(refresh());
        responses.push(teams());
        // team use x, from a team that cannot list the others
        responses.extend(refresh());
        responses.push((403, json!({"code": "403012"}).to_string()));
        responses.push(switched("x", "X"));
        responses
    });
    let cli = Cli::new(&backend.base);
    logged_in(&cli);

    let out = cli.run(&["team"]);
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (
            0,
            "  Personal (t-1) personal\n* Quiet Quartz (t-2)\n  Old Team (t-3) disabled\n"
        )
    );

    let out = cli.run(&["--json", "team", "use", "Personal"]);
    assert_eq!(
        out.json(),
        json!({"ok": true, "changed": true, "restart_required": false, "team":
            {"id": "t-1", "name": "Personal", "personal": true, "current": true, "status": "active"}})
    );

    let out = cli.run(&["team", "use", "t-2"]);
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (0, "Already on team Quiet Quartz (t-2).\n")
    );

    let out = cli.run(&["--json", "team", "use", "t-3"]);
    assert_eq!(
        (out.code, out.json()["code"].as_str()),
        (3, Some("TEAM_DISABLED"))
    );

    let out = cli.run(&["--json", "team", "use", "nobody"]);
    assert_eq!(
        (out.code, out.json()["code"].as_str()),
        (2, Some("TEAM_NOT_FOUND"))
    );

    let out = cli.run(&["team", "use", "x"]);
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (0, "Switched to team X (x).\n")
    );

    let requests = backend.requests();
    let after_login = &requests[4..];
    assert_eq!(
        after_login
            .iter()
            .filter(|r| r.as_str() == "POST /api/v1/me/switch-team")
            .count(),
        2,
        "{after_login:?}"
    );
}
