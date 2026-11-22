//! Path-routed mock backend for tests that need several endpoints at once:
//! sign-in restore, account, messages, device registration, the push queue
//! and the health route. Keeps request lines (with query) for assertions.

// Shared by several test modules; not every helper is used by each.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::auth::test_support::token_set;

pub(crate) const PUSH_TOKEN: &str = "ppd_test_token";

#[derive(Clone, Debug)]
pub(crate) struct MockAppleInstallation {
    pub(crate) secret: String,
    pub(crate) platform: String,
    pub(crate) bundle_id: String,
    pub(crate) environment: String,
    pub(crate) binding_id: Option<String>,
    pub(crate) registration_id: Option<String>,
    pub(crate) user_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) closed: bool,
    pub(crate) token: Option<String>,
}

#[derive(Default)]
pub(crate) struct Backend {
    pub(crate) messages: Mutex<Vec<Value>>,
    pub(crate) deliveries: Mutex<HashMap<String, u64>>,
    /// Answer every message endpoint with 403 (old token scope).
    pub(crate) forbidden: Mutex<bool>,
    pub(crate) requests: Mutex<Vec<String>>,
    pub(crate) unread: Mutex<u64>,
    /// Push queue: items not yet acked are delivered.
    pub(crate) push_items: Mutex<Vec<Value>>,
    pub(crate) acked: Mutex<Vec<u64>>,
    /// The push token the queue accepts (401 otherwise).
    pub(crate) push_token: Mutex<String>,
    /// Status for `POST /devices/register` (e.g. 403 before the scope).
    pub(crate) register_status: Mutex<Option<&'static str>>,
    /// Bodies of `POST /devices/register` and `/devices/unregister`.
    pub(crate) device_bodies: Mutex<Vec<Value>>,
    /// Answer of `GET /devices/apple/config`.
    pub(crate) apple_config: Mutex<Value>,
    /// Stateful per-installation CAS records for `/devices/apple/*`.
    pub(crate) apple_installations: Mutex<HashMap<String, MockAppleInstallation>>,
    /// Recorded `("<METHOD> <path>", body)` of `/devices/apple/*` mutations.
    pub(crate) apple_bodies: Mutex<Vec<(String, Value)>>,
    /// One-shot `(path_fragment, status_line, problem_code)` overrides.
    pub(crate) apple_next_errors: Mutex<VecDeque<(&'static str, &'static str, &'static str)>>,
    /// Pause one Apple registration for `token` after recording its body.
    pub(crate) apple_register_gates: Mutex<HashMap<String, Arc<Barrier>>>,
    pub(crate) apple_user_id: Mutex<String>,
    pub(crate) apple_session_id: Mutex<String>,
    /// Request heads (lower-cased) of `GET /api/v1/health`.
    pub(crate) health_heads: Mutex<Vec<String>>,
    /// Body of `GET /api/v1/me/proxy-profile`; none answers 404
    /// `SUBSCRIPTION_NOT_FOUND`.
    pub(crate) profile: Mutex<Option<String>>,
}

impl Backend {
    pub(crate) fn new() -> Arc<Self> {
        let backend = Self::default();
        *backend.push_token.lock().unwrap() = PUSH_TOKEN.to_string();
        *backend.apple_config.lock().unwrap() = json!({
            "configured": true,
            "bundle_id_ios": "com.peakpassvpn.ppvpn",
            "bundle_id_macos": "com.peakpassvpn.ppvpn.desktop",
            "environments": ["sandbox", "production"],
        });
        *backend.apple_user_id.lock().unwrap() = "u7".to_string();
        *backend.apple_session_id.lock().unwrap() = "sid-1".to_string();
        Arc::new(backend)
    }

    fn take_apple_error(&self, path: &str) -> Option<(&'static str, String)> {
        let mut queue = self.apple_next_errors.lock().unwrap();
        let index = queue
            .iter()
            .position(|(needle, _, _)| path.contains(needle))?;
        let (_, status, code) = queue.remove(index)?;
        Some((status, json!({ "code": code }).to_string()))
    }

    pub(crate) fn add_message(&self, id: u64) {
        self.messages.lock().unwrap().push(json!({
            "id": id, "user_id": 7, "title": format!("t{id}"), "content": "c",
            "type": "subscription", "event_key": "subscription.expire_reminder_3d",
            "severity": "important", "deep_link": "/app/subscriptions/88",
            "push": true, "is_read": false, "created_at": "2026-09-29T00:00:00Z",
            "unknown_field": 1
        }));
    }

