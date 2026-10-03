//! The Rust `ppvpn-core` (ppvpn-core#45, docs/host-integration.md), linked in
//! process. Behind the `rust-core` feature while the Go core still runs the
//! connection; the hosts switch over in one release.
//!
//! With `PPVPN_RUST_CORE=1` the standard instance runs on an in-process
//! [`ppvpn_core::Engine`] instead of the spawned Go core: [`EngineLauncher`]
//! creates it and [`EngineTransport`] serves the Core API v1 paths this crate
//! calls from it, in the IPC path's JSON shapes and error envelope, so the
//! typed helpers in `core_ipc` work unchanged.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ppvpn_core::{
    ApplyRequest, Engine, EngineConfig, EngineState, Event, EventItem, EventKind, EventReceiver,
    LocalProxyConfig, LogConfig, LogLevel, LogSink, Platform, ProbeAvailabilityRequest,
    ProbeEntrancesRequest, Role, RoutingMode,
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::core_ipc::{self, BoxFuture, CoreCallError, CoreTransport};
use crate::errors::{ClientErrorInfo, ErrorCode};
use crate::standard::{CoreLauncher, Launched};
use crate::ClientConfig;

/// Set to `1` to run the standard instance on the Rust core.
pub(crate) const RUST_CORE_ENV: &str = "PPVPN_RUST_CORE";

/// Engine and sail versions, for diagnostics.
#[allow(dead_code)]
pub(crate) fn version() -> ppvpn_core::VersionInfo {
    Engine::version()
}

/// The standard launcher `RUST_CORE_ENV`'s value asks for; `None` keeps the
/// Go core.
pub(crate) fn launcher(
    config: &ClientConfig,
    switch: Option<&OsStr>,
) -> Option<Arc<dyn CoreLauncher>> {
    (switch == Some(OsStr::new("1")))
        .then(|| Arc::new(EngineLauncher::new(config)) as Arc<dyn CoreLauncher>)
}

// ---------------------------------------------------------------------------
// Launcher
// ---------------------------------------------------------------------------

/// Creates a standard [`Engine`] in this process.
pub(crate) struct EngineLauncher {
    state_dir: PathBuf,
    log_dir: PathBuf,
    platform: Platform,
    /// Authority of the API the profile comes from (`allowed_rule_set_hosts`).
    rule_set_hosts: Vec<String>,
}

impl EngineLauncher {
    pub(crate) fn new(config: &ClientConfig) -> Self {
        Self {
            // Not the Go core's `state`: the two keep different files there.
            state_dir: Path::new(&config.data_dir)
                .join(crate::storage::CORE_DIR)
                .join("engine"),
            log_dir: PathBuf::from(&config.log_dir),
            platform: platform(&config.platform),
            rule_set_hosts: core_ipc::rule_set_hosts(&config.api_base),
        }
    }

    fn engine_config(&self) -> EngineConfig {
        // The Go core's log file; the engine appends, the host rotates.
        let log = self.log_dir.join(format!(
            "ppvpn-core.{}.log",
            crate::standard::utc_date(SystemTime::now())
        ));
        EngineConfig::new(Role::Standard, self.platform, self.state_dir.clone())
            .with_local_proxy(LocalProxyConfig::default())
            .with_system_proxy(true)
            .with_log(LogConfig::new(LogLevel::Info, LogSink::File { path: log }))
    }

    async fn create(&self) -> Result<Launched, ClientErrorInfo> {
        let _ = std::fs::create_dir_all(&self.log_dir);
        std::fs::create_dir_all(&self.state_dir).map_err(|error| {
            ClientErrorInfo::new(
                ErrorCode::StandardCoreFailed,
                format!("create engine dir: {error}"),
            )
        })?;
        let engine = Engine::new(self.engine_config()).await.map_err(|error| {
            ClientErrorInfo::new(
                ErrorCode::StandardCoreFailed,
                format!("STANDARD_ENGINE_FAILED: {error}"),
            )
        })?;
        tracing::info!(
            core = %Engine::version().core_version,
            "standard core: in-process Rust engine created"
        );
        let (stop, stop_rx) = oneshot::channel::<()>();
        let owned = engine.clone();
        let states = engine.subscribe(&[EventKind::StateChanged]);
        // A stop request, a dropped sender or a Fatal state shuts the engine
        // down. Fatal reads as an unexpected exit, so the standard instance's
        // supervisor recreates the engine and applies again, as it restarts
        // a crashed Go core.
        let exited = Box::pin(async move {
            let status = tokio::select! {
                _ = stop_rx => "stopped".to_string(),
                reason = until_fatal(&owned, states) => format!("fatal: {reason}"),
            };
            match owned.shutdown().await {
                Ok(report) if report.leftovers.is_empty() => {}
                Ok(report) => {
                    tracing::warn!(leftovers = ?report.leftovers, "engine shutdown left state")
                }
                Err(error) => tracing::warn!(%error, "engine shutdown failed"),
            }
            status
        });
        Ok(Launched {
            transport: Arc::new(EngineTransport { engine }),
            exited,
            stop,
            rule_set_hosts: Some(self.rule_set_hosts.clone()),
            accepts_routing_mode: true,
            accepts_routed_proxy: true,
        })
    }
}

