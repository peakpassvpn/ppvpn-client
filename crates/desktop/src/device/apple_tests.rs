use super::*;

use crate::auth::test_support::MemoryPlatform;
use crate::cores::test_support::{FakeLauncher, NoService};
use crate::test_backend::{serve, temp_dir, wait_until, Backend};
use crate::{AuthState, ClientConfig, ClientListener, ClientSnapshot, ProbeResult, TrafficSample};
use serde_json::{json, Value};
use std::sync::{Arc, Barrier};
use std::time::Duration;

const TOKEN_1: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TOKEN_2: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

struct Quiet;

impl ClientListener for Quiet {
    fn on_snapshot(&self, _: ClientSnapshot) {}
    fn on_probe_result(&self, _: ProbeResult) {}
    fn on_traffic(&self, _: TrafficSample) {}
}

fn saved_login() -> Arc<MemoryPlatform> {
    MemoryPlatform::with_blob(r#"{"version":1,"active_refresh":"jyr_old"}"#)
}

fn client(base: &str, dir: &str, platform_name: &str) -> Arc<Client> {
    Client::with_parts(
        ClientConfig {
            api_base: base.into(),
            data_dir: dir.into(),
            log_dir: dir.into(),
            platform: platform_name.into(),
            app_version: "1.2.3".into(),
        },
        saved_login(),
        Arc::new(Quiet),
        Arc::new(NoService),
        FakeLauncher::new(),
        Arc::new(crate::sysproxy::tests::FakeWriter::default()),
    )
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn apple_calls(backend: &Backend) -> Vec<(String, Value)> {
    backend.apple_bodies.lock().unwrap().clone()
}

fn finish(client: Arc<Client>, dir: &str) {
    client.shutdown();
    drop(client);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn apple_per_install_binding_registers_rotates_and_unbinds_on_sign_out() {
    for (platform, bundle) in [
        ("ios", "com.peakpassvpn.ppvpn"),
        ("macos", "com.peakpassvpn.ppvpn.desktop"),
    ] {
        let backend = Backend::new();
        let base = serve(backend.clone());
        let dir = temp_dir("ppvpn-apple-bind-test");
        let app = client(&base, &dir, platform);

        wait_until(|| app.snapshot().auth == AuthState::SignedIn);
        std::thread::sleep(Duration::from_millis(150));
        assert!(backend.requests_matching("/devices/").is_empty());

        app.set_push_token_with_environment(
            TOKEN_1.to_uppercase(),
            Some(bundle.into()),
            Some("production".into()),
        );
        wait_until(|| app.session_state().device == DeviceState::Registered);

        // Legacy `/devices/register` and `/devices/unregister` are never called for Apple.
        assert!(backend.requests_matching("/devices/register").is_empty());
        assert!(backend.requests_matching("/devices/unregister").is_empty());

        let saved = read_apple_installation(&dir).expect("saved apple installation");
        let binding_id = saved.binding_id.clone().expect("binding id");
        let first_reg_id = saved.registration_id.clone().expect("registration id");

        let calls = apple_calls(&backend);
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, "POST /api/v1/devices/apple/installations");
        assert_eq!(calls[0].1["platform"], platform);
        assert_eq!(calls[0].1["bundle_id"], bundle);
        assert_eq!(calls[0].1["environment"], "production");
        assert_eq!(
            calls[1].0,
            format!(
                "POST /api/v1/devices/apple/installations/{}/bindings",
                saved.installation_id
            )
        );
        assert!(calls[1].1.get("expected_binding_id").is_none());
        assert_eq!(
            calls[2].0,
            format!(
                "POST /api/v1/devices/apple/installations/{}/registrations",
                saved.installation_id
            )
        );
        assert_eq!(calls[2].1["binding_id"], binding_id);
        assert!(calls[2].1.get("expected_registration_id").is_none());
        assert_eq!(calls[2].1["token"], TOKEN_1);

        // Repeating the identical token does nothing.
        app.set_push_token_with_environment(
            TOKEN_1.into(),
            Some(bundle.into()),
            Some("production".into()),
        );
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(apple_calls(&backend).len(), 3);

        // Token rotation updates the same binding with expected_registration_id.
        app.set_push_token_with_environment(
            TOKEN_2.into(),
            Some(bundle.into()),
            Some("production".into()),
        );
        wait_until(|| apple_calls(&backend).len() == 5);
        wait_until(|| app.session_state().device == DeviceState::Registered);
        let calls = apple_calls(&backend);
        assert_eq!(calls[3].0, "POST /api/v1/devices/apple/installations");
        assert_eq!(
            calls[4].0,
            format!(
                "POST /api/v1/devices/apple/installations/{}/registrations",
                saved.installation_id
            )
        );
        assert_eq!(calls[4].1["binding_id"], binding_id);
        assert_eq!(calls[4].1["expected_registration_id"], first_reg_id);
        assert_eq!(calls[4].1["token"], TOKEN_2);

        // Sign-out calls DELETE .../bindings/{binding_id} and keeps the installation proof.
        block_on(app.logout()).unwrap();
        let calls = apple_calls(&backend);
        assert_eq!(calls.len(), 6);
        assert_eq!(
            calls[5].0,
            format!(
                "DELETE /api/v1/devices/apple/installations/{}/bindings/{}",
                saved.installation_id, binding_id
            )
        );
        assert_eq!(calls[5].1["installation_secret"], saved.installation_secret);

        let after_logout = read_apple_installation(&dir).expect("installation proof kept");
        assert_eq!(after_logout.installation_id, saved.installation_id);
        assert_eq!(after_logout.installation_secret, saved.installation_secret);
        assert!(after_logout.binding_id.is_none());
        assert!(after_logout.registration_id.is_none());
        assert_eq!(read_os_token(&dir).unwrap().token, TOKEN_2);

        finish(app, &dir);
    }
}

#[test]
fn token_ownership_conflict_does_not_retry_or_prompt_and_recovers_on_new_token_or_relaunch() {
    let backend = Backend::new();
    backend.apple_next_errors.lock().unwrap().push_back((
        "/registrations",
        "409 Conflict",
        PROBLEM_APPLE_TOKEN_OWNERSHIP_CONFLICT,
    ));
    let base = serve(backend.clone());
    let dir = temp_dir("ppvpn-apple-token-conflict");
    let app = client(&base, &dir, "ios");
    wait_until(|| app.snapshot().auth == AuthState::SignedIn);

    app.set_push_token_with_environment(
        TOKEN_1.into(),
        Some("com.peakpassvpn.ppvpn".into()),
        Some("production".into()),
    );
    wait_until(|| app.session_state().device == DeviceState::Deferred);

    // No error banner or re-login prompt, and no retry during this launch.
    assert_eq!(app.snapshot().auth, AuthState::SignedIn);
    assert!(app.snapshot().last_error.is_none());
    let count_after_conflict = apple_calls(&backend).len();
    assert_eq!(count_after_conflict, 3);

    // Duplicate callback of the same token or background ensure_device_registered does not retry.
    app.set_push_token_with_environment(
        TOKEN_1.into(),
        Some("com.peakpassvpn.ppvpn".into()),
        Some("production".into()),
    );
    let session = app.current_session().unwrap();
    block_on(app.ensure_device_registered(session));
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(apple_calls(&backend).len(), count_after_conflict);

    // Next app launch tries the normal registration flow again (say conflict persists once more).
    app.shutdown();
    drop(app);
    let relaunched = client(&base, &dir, "ios");
    wait_until(|| relaunched.session_state().device == DeviceState::Registered);
    assert_eq!(relaunched.snapshot().auth, AuthState::SignedIn);
    assert!(relaunched.snapshot().last_error.is_none());

    // And if a token rotation hits ownership conflict, a subsequent new OS token registers normally.
    backend.apple_next_errors.lock().unwrap().push_back((
        "/registrations",
        "409 Conflict",
        PROBLEM_APPLE_TOKEN_OWNERSHIP_CONFLICT,
    ));
    relaunched.set_push_token_with_environment(
        TOKEN_2.into(),
        Some("com.peakpassvpn.ppvpn".into()),
        Some("production".into()),
    );
    wait_until(|| relaunched.session_state().device == DeviceState::Deferred);
    let calls_before_rotate = apple_calls(&backend).len();
    relaunched.set_push_token_with_environment(
        TOKEN_1.into(),
        Some("com.peakpassvpn.ppvpn".into()),
        Some("production".into()),
    );
    wait_until(|| relaunched.session_state().device == DeviceState::Registered);
    assert!(apple_calls(&backend).len() > calls_before_rotate);

    finish(relaunched, &dir);
}

#[test]
fn stale_binding_and_registration_conflicts_reread_installation_and_retry_once() {
    // 1. DEVICE_BINDING_CONFLICT on bind -> re-queries installations and retries once.
    let backend = Backend::new();
    backend.apple_next_errors.lock().unwrap().push_back((
        "/bindings",
        "409 Conflict",
        PROBLEM_APPLE_BINDING_CONFLICT,
    ));
    let base = serve(backend.clone());
    let dir = temp_dir("ppvpn-apple-cas-conflict");
    let app = client(&base, &dir, "ios");
    wait_until(|| app.snapshot().auth == AuthState::SignedIn);

    app.set_push_token_with_environment(
        TOKEN_1.into(),
        Some("com.peakpassvpn.ppvpn".into()),
        Some("production".into()),
    );
    wait_until(|| app.session_state().device == DeviceState::Registered);
    let methods: Vec<String> = apple_calls(&backend).into_iter().map(|(m, _)| m).collect();
    assert_eq!(methods.len(), 5);
    assert_eq!(methods[0], "POST /api/v1/devices/apple/installations");
    assert!(methods[1].ends_with("/bindings"));
    assert_eq!(methods[2], "POST /api/v1/devices/apple/installations");
    assert!(methods[3].ends_with("/bindings"));
    assert!(methods[4].ends_with("/registrations"));

    // 2. DEVICE_REGISTRATION_CONFLICT on token rotation -> re-queries installations for latest registration_id and retries once.
    let saved = read_apple_installation(&dir).unwrap();
    let newer_server_reg = uuid::Uuid::new_v4().to_string();
    backend
        .apple_installations
        .lock()
        .unwrap()
        .get_mut(&saved.installation_id)
        .unwrap()
        .registration_id = Some(newer_server_reg.clone());
    backend.apple_next_errors.lock().unwrap().push_back((
        "/registrations",
        "409 Conflict",
        PROBLEM_APPLE_REGISTRATION_CONFLICT,
    ));

    let before_rotate = apple_calls(&backend).len();
    app.set_push_token_with_environment(
        TOKEN_2.into(),
        Some("com.peakpassvpn.ppvpn".into()),
        Some("production".into()),
    );
    wait_until(|| app.session_state().device == DeviceState::Registered);
    let calls = apple_calls(&backend);
    assert_eq!(calls.len() - before_rotate, 4);
    assert_eq!(
        calls[before_rotate].0,
        "POST /api/v1/devices/apple/installations"
    );
    assert!(calls[before_rotate + 1].0.ends_with("/registrations"));
    assert_eq!(
        calls[before_rotate + 2].0,
        "POST /api/v1/devices/apple/installations"
    );
    assert!(calls[before_rotate + 3].0.ends_with("/registrations"));
    assert_eq!(
        calls.last().unwrap().1["expected_registration_id"],
        newer_server_reg
    );

    finish(app, &dir);
}

#[test]
fn request_conflict_resends_with_a_new_request_id_and_scope_conflict_creates_new_installation_id() {
    let backend = Backend::new();
    backend.apple_next_errors.lock().unwrap().push_back((
        "/installations",
        "409 Conflict",
        PROBLEM_APPLE_SCOPE_CONFLICT,
    ));
    backend.apple_next_errors.lock().unwrap().push_back((
        "/registrations",
        "409 Conflict",
        PROBLEM_APPLE_REQUEST_CONFLICT,
    ));
    let base = serve(backend.clone());
    let dir = temp_dir("ppvpn-apple-req-scope-conflict");
    let seeded = load_or_create_apple_installation(
        &dir,
        "macos",
        "com.peakpassvpn.ppvpn.desktop",
        "production",
    )
    .unwrap();

    let app = client(&base, &dir, "macos");
    wait_until(|| app.snapshot().auth == AuthState::SignedIn);
    app.set_push_token_with_environment(
        TOKEN_1.into(),
        Some("com.peakpassvpn.ppvpn.desktop".into()),
        Some("production".into()),
    );
    wait_until(|| app.session_state().device == DeviceState::Registered);

    let updated = read_apple_installation(&dir).unwrap();
    assert_ne!(
        updated.installation_id, seeded.installation_id,
        "DEVICE_INSTALLATION_SCOPE_CONFLICT must create a new installation_id"
    );

    let reg_calls: Vec<Value> = apple_calls(&backend)
        .into_iter()
        .filter(|(path, _)| path.ends_with("/registrations"))
        .map(|(_, body)| body)
        .collect();
    assert_eq!(reg_calls.len(), 2);
    assert_ne!(
        reg_calls[0]["request_id"], reg_calls[1]["request_id"],
        "DEVICE_REQUEST_CONFLICT must resend with a fresh request_id"
    );

    finish(app, &dir);
}

#[test]
fn sign_out_waits_for_in_flight_apple_registration_then_unbinds_cleanly() {
    let backend = Backend::new();
    let gate = Arc::new(Barrier::new(2));
    backend
        .apple_register_gates
        .lock()
        .unwrap()
        .insert(TOKEN_1.into(), gate.clone());
    let base = serve(backend.clone());
    let dir = temp_dir("ppvpn-apple-logout-race");
    let app = client(&base, &dir, "macos");
    wait_until(|| app.snapshot().auth == AuthState::SignedIn);

    app.set_push_token_with_environment(
        TOKEN_1.into(),
        Some("com.peakpassvpn.ppvpn.desktop".into()),
        Some("production".into()),
    );
    wait_until(|| {
        apple_calls(&backend)
            .iter()
            .any(|(path, _)| path.ends_with("/registrations"))
    });

    let signing_out = app.clone();
    let logout_task = std::thread::spawn(move || block_on(signing_out.logout()));
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        !apple_calls(&backend)
            .iter()
            .any(|(path, _)| path.starts_with("DELETE ")),
        "unbind must wait until the in-flight registration finishes"
    );

    gate.wait();
    logout_task.join().unwrap().unwrap();

    let calls = apple_calls(&backend);
    assert!(
        calls
            .last()
            .unwrap()
            .0
            .starts_with("DELETE /api/v1/devices/apple/installations/"),
        "final Apple call at sign-out must be DELETE .../bindings/{{binding_id}}"
    );
    assert!(read_apple_installation(&dir).unwrap().binding_id.is_none());

    finish(app, &dir);
}

#[test]
fn unconfigured_apple_backend_defers_without_error() {
    let backend = Backend::new();
    *backend.apple_config.lock().unwrap() = json!({
        "configured": false,
        "bundle_id_ios": "",
        "bundle_id_macos": "",
        "environments": [],
    });
    let base = serve(backend.clone());
    let dir = temp_dir("ppvpn-apple-unconfigured");
    let app = client(&base, &dir, "ios");
    wait_until(|| app.snapshot().auth == AuthState::SignedIn);

    app.set_push_token_with_environment(
        TOKEN_1.into(),
        Some("com.peakpassvpn.ppvpn".into()),
        Some("production".into()),
    );
    wait_until(|| app.session_state().device == DeviceState::Deferred);
    assert!(apple_calls(&backend).is_empty());
    assert!(app.snapshot().last_error.is_none());

    finish(app, &dir);
}

#[test]
fn macos_startup_removes_local_legacy_agent_files_without_deleting_old_backend_device() {
    let backend = Backend::new();
    let base = serve(backend.clone());
    let dir = temp_dir("ppvpn-macos-legacy-cleanup");
    let legacy_install = installation_id(&dir).unwrap();
    let agent = AgentFile {
        api_base: base.clone(),
        device_id: 42,
        push_token: crate::test_backend::PUSH_TOKEN.into(),
        installation_id: legacy_install.clone(),
    };
    write_private(&agent_file_path(&dir), &serde_json::to_vec(&agent).unwrap()).unwrap();
    std::fs::write(Path::new(&dir).join(AGENT_CURSOR_FILE), "{}").unwrap();
    std::fs::write(Path::new(&dir).join(crate::push_shown::SHOWN_FILE), "{}").unwrap();

    let app = client(&base, &dir, "macos");
    assert!(!agent_file_path(&dir).exists());
    assert!(!Path::new(&dir).join(AGENT_CURSOR_FILE).exists());
    assert!(!Path::new(&dir).join(crate::push_shown::SHOWN_FILE).exists());
    assert_eq!(installation_id(&dir).unwrap(), legacy_install);

    wait_until(|| app.snapshot().auth == AuthState::SignedIn);
    app.set_push_token_with_environment(
        TOKEN_1.into(),
        Some("com.peakpassvpn.ppvpn.desktop".into()),
        Some("production".into()),
    );
    wait_until(|| app.session_state().device == DeviceState::Registered);

    // Same-account legacy devices are taken over directly on the backend:
    // no `DELETE /api/v1/devices/42` request is made.
    assert!(backend.requests_matching("/devices/42").is_empty());

    finish(app, &dir);
}
