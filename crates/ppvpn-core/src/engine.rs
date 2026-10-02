//! The instance handle (docs/host-integration.md, sections 3 and 4).
//!
//! This is the public API's skeleton: the signatures are the contract hosts
//! write against; the bodies come module by module. Unimplemented calls
//! return `CORE_OPERATION_FAILED` ("not implemented"); queries return an
//! empty snapshot.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::config::{EngineConfig, Role};
use crate::error::{codes, Error};
use crate::event::{EventItem, EventKind, EventReceiver, LogReceiver};
use crate::request::{validate_request, ApplyRequest, ApplyResult};
use crate::status::{EngineState, Status, SystemProxyStatus};
use crate::types::{
    AvailabilityResult, Connection, EntranceResult, LocalProxyCredential, LocalProxyMetadata,
    NodeInfo, ProbeAvailabilityRequest, ProbeEntrancesRequest, ShutdownReport, Traffic,
    VersionInfo,
};

/// The version of the local proxy contract (users, ports, metadata).
pub const LOCAL_PROXY_CONTRACT_VERSION: u32 = 1;

/// An instance (section 3): a reference-counted handle; clones are the same
/// instance. `Send + Sync`; every method may be called from any task.
/// Lifecycle calls are serialised inside; queries never wait on them.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    config: EngineConfig,
    state: Mutex<Lifecycle>,
}

#[derive(Default)]
struct Lifecycle {
    shut_down: bool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("role", &self.inner.config.role)
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Creates an instance (section 3): sweeps what a previous one left,
    /// locks `state_dir`, reads or creates the local proxy state. Applies
    /// nothing and starts nothing. Errors known at creation are returned
    /// here (`PERMISSION_DENIED`, `WINTUN_UNAVAILABLE`, `STATE_DIR_IN_USE`,
    /// `TUN_INSTANCE_EXISTS`), never as `Fatal` later.
    pub async fn new(config: EngineConfig) -> Result<Engine, Error> {
        Ok(Engine {
            inner: Arc::new(Inner {
                config,
                state: Mutex::new(Lifecycle::default()),
            }),
        })
    }

    /// Stops the whole instance (any handle may call it; idempotent): stops
    /// accepting, closes listeners and the TUN, removes its routing and DNS.
    /// At most 10 s; what could not be cleaned up in time is returned and
    /// logged. Afterwards lifecycle calls return `ENGINE_SHUT_DOWN`.
    pub async fn shutdown(&self) -> Result<ShutdownReport, Error> {
        self.inner.state.lock().expect("lifecycle lock").shut_down = true;
        Ok(ShutdownReport::default())
    }

    /// Validates a request as `apply` would, without an instance (4.1).
    pub fn validate(request: &ApplyRequest) -> Result<(), Error> {
        validate_request(request, Utc::now()).map(|_| ())
    }

    /// Applies a profile with the host's routing mode, selection and pins,
    /// atomically (4.1).
    pub async fn apply(&self, request: ApplyRequest) -> Result<ApplyResult, Error> {
        self.lifecycle()?;
        validate_request(&request, Utc::now())?;
        Err(Error::not_implemented("apply"))
    }

    /// Starts the applied profile; `PROFILE_NOT_APPLIED` without one (D1).
    pub async fn start(&self) -> Result<(), Error> {
        self.lifecycle()?;
        Err(Error::new(
            codes::PROFILE_NOT_APPLIED,
            false,
            "no profile has been applied",
        ))
    }

    /// Stops; the profile stays applied (`Configured`). Idempotent.
    pub async fn stop(&self) -> Result<(), Error> {
        self.lifecycle()?;
        Ok(())
    }

    /// Selects the node of new connections; the host persists it (4.3).
    pub async fn select_node(&self, node_id: &str) -> Result<(), Error> {
        self.lifecycle()?;
        let _ = node_id;
        Err(Error::new(
            codes::PROFILE_NOT_APPLIED,
            false,
            "no profile has been applied",
        ))
    }

    /// Pins a node to one ingress, or back to automatic with `None` (4.3).
    pub async fn pin_ingress(
        &self,
        node_id: &str,
        endpoint_key: Option<&str>,
    ) -> Result<(), Error> {
        self.lifecycle()?;
        let _ = (node_id, endpoint_key);
        Err(Error::new(
            codes::PROFILE_NOT_APPLIED,
            false,
            "no profile has been applied",
        ))
    }

    /// The authoritative snapshot (section 5).
    pub fn status(&self) -> Status {
        Status {
            state: EngineState::Stopped,
            system_proxy: SystemProxyStatus::default(),
            ..Status::default()
        }
    }

