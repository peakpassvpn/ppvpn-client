//! Standard mode's engine: the Rust `ppvpn-core` ([`ppvpn_core::Engine`],
//! docs/host-integration.md) in this process. [`EngineLauncher`] creates it
//! and [`EngineTransport`] serves the Core API v1 paths this crate calls from
//! it ([`ppvpn_engine_host::dispatch`]), so the typed helpers in `core_ipc`
//! work the same over the privileged service.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ppvpn_core::{
    Engine, EngineConfig, EventKind, LocalProxyConfig, LogConfig, LogLevel, LogSink, Platform, Role,
};
use ppvpn_engine_host::ApiError;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::core_ipc::{self, BoxFuture, CoreCallError, CoreTransport};
use crate::errors::{ClientErrorInfo, ErrorCode};
use crate::standard::{CoreLauncher, Launched};
use crate::ClientConfig;

/// Engine and sail versions, for diagnostics.
#[allow(dead_code)]
pub(crate) fn version() -> ppvpn_core::VersionInfo {
    Engine::version()
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
            state_dir: Path::new(&config.data_dir)
                .join(crate::storage::CORE_DIR)
                .join("engine"),
            log_dir: PathBuf::from(&config.log_dir),
            platform: platform(&config.platform),
            rule_set_hosts: core_ipc::rule_set_hosts(&config.api_base),
        }
    }

    fn engine_config(&self) -> EngineConfig {
        // The engine appends; the host rotates.
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
        // Only `new` resets the local proxy credentials; the reason stays in
        // the status for the instance's lifetime (section 4.6).
        let reset = engine
            .status()
            .local_proxy
            .and_then(|p| p.credentials_reset);
        if let Some(reason) = reset {
            tracing::warn!(
                ?reason,
                "standard engine: local proxy credentials were reset; apps holding the old ones must copy them again"
            );
        }
        let (stop, stop_rx) = oneshot::channel::<()>();
        let owned = engine.clone();
        let states = engine.subscribe(&[EventKind::StateChanged]);
        // A stop request, a dropped sender or a Fatal state shuts the engine
        // down. Fatal reads as an unexpected exit, so the standard instance's
        // supervisor recreates the engine and applies again.
        let exited = Box::pin(async move {
            let status = tokio::select! {
                _ = stop_rx => "stopped".to_string(),
                reason = ppvpn_engine_host::until_fatal(&owned, states) => format!("fatal: {reason}"),
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
            rule_set_hosts: self.rule_set_hosts.clone(),
        })
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
            tokio::time::timeout(
                timeout,
                ppvpn_engine_host::dispatch(&self.engine, path, body),
            )
            .await
            .map_err(|_| CoreCallError::Transport(format!("{path}: timed out")))?
            .map_err(api_error)
        })
    }
}

/// An engine error as every other Core API caller sees it.
fn api_error(error: ApiError) -> CoreCallError {
    CoreCallError::Api {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ppvpn_core::ApplyRequest;
    use serde_json::json;

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

    #[tokio::test]
    async fn the_launcher_creates_and_shuts_down_an_engine() {
        let dir = TempDir::new();
        let launched = launched(dir.path()).await;
        assert!(dir
            .path()
            .join(crate::storage::CORE_DIR)
            .join("engine")
            .is_dir());
        assert_eq!(
            launched.rule_set_hosts,
            vec!["api.example.test".to_string()]
        );
        let version = call(&launched, "/v1/get-version", json!({})).await.unwrap();
        assert_eq!(
            version["core_version"],
            json!(Engine::version().core_version)
        );
        assert_eq!(version["core_api_version"], json!(1));

        let transport = launched.transport.clone();
        launched.stop.send(()).unwrap();
        assert_eq!(launched.exited.await, "stopped");
        let after = core_ipc::start(transport.as_ref()).await.unwrap_err();
        assert_eq!(after.code(), Some("ENGINE_SHUT_DOWN"));
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