/// Waits for the engine to reach `Fatal` and returns the reason as JSON.
/// Logs `Degraded` reasons on the way: the engine heals those itself. Never
/// returns once the subscription closes (the engine shut down), so a stop
/// request decides.
async fn until_fatal(engine: &Engine, mut states: EventReceiver) -> String {
    // A Fatal between Engine::new and the subscription has no event.
    if let Some(reason) = fatal_reason(&engine.status().state) {
        return reason;
    }
    loop {
        let state = match states.recv().await {
            Some(EventItem::Event {
                event: Event::StateChanged { state, .. },
            }) => state,
            // Fell behind: the current state is what counts.
            Some(EventItem::Lagged { .. }) => engine.status().state,
            Some(_) => continue,
            None => std::future::pending().await,
        };
        if let Some(reason) = fatal_reason(&state) {
            return reason;
        }
    }
}

/// The reason of a `Fatal` state as JSON; logs the reasons of a `Degraded`.
fn fatal_reason(state: &EngineState) -> Option<String> {
    match state {
        EngineState::Fatal { reason } => {
            let reason = serde_json::to_string(reason).unwrap_or_default();
            tracing::warn!(%reason, "standard engine fatal");
            Some(reason)
        }
        EngineState::Degraded { reasons } => {
            let reasons = serde_json::to_string(reasons).unwrap_or_default();
            tracing::info!(%reasons, "standard engine degraded");
            None
        }
        _ => None,
    }
}

impl CoreLauncher for EngineLauncher {
    fn launch(&self) -> BoxFuture<'_, Result<Launched, ClientErrorInfo>> {
        Box::pin(self.create())
    }
}