    pub fn nodes(&self) -> Vec<NodeInfo> {
        Vec::new()
    }

    pub fn selected_node(&self) -> Option<NodeInfo> {
        None
    }

    pub fn traffic(&self) -> Traffic {
        Traffic {
            upload_bytes: 0,
            download_bytes: 0,
            measured_at: now(),
        }
    }

    pub fn connections(&self) -> Vec<Connection> {
        Vec::new()
    }

    /// Versions; hosts show them in diagnostics.
    pub fn version() -> VersionInfo {
        VersionInfo {
            core_version: env!("CARGO_PKG_VERSION").into(),
            sail_version: String::new(),
            sail_commit: String::new(),
            profile_schema_version: crate::profile::CURRENT_SCHEMA_VERSION as u32,
            local_proxy_contract_version: LOCAL_PROXY_CONTRACT_VERSION,
        }
    }

    /// Entrance probes (4.5); `NO_DEFAULT_INTERFACE` (retryable) offline.
    pub async fn probe_entrances(
        &self,
        request: ProbeEntrancesRequest,
    ) -> Result<Vec<EntranceResult>, Error> {
        let _ = request;
        Err(Error::not_implemented("probe_entrances"))
    }

    /// An availability probe through the node's local proxy user (4.5).
    pub async fn probe_availability(
        &self,
        request: ProbeAvailabilityRequest,
    ) -> Result<AvailabilityResult, Error> {
        let _ = request;
        Err(Error::not_implemented("probe_availability"))
    }

    /// Local proxy endpoints without secrets (4.6).
    pub fn local_proxy_metadata(&self) -> Result<Vec<LocalProxyMetadata>, Error> {
        self.standard()?;
        Err(Error::not_implemented("local_proxy_metadata"))
    }

    /// A per-node credential (4.6).
    pub fn local_proxy_credential(&self, node_id: &str) -> Result<LocalProxyCredential, Error> {
        self.standard()?;
        let _ = node_id;
        Err(Error::not_implemented("local_proxy_credential"))
    }

    /// The routed user's credential; its username is the bare prefix (4.6).
    pub fn local_proxy_routed_credential(&self) -> Result<LocalProxyCredential, Error> {
        self.standard()?;
        Err(Error::not_implemented("local_proxy_routed_credential"))
    }

    /// Opens or closes the unauthenticated loopback listener for OS proxy
    /// settings (7891); the OS settings are the host's (4.6).
    pub async fn set_system_proxy_listener(
        &self,
        enabled: bool,
    ) -> Result<SystemProxyStatus, Error> {
        self.lifecycle()?;
        if self.inner.config.role != Role::Standard || !self.inner.config.system_proxy {
            return Err(Error::new(
                codes::SYSTEM_PROXY_UNAVAILABLE,
                false,
                "this instance cannot host the system proxy listener",
            ));
        }
        let _ = enabled;
        Err(Error::not_implemented("set_system_proxy_listener"))
    }

    /// Subscribes to `kinds` (section 6): one bounded buffer per kind, a
    /// `Lagged` item when this receiver fell behind on one.
    pub fn subscribe(&self, kinds: &[EventKind]) -> EventReceiver {
        let _ = kinds;
        let (_sender, receiver) = mpsc::channel::<EventItem>(1);
        EventReceiver { receiver }
    }

    /// The log lines, with `LogSink::Channel` (section 10).
    pub fn logs(&self) -> LogReceiver {
        let (_sender, receiver) = mpsc::channel(1);
        LogReceiver { receiver }
    }

    fn lifecycle(&self) -> Result<(), Error> {
        if self.inner.state.lock().expect("lifecycle lock").shut_down {
            return Err(Error::new(
                codes::ENGINE_SHUT_DOWN,
                false,
                "the instance was shut down",
            ));
        }
        Ok(())
    }

