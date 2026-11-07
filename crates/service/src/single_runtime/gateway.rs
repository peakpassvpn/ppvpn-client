//! Authenticated IPC front end for ONE user-owned forwarding Engine. Capture
//! authority remains in core-wire; this gateway cannot launch a privileged core.
use super::auth::{Peer, Session};
use crate::protocol::{
    self, Command, CoreApiPayload, HandshakePayload, HandshakeResponse, Request, Response,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use ppvpn_core_wire::ForwarderIdentity;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
const AUTH_LIFETIME: Duration = Duration::from_secs(300);
const MAX_SESSIONS: usize = 256;
const MAX_PEER_SESSIONS: usize = 16;
#[async_trait]
pub trait Forwarder: Send + Sync + 'static {
    fn identity(&self) -> ForwarderIdentity;
    async fn call(&self, path: &str, body: Value) -> Result<Value, ppvpn_engine_host::ApiError>;
}
pub struct EngineForwarder {
    engine: ppvpn_core::Engine,
    operation: tokio::sync::Mutex<()>,
    capture_deadline: Mutex<Option<Instant>>,
}
impl EngineForwarder {
    pub async fn new(config: ppvpn_core::EngineConfig) -> Result<Arc<Self>> {
        if !matches!(
            config.role,
            ppvpn_core::Role::Standard | ppvpn_core::Role::Shared
        ) {
            return Err(anyhow!("USER_RUNTIME_REQUIRES_USER_ROLE"));
        }
        let engine = ppvpn_core::Engine::new(config)
            .await
            .map_err(|e| anyhow!("{}", e.code))?;
        Ok(Arc::new(Self {
            engine,
            operation: tokio::sync::Mutex::new(()),
            capture_deadline: Mutex::new(None),
        }))
    }
    pub fn network(&self, state: ppvpn_core::NetworkState) -> Result<()> {
        self.engine
            .set_network_state(state)
            .map_err(|e| anyhow!("{}", e.code))
    }
    pub fn capture_expired(&self) -> bool {
        self.capture_deadline
            .lock()
            .is_some_and(|d| Instant::now() >= d)
    }
    pub async fn reconcile_lost_capture(&self) -> Result<()> {
        let _operation = self.operation.lock().await;
        let Ok(status) = self.engine.capture_status() else {
            return Ok(());
        };
        if let Some(lease) = status.lease {
            self.engine
                .detach_capture(ppvpn_core_wire::DetachCapture {
                    protocol_version: 1,
                    expected_epoch: status.epoch,
                    lease,
                })
                .await
                .map_err(|_| anyhow!("CAPTURE_LEASE_RECONCILIATION_FAILED"))?;
        }
        *self.capture_deadline.lock() = None;
        Ok(())
    }
    pub fn logs(&self) -> ppvpn_core::LogReceiver {
        self.engine.logs()
    }
    pub async fn until_fatal(&self) {
        let states = self
            .engine
            .subscribe(&[ppvpn_core::EventKind::StateChanged]);
        let _ = ppvpn_engine_host::until_fatal(&self.engine, states).await;
    }
    /// Only the owning agent calls this at logout/service shutdown, never on UI
    /// disconnect, capture expiry or VPN-off.
    pub async fn shutdown(&self) -> Result<()> {
        let report = self
            .engine
            .shutdown()
            .await
            .map_err(|e| anyhow!("{}", e.code))?;
        if !report.leftovers.is_empty() {
            return Err(anyhow!("USER_RUNTIME_SHUTDOWN_LEFTOVERS"));
        }
        Ok(())
    }
}
#[async_trait]
impl Forwarder for EngineForwarder {
    fn identity(&self) -> ForwarderIdentity {
        self.engine.runtime_observation().owner
    }
    async fn call(&self, path: &str, body: Value) -> Result<Value, ppvpn_engine_host::ApiError> {
        let _operation = self.operation.lock().await;
        if path == "/v1/renew-capture-lease" {
            let lease: ppvpn_core_wire::CaptureLease =
                serde_json::from_value(body).map_err(|_| lease_error())?;
            if self.capture_expired() {
                return Err(lease_error());
            }
            let status = self.engine.capture_status().map_err(|_| lease_error())?;
            if status.phase != ppvpn_core_wire::CapturePhase::Attached
                || status.lease.as_ref() != Some(&lease)
            {
                return Err(lease_error());
            }
            *self.capture_deadline.lock() = Some(Instant::now() + Duration::from_secs(45));
            return serde_json::to_value(status).map_err(|_| lease_error());
        }
        let result = ppvpn_engine_host::dispatch(&self.engine, path, body).await?;
        if path == "/v1/attach-capture" {
            *self.capture_deadline.lock() = Some(Instant::now() + Duration::from_secs(45));
        } else if matches!(path, "/v1/detach-capture" | "/v1/stop") {
            *self.capture_deadline.lock() = None;
        }
        Ok(result)
    }
}
fn lease_error() -> ppvpn_engine_host::ApiError {
    ppvpn_engine_host::ApiError {
        code: "CAPTURE_LEASE_MISMATCH".into(),
        field: None,
        message: "capture lease is absent or stale".into(),
        retryable: false,
        capture_status: None,
    }
}
struct Authenticated {
    peer: Peer,
    expires: Instant,
    session: Arc<Mutex<Session>>,
}
pub struct Gateway<B: Forwarder> {
    backend: Arc<B>,
    uid: u32,
    service_generation: u64,
    auth: Mutex<HashMap<String, Authenticated>>,
}
impl<B: Forwarder> Gateway<B> {
    pub fn new(backend: Arc<B>, uid: u32) -> Result<Arc<Self>> {
        if uid == 0 {
            return Err(anyhow!("USER_RUNTIME_ROOT_FORBIDDEN"));
        }
        let mut bytes = [0; 8];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| anyhow!("RANDOM_UNAVAILABLE"))?;
        Ok(Arc::new(Self {
            backend,
            uid,
            service_generation: u64::from_ne_bytes(bytes).max(1),
            auth: Mutex::new(HashMap::new()),
        }))
    }
    pub fn identity(&self) -> ForwarderIdentity {
        self.backend.identity()
    }
    /// peer MUST be established by the OS/signature/installation verifier.
    pub async fn handle(&self, peer: Peer, request: Request) -> Result<Response> {
        if peer.uid != self.uid || peer.pid == 0 || peer.pid_version == 0 {
            return Err(anyhow!("USER_RUNTIME_PEER_REJECTED"));
        }
        let now = Instant::now();
        if request.command == Command::Handshake {
            return self.handshake(peer, request, now);
        }
        let session = {
            let mut auth = self.auth.lock();
            auth.retain(|_, entry| entry.expires > now);
            auth.get(request.auth_token.as_deref().unwrap_or_default())
                .map(|entry| entry.session.clone())
        }
        .ok_or_else(|| anyhow!("AUTH_SESSION_INVALID"))?;
        session
            .lock()
            .verify(
                peer,
                &request,
                self.service_generation,
                now,
                protocol::now_epoch(),
            )
            .map_err(|e| anyhow!("AUTH_REJECTED:{e:?}"))?;
        let id = request.id;
        let result = match request.command {
            Command::GetStatus => Ok(
                json!({"running":true,"pid":std::process::id(),"bin_path":"","mode":"user-runtime",
                "service_version":env!("CARGO_PKG_VERSION"),"service_build_id":protocol::SERVICE_BUILD_ID,
                "session_id":self.backend.identity().instance,"generation":self.backend.identity().generation,
                "profile_revision":null,"lease_expires_at_ms":null}),
            ),
            Command::GetVersion => Ok(
                json!({"service":"ppvpn-user-runtime","version":env!("CARGO_PKG_VERSION"),
                "build_id":protocol::SERVICE_BUILD_ID}),
            ),
            Command::CoreApi => match serde_json::from_value::<CoreApiPayload>(request.payload) {
                Err(_) => Err(("REQUEST_INVALID".into(), None)),
                Ok(call)
                    if call.session.session_id != self.backend.identity().instance
                        || call.session.generation != self.backend.identity().generation =>
                {
                    Err(("CAPTURE_LEASE_MISMATCH".into(), None))
                }
                Ok(call)
                    if call.path == "/v1/stop"
                        && call.body.get("owner_reason").and_then(Value::as_str)
                            != Some("session_ended") =>
                {
                    Err(("USER_RUNTIME_LIFETIME_OWNED".into(), None))
                }
                Ok(call) => self
                    .backend
                    .call(&call.path, call.body)
                    .await
                    .map_err(|error| {
                        (
                            format!("{}: {}", error.code, error.message),
                            serde_json::to_value(error).ok(),
                        )
                    }),
            },
            Command::Connect => Err(("CAPTURE_UNSUPPORTED".into(), None)),
            _ => Err(("BROKER_COMMAND_REQUIRED".into(), None)),
        };
        let response = match result {
            Ok(data) => session.lock().sign_response(id, true, Some(data), None),
            Err((error, data)) => session.lock().sign_response(id, false, data, Some(error)),
        };
        response
    }
    fn handshake(&self, peer: Peer, request: Request, now: Instant) -> Result<Response> {
        if !request.is_fresh() || request.id.is_empty() || request.id.len() > 128 {
            return Err(anyhow!("HANDSHAKE_INVALID"));
        }
        let hello: HandshakePayload = serde_json::from_value(request.payload)?;
        if !(32..=128).contains(&hello.client_nonce.len()) {
            return Err(anyhow!("HANDSHAKE_NONCE_INVALID"));
        }
        let mut auth = self.auth.lock();
        auth.retain(|_, entry| entry.expires > now);
        if auth.len() >= MAX_SESSIONS
            || auth.values().filter(|entry| entry.peer == peer).count() >= MAX_PEER_SESSIONS
        {
            return Err(anyhow!("AUTH_CAPACITY"));
        }
        let mut key = [0; 32];
        SystemRandom::new()
            .fill(&mut key)
            .map_err(|_| anyhow!("RANDOM_UNAVAILABLE"))?;
        let token = uuid::Uuid::new_v4().to_string();
        let session = Session::new(
            peer,
            key,
            token.clone(),
            self.service_generation,
            now,
            AUTH_LIFETIME,
        )
        .map_err(|_| anyhow!("AUTH_IDENTITY_INVALID"))?;
        let data = serde_json::to_value(HandshakeResponse {
            auth_token: token.clone(),
            auth_key: hex::encode(key),
            expires_at: protocol::now_epoch() + AUTH_LIFETIME.as_secs(),
        })?;
        auth.insert(
            token,
            Authenticated {
                peer,
                expires: now + AUTH_LIFETIME,
                session: Arc::new(Mutex::new(session)),
            },
        );
        Ok(Response::unsigned(request.id, true, Some(data), None))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        calls: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl Forwarder for Fake {
        fn identity(&self) -> ForwarderIdentity {
            ForwarderIdentity {
                instance: "same-forwarder".into(),
                generation: 3,
            }
        }
        async fn call(&self, path: &str, _: Value) -> Result<Value, ppvpn_engine_host::ApiError> {
            self.calls.lock().push(path.into());
            Ok(json!({"protocol_version":1}))
        }
    }
    fn peer() -> Peer {
        Peer {
            uid: 501,
            pid: 9,
            pid_version: 2,
            audit_session: 4,
        }
    }
    async fn auth(gateway: &Gateway<Fake>) -> (String, Vec<u8>) {
        let hello = Request {
            id: "hello".into(),
            timestamp: protocol::now_epoch(),
            command: Command::Handshake,
            payload: json!({"client_nonce":"01234567890123456789012345678901"}),
            auth_token: None,
            signature: String::new(),
        };
        let reply = gateway.handle(peer(), hello).await.unwrap();
        let info: HandshakeResponse = serde_json::from_value(reply.data.unwrap()).unwrap();
        (info.auth_token, hex::decode(info.auth_key).unwrap())
    }
    #[tokio::test]
    async fn gateway_has_one_backend_and_ui_stop_cannot_destroy_it() {
        let backend = Arc::new(Fake {
            calls: Mutex::new(vec![]),
        });
        let gateway = Gateway::new(backend.clone(), 501).unwrap();
        let (token, key) = auth(&gateway).await;
        for (id, path) in [
            ("one", "/v1/get-status"),
            ("two", "/v1/stop"),
            ("three", "/v1/get-capture-capabilities"),
        ] {
            let req = Request::signed(
                id.into(),
                Command::CoreApi,
                json!({"session_id":"same-forwarder","generation":3,"path":path,"body":{}}),
                Some(token.clone()),
                &key,
            )
            .unwrap();
            let response = gateway.handle(peer(), req).await.unwrap();
            assert!(response.verify_with(&key).unwrap());
            assert_eq!(response.success, path != "/v1/stop");
        }
        assert_eq!(
            backend.calls.lock().as_slice(),
            ["/v1/get-status", "/v1/get-capture-capabilities"]
        );
        assert_eq!(gateway.identity(), backend.identity());
        let next = auth(&gateway).await;
        assert_ne!(token, next.0);
        assert_eq!(gateway.identity(), backend.identity());
    }
    #[tokio::test]
    async fn gateway_refuses_cross_user_stale_owner_replay_and_broker_commands() {
        let gateway = Gateway::new(
            Arc::new(Fake {
                calls: Mutex::new(vec![]),
            }),
            501,
        )
        .unwrap();
        let (token, key) = auth(&gateway).await;
        let req = Request::signed(
            "req".into(),
            Command::GetStatus,
            json!({}),
            Some(token.clone()),
            &key,
        )
        .unwrap();
        let mut wrong = peer();
        wrong.uid = 502;
        assert!(gateway.handle(wrong, req.clone()).await.is_err());
        gateway.handle(peer(), req.clone()).await.unwrap();
        assert!(gateway.handle(peer(), req).await.is_err());
        let req = Request::signed(
            "stale".into(),
            Command::CoreApi,
            json!({"session_id":"old-forwarder","generation":3,"path":"/v1/get-status","body":{}}),
            Some(token.clone()),
            &key,
        )
        .unwrap();
        assert!(!gateway.handle(peer(), req).await.unwrap().success);
        let req = Request::signed(
            "broker".into(),
            Command::Connect,
            json!({}),
            Some(token),
            &key,
        )
        .unwrap();
        assert_eq!(
            gateway.handle(peer(), req).await.unwrap().error.as_deref(),
            Some("CAPTURE_UNSUPPORTED")
        );
    }
    #[tokio::test]
    async fn actual_engine_adapter_advertises_unsupported_not_restart() {
        let dir = std::env::temp_dir().join(format!("ppvpn-runtime-test-{}", uuid::Uuid::new_v4()));
        let backend = EngineForwarder::new(ppvpn_core::EngineConfig::new(
            ppvpn_core::Role::Standard,
            ppvpn_core::Platform::Macos,
            dir.clone(),
        ))
        .await
        .unwrap();
        let identity = backend.identity();
        let caps = backend
            .call("/v1/get-capture-capabilities", json!({}))
            .await
            .unwrap();
        let caps: ppvpn_core_wire::CaptureCapabilities = serde_json::from_value(caps).unwrap();
        assert!(!caps.supports_live_capture());
        let result=backend.call("/v1/attach-capture",json!({"protocol_version":1,"expected_epoch":0,"spec":{"resource":{"kind":"host_lease","lease_id":"test"},"ipv6":true}})).await.unwrap_err();
        assert_eq!(result.code, "CAPTURE_UNSUPPORTED");
        assert_eq!(identity, backend.identity());
        backend.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(all(any(test, feature = "macos-capture-acceptance"), target_os = "macos"))]
#[path = "gateway/mixed_tests.rs"]
mod mixed_tests;

#[cfg(all(feature = "macos-capture-acceptance", target_os = "macos"))]
pub async fn native_acceptance() {
    mixed_tests::exercise(Arc::new(super::host::Host::default()), true).await;
}