/// `ClientConfig::platform` (`macos` / `windows` / `linux`); anything else
/// is this build's OS.
fn platform(name: &str) -> Platform {
    match name {
        "macos" => Platform::Macos,
        "windows" => Platform::Windows,
        "linux" => Platform::Linux,
        _ if cfg!(target_os = "macos") => Platform::Macos,
        _ if cfg!(windows) => Platform::Windows,
        _ => Platform::Linux,
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// Serves Core API v1 calls from an in-process [`Engine`].
pub(crate) struct EngineTransport {
    engine: Engine,
}

impl CoreTransport for EngineTransport {
    fn call<'a>(
        &'a self,
        path: &'static str,
        body: Value,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Value, CoreCallError>> {
        Box::pin(async move {
            tokio::time::timeout(timeout, self.dispatch(path, body))
                .await
                .map_err(|_| CoreCallError::Transport(format!("{path}: timed out")))?
        })
    }
}

impl EngineTransport {
    async fn dispatch(&self, path: &str, body: Value) -> Result<Value, CoreCallError> {
        let engine = &self.engine;
        match path {
            "/v1/get-version" => {
                let mut version = data(Engine::version())?;
                version["core_api_version"] = json!(1);
                Ok(version)
            }
            "/v1/validate-profile" => {
                reply(Engine::validate(&apply_request(&body)?).map(|()| json!({ "valid": true })))
            }
            "/v1/apply-profile" => reply(engine.apply(apply_request(&body)?).await),
            "/v1/start" => reply(engine.start().await.map(|()| json!({}))),
            "/v1/stop" => reply(engine.stop().await.map(|()| json!({}))),
            "/v1/get-status" => status(engine),
            "/v1/list-nodes" => data(engine.nodes()),
            "/v1/get-selected-node" => data(engine.selected_node()),
            "/v1/select-node" => {
                let node_id = string(&body, "node_id")?;
                reply(engine.select_node(&node_id).await.map(|()| json!({})))
            }
            "/v1/pin-ingress" => {
                let node_id = string(&body, "node_id")?;
                let endpoint_key = body.get("endpoint_key").and_then(Value::as_str);
                reply(
                    engine
                        .pin_ingress(&node_id, endpoint_key)
                        .await
                        .map(|()| json!({})),
                )
            }
            "/v1/probe-entrances" => {
                let request: ProbeEntrancesRequest = decode(body)?;
                reply(engine.probe_entrances(request).await)
            }
            "/v1/probe-availability" => {
                let request: ProbeAvailabilityRequest = decode(body)?;
                reply(engine.probe_availability(request).await)
            }
            "/v1/get-local-proxy-metadata" => reply(engine.local_proxy_metadata()),
            "/v1/get-local-proxy-credential" => {
                if body.get("kind").and_then(Value::as_str) == Some("routed") {
                    reply(engine.local_proxy_routed_credential())
                } else {
                    reply(engine.local_proxy_credential(&string(&body, "node_id")?))
                }
            }
            "/v1/set-system-proxy" => {
                let enabled = body
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| request_invalid("enabled is required"))?;
                reply(engine.set_system_proxy_listener(enabled).await)
            }
            "/v1/get-traffic" => data(engine.traffic()),
            "/v1/get-connections" => data(engine.connections()),
            // A stream on the IPC path, which this crate does not read; the
            // engine's events are `Engine::subscribe`.
            _ => Err(CoreCallError::Api {
                code: "CORE_API_UNSUPPORTED".to_string(),
                message: format!("{path} is not served by the in-process engine"),
                retryable: false,
            }),
        }
    }
}

/// `get-status` in the Go core's shape. Go has no `degraded`: the engine
/// still forwards then, so it reads `running`; `fatal` stays as is.
fn status(engine: &Engine) -> Result<Value, CoreCallError> {
    let mut status = data(engine.status())?;
    if status["state"] == "degraded" {
        status["state"] = json!("running");
    }
    Ok(status)
}

/// The `apply-profile` / `validate-profile` body as an [`ApplyRequest`].
fn apply_request(body: &Value) -> Result<ApplyRequest, CoreCallError> {
    // A missing profile stays empty: the engine reports PROFILE_REQUIRED.
    let profile = match body.get("profile") {
        None | Some(Value::Null) => Vec::new(),
        Some(profile) => serde_json::to_vec(profile)
            .map_err(|error| request_invalid(&format!("encode profile: {error}")))?,
    };
    let mode = body
        .get("routing_mode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mode = RoutingMode::parse(mode).map_err(api_error)?;
    let hosts = body
        .get("allowed_rule_set_hosts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    Ok(ApplyRequest::new(profile)
        .with_routing_mode(mode)
        .with_allowed_rule_set_hosts(hosts))
}

fn reply<T: Serialize>(result: Result<T, ppvpn_core::Error>) -> Result<Value, CoreCallError> {
    data(result.map_err(api_error)?)
}

fn data<T: Serialize>(value: T) -> Result<Value, CoreCallError> {
    serde_json::to_value(value)
        .map_err(|error| CoreCallError::Transport(format!("encode engine reply: {error}")))
}

fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T, CoreCallError> {
    serde_json::from_value(body).map_err(|error| request_invalid(&error.to_string()))
}

fn string(body: &Value, key: &str) -> Result<String, CoreCallError> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| request_invalid(&format!("{key} is required")))
}

fn request_invalid(message: &str) -> CoreCallError {
    CoreCallError::Api {
        code: "REQUEST_INVALID".to_string(),
        message: message.to_string(),
        retryable: false,
    }
}