    fn standard(&self) -> Result<(), Error> {
        if self.inner.config.role != Role::Standard || self.inner.config.local_proxy.is_none() {
            return Err(Error::new(
                codes::LOCAL_PROXY_DISABLED,
                false,
                "this instance has no local proxy",
            ));
        }
        Ok(())
    }
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LocalProxyConfig, Platform};

    fn assert_send_sync_clone<T: Send + Sync + Clone + 'static>() {}

    #[test]
    fn engine_is_send_sync_clone() {
        assert_send_sync_clone::<Engine>();
    }

    #[tokio::test]
    async fn shut_down_instances_refuse_lifecycle_calls() {
        let engine = Engine::new(EngineConfig::new(
            Role::Standard,
            Platform::Linux,
            "/nonexistent",
        ))
        .await
        .unwrap();
        let other = engine.clone();
        engine.shutdown().await.unwrap();
        assert_eq!(
            other.start().await.unwrap_err().code,
            codes::ENGINE_SHUT_DOWN
        );
        assert_eq!(other.shutdown().await, Ok(ShutdownReport::default()));
    }

    #[tokio::test]
    async fn a_tun_instance_has_no_local_proxy() {
        let engine = Engine::new(EngineConfig::new(
            Role::Tun,
            Platform::Linux,
            "/nonexistent",
        ))
        .await
        .unwrap();
        assert_eq!(
            engine.local_proxy_metadata().unwrap_err().code,
            codes::LOCAL_PROXY_DISABLED
        );
        let standard = Engine::new(
            EngineConfig::new(Role::Standard, Platform::Linux, "/x")
                .with_local_proxy(LocalProxyConfig::new()),
        )
        .await
        .unwrap();
        assert_eq!(
            standard
                .set_system_proxy_listener(true)
                .await
                .unwrap_err()
                .code,
            codes::SYSTEM_PROXY_UNAVAILABLE
        );
    }

    #[test]
    fn states_serialise_tagged_snake_case() {
        use crate::status::DegradedReason;
        let state = EngineState::Degraded {
            reasons: vec![
                DegradedReason::IngressUnavailable {
                    node_id: "jp".into(),
                },
                DegradedReason::NoDefaultInterface,
            ],
        };
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            r#"{"state":"degraded","reasons":[{"kind":"ingress_unavailable","node_id":"jp"},{"kind":"no_default_interface"}]}"#
        );
        let status = serde_json::to_value(Status::default()).unwrap();
        assert_eq!(status["state"], "stopped");
    }

    /// The whole `Status` JSON is a contract: the CLI's `--json` passes it
    /// through. The state's tag and its fields sit at the top level, beside
    /// the other fields (flatten); empty optional fields are left out.
    #[test]
    fn status_json_shape_is_fixed() {
        use crate::request::RoutingMode;
        use crate::status::{DegradedReason, FatalReason, IngressHealth, NodeStatus, TunRouting};
        let status = Status {
            state: EngineState::Degraded {
                reasons: vec![
                    DegradedReason::PinnedLineDown {
                        node_id: "jp".into(),
                        endpoint_key: "jp-2".into(),
                    },
                    DegradedReason::NoDefaultInterface,
                ],
            },
            revision: Some("r7".into()),
            routing_mode: Some(RoutingMode::Global),
            selected_node_id: Some("jp".into()),
            node_count: 1,
            nodes: vec![NodeStatus {
                node_id: "jp".into(),
                pinned_endpoint_key: Some("jp-2".into()),
                ingresses: vec![IngressHealth {
                    endpoint_key: "jp-2".into(),
                    role: "backup".into(),
                    label: String::new(),
                    healthy: Some(false),
                    last_check_at: None,
                    consecutive_failures: 3,
                    active: true,
                }],
            }],
            draining_kernels: 1,
            tun_routing: Some(TunRouting::Ok),
            ..Status::default()
        };
        assert_eq!(
            serde_json::to_string(&status).unwrap(),
            concat!(
                r#"{"state":"degraded","reasons":[{"kind":"pinned_line_down","node_id":"jp","endpoint_key":"jp-2"},{"kind":"no_default_interface"}],"#,
                r#""revision":"r7","routing_mode":"global","selected_node_id":"jp","node_count":1,"#,
                r#""nodes":[{"node_id":"jp","pinned_endpoint_key":"jp-2","ingresses":[{"endpoint_key":"jp-2","role":"backup","healthy":false,"consecutive_failures":3,"active":true}]}],"#,
                r#""system_proxy":{"available":false,"enabled":false,"listening":false},"#,
                r#""draining_kernels":1,"tun_routing":"ok","dropped_log_lines":0}"#
            )
        );
        let fatal = Status {
            state: EngineState::Fatal {
                reason: FatalReason::TunRoutingBroken {
                    missing: vec!["9101/v4 nop".into()],
                },
            },
            ..Status::default()
        };
        assert_eq!(
            serde_json::to_string(&fatal).unwrap(),
            concat!(
                r#"{"state":"fatal","reason":{"kind":"tun_routing_broken","missing":["9101/v4 nop"]},"node_count":0,"#,
                r#""system_proxy":{"available":false,"enabled":false,"listening":false},"draining_kernels":0,"dropped_log_lines":0}"#
            )
        );
        // And it reads back.
        let back: Status = serde_json::from_str(&serde_json::to_string(&status).unwrap()).unwrap();
        assert_eq!(back, status);
    }
}