    pub(crate) fn add_push(&self, id: u64, created_at: &str) {
        self.push_items.lock().unwrap().push(json!({
            "id": id, "title": format!("p{id}"), "body": "b",
            "deep_link": "/app/subscriptions/88", "severity": "critical",
            "event_key": "invoice.issued", "created_at": created_at
        }));
    }

    /// A push about inbox message `message_id` (`add_push` has none, like a
    /// broadcast).
    pub(crate) fn add_push_for_message(&self, id: u64, created_at: &str, message_id: u64) {
        self.add_push(id, created_at);
        let mut items = self.push_items.lock().unwrap();
        if let Some(item) = items.last_mut() {
            item["message_id"] = json!(message_id);
        }
    }

    pub(crate) fn requests_matching(&self, needle: &str) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.contains(needle))
            .cloned()
            .collect()
    }

    fn pending_push(&self, after: u64) -> Vec<Value> {
        let acked = self.acked.lock().unwrap().clone();
        self.push_items
            .lock()
            .unwrap()
            .iter()
            .filter(|item| {
                let id = item["id"].as_u64().unwrap();
                id > after && !acked.contains(&id)
            })
            .cloned()
            .collect()
    }

    fn route(
        &self,
        method: &str,
        path: &str,
        query: &str,
        headers: &str,
        body: &str,
    ) -> (&'static str, String) {
        let param = |name: &str| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
                .and_then(|value| value.parse::<u64>().ok())
        };
        if path.starts_with("/api/v1/messages") && *self.forbidden.lock().unwrap() {
            return ("403 Forbidden", r#"{"code":"403001"}"#.into());
        }
        if path.starts_with("/api/v1/push/") {
            let token = self.push_token.lock().unwrap().to_ascii_lowercase();
            if headers.contains("x-push-token: ppd_forbidden") {
                return ("403 Forbidden", r#"{"code":"PUSH_TOKEN_REVOKED"}"#.into());
            }
            if !headers.contains(&format!("x-push-token: {token}")) {
                return (
                    "401 Unauthorized",
                    r#"{"code":"PUSH_TOKEN_INVALID"}"#.into(),
                );
            }
        }
        match (method, path) {
            // The backend's unauthenticated health route (200, no-store).
            ("GET", "/api/v1/health") => {
                self.health_heads.lock().unwrap().push(headers.to_string());
                ("200 OK", r#"{"status":"ok"}"#.into())
            }
            ("POST", "/api/v1/auth/device/refresh") => {
                ("200 OK", r#"{"refresh_token":"jyr_prepared"}"#.into())
            }
            ("POST", "/api/v1/auth/device/refresh/commit") => ("200 OK", token_set("jyr_a")),
            ("POST", "/api/v1/auth/device/revoke") => ("200 OK", r#"{"revoked":true}"#.into()),
            ("GET", "/api/v1/users/me") => {
                ("200 OK", r#"{"id":"u7","name":"N","avatar":""}"#.into())
            }
            ("GET", "/api/v1/me/teams") => (
                "200 OK",
                r#"{"items":[{"id":"t1","name":"P","is_personal":true,"is_default":true}]}"#.into(),
            ),
            ("GET", "/api/v1/me/proxy-profile") => match self.profile.lock().unwrap().clone() {
                Some(profile) => ("200 OK", profile),
                None => (
                    "404 Not Found",
                    r#"{"code":"SUBSCRIPTION_NOT_FOUND"}"#.into(),
                ),
            },
            ("GET", "/api/v1/messages/unread-count") => (
                "200 OK",
                json!({"count": *self.unread.lock().unwrap()}).to_string(),
            ),
            ("GET", "/api/v1/messages") => {
                let mut all = self.messages.lock().unwrap().clone();
                all.reverse();
                let size = param("page_size").unwrap_or(20) as usize;
                let total = all.len();
                let items: Vec<Value> = all.into_iter().take(size).collect();
                (
                    "200 OK",
                    json!({"items": items, "total": total, "page": 1, "page_size": size})
                        .to_string(),
                )
            }
            ("PUT", path) if path.starts_with("/api/v1/messages/") => {
                ("200 OK", r#"{"updated":true}"#.into())
            }
            ("GET", path) if path.starts_with("/api/v1/messages/deliveries/") => {
                let delivery_id = path.trim_start_matches("/api/v1/messages/deliveries/");
                let target_id = self.deliveries.lock().unwrap().get(delivery_id).copied();
                let found = target_id.and_then(|id| {
                    self.messages
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|message| message["id"].as_u64() == Some(id))
                        .cloned()
                });
                match found {
                    Some(message) => ("200 OK", message.to_string()),
                    None => ("404 Not Found", r#"{"code":"NOT_FOUND"}"#.into()),
                }
            }
            ("GET", "/api/v1/devices/apple/config") => {
                if let Some(error) = self.take_apple_error(path) {
                    return error;
                }
                ("200 OK", self.apple_config.lock().unwrap().to_string())
            }
            ("POST", "/api/v1/devices/apple/installations") => {
                let request: Value = serde_json::from_str(body).unwrap_or_default();
                self.apple_bodies
                    .lock()
                    .unwrap()
                    .push((format!("{method} {path}"), request.clone()));
                if let Some(error) = self.take_apple_error(path) {
                    return error;
                }
                let id = request["installation_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let secret = request["installation_secret"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let platform = request["platform"].as_str().unwrap_or_default().to_string();
                let bundle_id = request["bundle_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let environment = request["environment"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let user = self.apple_user_id.lock().unwrap().clone();
                let sid = self.apple_session_id.lock().unwrap().clone();
                let mut map = self.apple_installations.lock().unwrap();
                let entry = map
                    .entry(id.clone())
                    .or_insert_with(|| MockAppleInstallation {
                        secret: secret.clone(),
                        platform: platform.clone(),
                        bundle_id: bundle_id.clone(),
                        environment: environment.clone(),
                        binding_id: None,
                        registration_id: None,
                        user_id: None,
                        session_id: None,
                        closed: false,
                        token: None,
                    });
                if entry.secret != secret {
                    return (
                        "403 Forbidden",
                        r#"{"code":"DEVICE_INSTALLATION_PROOF_INVALID"}"#.into(),
                    );
                }
                if entry.platform != platform
                    || entry.bundle_id != bundle_id
                    || entry.environment != environment
                {
                    return (
                        "409 Conflict",
                        r#"{"code":"DEVICE_INSTALLATION_SCOPE_CONFLICT"}"#.into(),
                    );
                }
                let bound_user = entry.user_id.as_deref() == Some(user.as_str());
                let bound_session = bound_user
                    && entry.session_id.as_deref() == Some(sid.as_str())
                    && !entry.closed;
                let registration_id = if bound_session {
                    entry.registration_id.clone()
                } else {
                    None
                };
                (
                    "200 OK",
                    json!({
                        "installation_id": id,
                        "binding_id": entry.binding_id,
                        "registration_id": registration_id,
                        "bound_to_current_user": bound_user,
                        "bound_to_current_session": bound_session,
                    })
                    .to_string(),
                )
            }
            ("POST", path)
                if path.starts_with("/api/v1/devices/apple/installations/")
                    && path.ends_with("/bindings") =>
            {
                let request: Value = serde_json::from_str(body).unwrap_or_default();
                self.apple_bodies
                    .lock()
                    .unwrap()
                    .push((format!("{method} {path}"), request.clone()));
                if let Some(error) = self.take_apple_error(path) {
                    return error;
                }
                let id = path
                    .trim_start_matches("/api/v1/devices/apple/installations/")
                    .trim_end_matches("/bindings");
                let secret = request["installation_secret"].as_str().unwrap_or_default();
                let expected = request.get("expected_binding_id").and_then(Value::as_str);
                let user = self.apple_user_id.lock().unwrap().clone();
                let sid = self.apple_session_id.lock().unwrap().clone();
                let mut map = self.apple_installations.lock().unwrap();
                let Some(entry) = map.get_mut(id) else {
                    return (
                        "403 Forbidden",
                        r#"{"code":"DEVICE_INSTALLATION_PROOF_INVALID"}"#.into(),
                    );
                };
                if entry.secret != secret {
                    return (
                        "403 Forbidden",
                        r#"{"code":"DEVICE_INSTALLATION_PROOF_INVALID"}"#.into(),
                    );
                }
                if entry.binding_id.as_deref() != expected {
                    return (
                        "409 Conflict",
                        r#"{"code":"DEVICE_BINDING_CONFLICT"}"#.into(),
                    );
                }
                let binding_id = uuid::Uuid::new_v4().to_string();
                entry.binding_id = Some(binding_id.clone());
                entry.registration_id = None;
                entry.user_id = Some(user);
                entry.session_id = Some(sid);
                entry.closed = false;
                (
                    "200 OK",
                    json!({
                        "installation_id": id,
                        "binding_id": binding_id,
                        "registration_id": Value::Null,
                        "bound_to_current_user": true,
                        "bound_to_current_session": true,
                    })
                    .to_string(),
                )
            }
            ("POST", path)
                if path.starts_with("/api/v1/devices/apple/installations/")
                    && path.ends_with("/registrations") =>
            {
                let request: Value = serde_json::from_str(body).unwrap_or_default();
                self.apple_bodies
                    .lock()
                    .unwrap()
                    .push((format!("{method} {path}"), request.clone()));
                let token_str = request["token"].as_str().unwrap_or_default().to_string();
                let gate = self.apple_register_gates.lock().unwrap().remove(&token_str);
                if let Some(gate) = gate {
                    gate.wait();
                }
                if let Some(error) = self.take_apple_error(path) {
                    return error;
                }
                let id = path
                    .trim_start_matches("/api/v1/devices/apple/installations/")
                    .trim_end_matches("/registrations");
                let secret = request["installation_secret"].as_str().unwrap_or_default();
                let binding = request["binding_id"].as_str().unwrap_or_default();
                let expected = request
                    .get("expected_registration_id")
                    .and_then(Value::as_str);
                let user = self.apple_user_id.lock().unwrap().clone();
                let sid = self.apple_session_id.lock().unwrap().clone();
                let mut map = self.apple_installations.lock().unwrap();
                let Some(entry) = map.get_mut(id) else {
                    return (
                        "403 Forbidden",
                        r#"{"code":"DEVICE_INSTALLATION_PROOF_INVALID"}"#.into(),
                    );
                };
                if entry.secret != secret {
                    return (
                        "403 Forbidden",
                        r#"{"code":"DEVICE_INSTALLATION_PROOF_INVALID"}"#.into(),
                    );
                }
                if entry.binding_id.as_deref() != Some(binding)
                    || entry.user_id.as_deref() != Some(user.as_str())
                    || entry.session_id.as_deref() != Some(sid.as_str())
                {
                    return (
                        "409 Conflict",
                        r#"{"code":"DEVICE_BINDING_CONFLICT"}"#.into(),
                    );
                }
                if entry.closed {
                    return (
                        "403 Forbidden",
                        r#"{"code":"DEVICE_BINDING_CLOSED"}"#.into(),
                    );
                }
                if entry.registration_id.as_deref() != expected {
                    return (
                        "409 Conflict",
                        r#"{"code":"DEVICE_REGISTRATION_CONFLICT"}"#.into(),
                    );
                }
                let registration_id = uuid::Uuid::new_v4().to_string();
                entry.registration_id = Some(registration_id.clone());
                entry.token = Some(token_str);
                (
                    "200 OK",
                    json!({
                        "id": 77,
                        "registration_id": registration_id,
                        "token_hash_prefix": "01234567",
                    })
                    .to_string(),
                )
            }
            ("DELETE", path)
                if path.starts_with("/api/v1/devices/apple/installations/")
                    && path.contains("/bindings/") =>
            {
                let request: Value = serde_json::from_str(body).unwrap_or_default();
                self.apple_bodies
                    .lock()
                    .unwrap()
                    .push((format!("{method} {path}"), request.clone()));
                if let Some(error) = self.take_apple_error(path) {
                    return error;
                }
                let rest = path.trim_start_matches("/api/v1/devices/apple/installations/");
                let Some((id, binding_id)) = rest.split_once("/bindings/") else {
                    return ("404 Not Found", r#"{"code":"NOT_FOUND"}"#.into());
                };
                let secret = request["installation_secret"].as_str().unwrap_or_default();
                let mut map = self.apple_installations.lock().unwrap();
                let Some(entry) = map.get_mut(id) else {
                    return (
                        "403 Forbidden",
                        r#"{"code":"DEVICE_INSTALLATION_PROOF_INVALID"}"#.into(),
                    );
                };
                if entry.secret != secret {
                    return (
                        "403 Forbidden",
                        r#"{"code":"DEVICE_INSTALLATION_PROOF_INVALID"}"#.into(),
                    );
                }
                if entry.binding_id.as_deref() == Some(binding_id) && !entry.closed {
                    entry.binding_id = Some(uuid::Uuid::new_v4().to_string());
                    entry.registration_id = None;
                    entry.closed = true;
                }
                ("204 No Content", String::new())
            }
            ("POST", "/api/v1/devices/register") => {
                if let Some(status) = *self.register_status.lock().unwrap() {
                    return (
                        status,
                        r#"{"code":"DEVICE_DESKTOP_SESSION_REQUIRED"}"#.into(),
                    );
                }
                let request: Value = serde_json::from_str(body).unwrap_or_default();
                self.device_bodies.lock().unwrap().push(request.clone());
                let desktop = request["platform"]
                    .as_str()
                    .is_some_and(|platform| platform.starts_with("desktop-"));
                if desktop {
                    return (
                        "200 OK",
                        json!({"id": 42, "platform": request["platform"],
                               "push_token": *self.push_token.lock().unwrap()})
                        .to_string(),
                    );
                }
                if request["token"].as_str().unwrap_or_default().is_empty() {
                    return (
                        "400 Bad Request",
                        r#"{"code":"DEVICE_TOKEN_REQUIRED"}"#.into(),
                    );
                }
                (
                    "200 OK",
                    json!({"id": 43, "platform": request["platform"], "token_hash_prefix": "0a1b"})
                        .to_string(),
                )
            }
            ("POST", "/api/v1/devices/unregister") => {
                let request: Value = serde_json::from_str(body).unwrap_or_default();
                self.device_bodies.lock().unwrap().push(request);
                ("200 OK", r#"{"message":"device unregistered"}"#.into())
            }
            ("DELETE", "/api/v1/devices/42") => ("204 No Content", String::new()),
            ("GET", "/api/v1/push/pull") => {
                let after = param("after").unwrap_or(0);
                let wait = param("wait").unwrap_or(25);
                if wait > 25 {
                    return ("422 Unprocessable Entity", "{}".into());
                }
                let deadline = Instant::now() + Duration::from_secs(wait);
                loop {
                    let items = self.pending_push(after);
                    if !items.is_empty() || Instant::now() >= deadline {
                        let items: Vec<Value> = items.into_iter().take(50).collect();
                        let cursor = items
                            .iter()
                            .filter_map(|item| item["id"].as_u64())
                            .max()
                            .unwrap_or(after);
                        return (
                            "200 OK",
                            json!({"items": items, "cursor": cursor}).to_string(),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            ("POST", "/api/v1/push/ack") => {
                let request: Value = serde_json::from_str(body).unwrap_or_default();
                let ids: Vec<u64> = request["ids"]
                    .as_array()
                    .map(|ids| ids.iter().filter_map(Value::as_u64).collect())
                    .unwrap_or_default();
                self.acked.lock().unwrap().extend(&ids);
                ("200 OK", json!({"acked": ids.len()}).to_string())
            }
            _ => ("404 Not Found", r#"{"code":"NOT_FOUND"}"#.into()),
        }
    }
}

/// Serves `backend` on a local port; returns the base URL.
pub(crate) fn serve(backend: Arc<Backend>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { return };
            let backend = backend.clone();
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body) = loop {
                    let Ok(read) = socket.read(&mut chunk) else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&request[..end]).into_owned();
                    let length = head
                        .to_ascii_lowercase()
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        let body = String::from_utf8_lossy(&request[end + 4..end + 4 + length])
                            .into_owned();
                        break (head, body);
                    }
                };
                let line = head.lines().next().unwrap_or_default().to_string();
                let mut parts = line.split(' ');
                let method = parts.next().unwrap_or_default().to_string();
                let target = parts.next().unwrap_or_default().to_string();
                backend
                    .requests
                    .lock()
                    .unwrap()
                    .push(format!("{method} {target}"));
                let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                let (status, response_body) =
                    backend.route(&method, path, query, &head.to_ascii_lowercase(), &body);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                    response_body.len()
                );
                let _ = socket.write_all(response.as_bytes());
            });
        }
    });
    base
}

pub(crate) fn temp_dir(prefix: &str) -> String {
    std::env::temp_dir()
        .join(format!("{prefix}-{}", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn wait_until(what: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !what() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