/// An engine error as the IPC path decodes the same `ok:false` envelope.
fn api_error(error: ppvpn_core::Error) -> CoreCallError {
    let envelope = json!({ "ok": false, "error": error });
    match core_ipc::decode_envelope(envelope.to_string().as_bytes()) {
        Err(error) => error,
        Ok(_) => CoreCallError::Transport("engine error decoded as success".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("ppvpn-engine-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn config(dir: &Path) -> ClientConfig {
        let dir = dir.to_str().unwrap();
        ClientConfig {
            api_base: "https://api.example.test".into(),
            data_dir: dir.into(),
            log_dir: format!("{dir}/logs"),
            core_bin_dir: dir.into(),
            platform: "linux".into(),
            app_version: "0.0.0".into(),
        }
    }

    async fn launched(dir: &Path) -> Launched {
        match EngineLauncher::new(&config(dir)).launch().await {
            Ok(launched) => launched,
            Err(error) => panic!("launch: {error:?}"),
        }
    }

    async fn call(
        launched: &Launched,
        path: &'static str,
        body: Value,
    ) -> Result<Value, CoreCallError> {
        launched
            .transport
            .call(path, body, Duration::from_secs(5))
            .await
    }

    fn code(result: &Result<Value, CoreCallError>) -> Option<&str> {
        result.as_ref().err().and_then(CoreCallError::code)
    }

    #[test]
    fn the_rust_core_links() {
        let version = super::version();
        assert!(!version.core_version.is_empty());
        assert!(!version.sail_version.is_empty());
        assert!(!version.sail_commit.is_empty());
    }

    #[test]
    fn the_engine_profile_codes_count_as_profile_errors() {
        for code in ppvpn_core::codes::PROFILE_VALIDATION {
            assert!(core_ipc::is_profile_error(code), "{code}");
        }
        assert!(!core_ipc::is_profile_error("ENGINE_SHUT_DOWN"));
    }

    #[test]
    fn only_the_switch_selects_the_engine() {
        let dir = TempDir::new();
        let config = config(dir.path());
        assert!(launcher(&config, Some(OsStr::new("1"))).is_some());
        assert!(launcher(&config, None).is_none());
        assert!(launcher(&config, Some(OsStr::new("0"))).is_none());
        assert!(launcher(&config, Some(OsStr::new(""))).is_none());
    }

    #[test]
    fn engine_errors_decode_like_the_ipc_envelope() {
        let error = Engine::validate(&ApplyRequest::new(Vec::new())).unwrap_err();
        assert_eq!(
            api_error(error),
            CoreCallError::Api {
                code: "PROFILE_REQUIRED".to_string(),
                message: "profile is required".to_string(),
                retryable: false,
            }
        );
    }

    #[tokio::test]
    async fn the_launcher_creates_and_shuts_down_an_engine() {
        let dir = TempDir::new();
        let launched = launched(dir.path()).await;
        assert!(dir
            .path()
            .join(crate::storage::CORE_DIR)
            .join("engine")
            .is_dir());
        assert!(launched.accepts_routing_mode && launched.accepts_routed_proxy);
        assert_eq!(
            launched.rule_set_hosts,
            Some(vec!["api.example.test".to_string()])
        );
        let version =
            core_ipc::get_version_within(launched.transport.as_ref(), Duration::from_secs(5))
                .await
                .unwrap();
        assert_eq!(version.core_version, Engine::version().core_version);
        assert_eq!(version.core_api_version, 1);

        let transport = launched.transport.clone();
        launched.stop.send(()).unwrap();
        assert_eq!(launched.exited.await, "stopped");
        let after = core_ipc::start(transport.as_ref()).await.unwrap_err();
        assert_eq!(after.code(), Some("ENGINE_SHUT_DOWN"));
    }

    #[test]
    fn only_fatal_ends_the_engine() {
        use ppvpn_core::{DegradedReason, FatalReason};
        let fatal = fatal_reason(&EngineState::Fatal {
            reason: FatalReason::TunDeviceLost,
        })
        .unwrap();
        assert!(fatal.contains("tun_device_lost"), "{fatal}");
        let degraded = EngineState::Degraded {
            reasons: vec![DegradedReason::NoDefaultInterface],
        };
        assert_eq!(fatal_reason(&degraded), None);
        assert_eq!(fatal_reason(&EngineState::Running), None);
        assert_eq!(fatal_reason(&EngineState::Stopped), None);
    }

    #[tokio::test]
    async fn dropping_the_stop_sender_shuts_the_engine_down() {
        let dir = TempDir::new();
        let launched = launched(dir.path()).await;
        let transport = launched.transport.clone();
        drop(launched.stop);
        assert_eq!(launched.exited.await, "stopped");
        let after = core_ipc::stop(transport.as_ref(), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(after.code(), Some("ENGINE_SHUT_DOWN"));
    }

    #[tokio::test]
    async fn an_invalid_profile_fails_as_on_the_ipc_path() {
        let dir = TempDir::new();
        let launched = launched(dir.path()).await;
        let core = launched.transport.as_ref();

        // No profile at all.
        let error = core_ipc::apply_profile(core, &Value::Null, None, None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("PROFILE_REQUIRED"));
        assert_eq!(
            error.info(ErrorCode::StandardCoreFailed).code,
            ErrorCode::ProfileInvalid
        );

        // A profile the engine rejects: the same code apply would return,
        // through both apply-profile and validate-profile.
        let profile = json!({ "schema_version": 1, "nodes": [] });
        let expected = Engine::validate(&ApplyRequest::new(serde_json::to_vec(&profile).unwrap()))
            .unwrap_err();
        let applied = core_ipc::apply_profile(core, &profile, None, None)
            .await
            .unwrap_err();
        assert_eq!(applied.code(), Some(expected.code));
        let validated = call(
            &launched,
            "/v1/validate-profile",
            json!({ "profile": profile }),
        )
        .await;
        assert_eq!(code(&validated), Some(expected.code));

        // A routing mode the engine does not know.
        let mode = call(
            &launched,
            "/v1/apply-profile",
            json!({ "profile": profile, "routing_mode": "direct" }),
        )
        .await;
        assert_eq!(code(&mode), Some("ROUTING_MODE_INVALID"));
    }

    #[tokio::test]
    async fn each_path_reaches_its_engine_method() {
        let dir = TempDir::new();
        let launched = launched(dir.path()).await;
        let core = launched.transport.as_ref();

        let status = core_ipc::get_status(core).await.unwrap();
        assert_eq!(status.state, "stopped");
        assert_eq!(status.node_count, 0);
        assert!(status.system_proxy.is_some());
        assert_eq!(
            core_ipc::list_node_ids(core).await.unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(core_ipc::get_traffic(core).await.unwrap(), (0, 0));
        assert_eq!(
            call(&launched, "/v1/get-selected-node", json!({})).await,
            Ok(Value::Null)
        );
        assert_eq!(
            call(&launched, "/v1/get-connections", json!({})).await,
            Ok(json!([]))
        );
        core_ipc::stop(core, Duration::from_secs(5)).await.unwrap();

        // Lifecycle calls that need an applied profile.
        let start = core_ipc::start(core).await.unwrap_err();
        assert_eq!(start.code(), Some("PROFILE_NOT_APPLIED"));
        let select = core_ipc::select_node(core, "n1").await.unwrap_err();
        assert_eq!(select.code(), Some("PROFILE_NOT_APPLIED"));
        let pin = core_ipc::pin_ingress(core, "n1", Some("k1"))
            .await
            .unwrap_err();
        assert_eq!(pin.code(), Some("PROFILE_NOT_APPLIED"));
        let unpin = core_ipc::pin_ingress(core, "n1", None).await.unwrap_err();
        assert_eq!(unpin.code(), Some("PROFILE_NOT_APPLIED"));

        // Probes and local proxy calls before a profile: the routed user
        // and the system proxy listener exist from `new`, nodes do not.
        let not_applied = |result: Result<Value, CoreCallError>| {
            assert_eq!(code(&result), Some("PROFILE_NOT_APPLIED"), "{result:?}");
        };
        not_applied(
            core_ipc::probe_entrances(core, "tcp", &["n1".to_string()], 1_000, 4)
                .await
                .map(Value::from),
        );
        not_applied(core_ipc::probe_availability(core, "n1", "http://example.test", 1_000).await);
        assert!(core_ipc::local_proxies(core).await.unwrap().is_empty());
        let routed = core_ipc::routed_local_proxy(core).await.unwrap();
        assert!(
            routed.is_some_and(|p| p.node_id.is_empty() && p.port != 0),
            "no routed user before a profile"
        );
        let node = call(
            &launched,
            "/v1/get-local-proxy-credential",
            json!({ "node_id": "n1" }),
        )
        .await;
        assert_eq!(code(&node), Some("NODE_NOT_FOUND"));
        let system = core_ipc::set_system_proxy(core, true).await.unwrap();
        assert!(system.available && system.enabled && !system.listening);
        assert!(system.port.is_some_and(|port| port != 0));

        // Malformed bodies and unknown paths.
        let bad = call(&launched, "/v1/select-node", json!({})).await;
        assert_eq!(code(&bad), Some("REQUEST_INVALID"));
        let bad = call(&launched, "/v1/probe-entrances", json!({ "method": "udp" })).await;
        assert_eq!(code(&bad), Some("REQUEST_INVALID"));
        let unknown = call(&launched, "/v1/watch-events", json!({})).await;
        assert_eq!(code(&unknown), Some("CORE_API_UNSUPPORTED"));
    }
}
