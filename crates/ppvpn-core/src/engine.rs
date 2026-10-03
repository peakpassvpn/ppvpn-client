//! The instance handle (docs/host-integration.md, sections 3 and 4).
//!
//! The signatures are the contract hosts write against. Lifecycle calls
//! (apply, start, stop) run one at a time under `op`; what they leave is in
//! [`state::Live`], read by the queries under a short std lock, so a query
//! never waits on a lifecycle call. The runtime (sail, or the fake in tests)
//! sits behind [`Runtime`]. Calls not wired yet return
//! `CORE_OPERATION_FAILED` ("not implemented").

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use tokio::task::JoinHandle;

use crate::config::{EngineConfig, LogLevel, Role};
use crate::error::{codes, Error};
use crate::event::{Event, EventKind, EventReceiver, LogReceiver};
use crate::request::{validate_request, ApplyRequest, ApplyResult, RoutingMode};
use crate::runtime::sail::SailRuntime;
use crate::runtime::{Runtime, RuntimeError};
use crate::state_dir::StateDirLock;
use crate::status::{FatalReason, Status, SystemProxyStatus, TunRouting};
use crate::translate;
use crate::types::{
    AvailabilityResult, Connection, EntranceResult, LocalProxyCredential, LocalProxyMetadata,
    NodeInfo, ProbeAvailabilityRequest, ProbeEntrancesRequest, ShutdownReport, Traffic,
    VersionInfo,
};

mod bus;
mod cleanup;
mod lifecycle;
#[cfg(test)]
mod lifecycle_tests;
mod logs;
mod network;
mod probes;
#[cfg(test)]
mod probes_tests;
mod proxy;
#[cfg(test)]
mod proxy_tests;
mod routing;
mod selection;
#[cfg(test)]
mod selection_tests;
mod state;
mod switch;
mod tun;

use bus::Bus;
pub(crate) use bus::Subscription;
pub use logs::tracing_layer;
use logs::Logs;
use state::Live;
#[cfg(test)]
pub(crate) use state::TunRoutingSignal;

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
    runtime: Arc<dyn Runtime>,
    /// Serialises the lifecycle calls.
    op: tokio::sync::Mutex<()>,
    live: Mutex<Live>,
    bus: Bus,
    /// Follows the runtime: group switches, its state, health.
    watcher: Mutex<Option<JoinHandle<()>>>,
    /// Held from `new` to `shutdown` (or the last handle's drop).
    state_dir: Mutex<Option<StateDirLock>>,
    /// The local proxy's state and the system proxy listener (Standard).
    proxies: Option<Mutex<proxy::Proxies>>,
    /// The instance's log lines, to its sink (section 10).
    log: Logs,
    tun: tun::TunState,
    network: network::NetworkState,
    /// The Linux desktop TUN's routing guard while it runs.
    routing: routing::RoutingGuard,
}

impl Drop for Inner {
    /// The last handle went: what `shutdown` would do, bounded and off the
    /// caller's runtime; the state_dir lock goes after it.
    fn drop(&mut self) {
        if let Some(watcher) = self.watcher.get_mut().ok().and_then(Option::take) {
            watcher.abort();
        }
        // Before the TUN's cleanup.
        self.routing.stop();
        let running = self
            .live
            .get_mut()
            .map(|live| live.running && !live.shut_down)
            .unwrap_or(false);
        let shut_down = self
            .live
            .get_mut()
            .map(|live| live.shut_down)
            .unwrap_or(true);
        if !shut_down {
            // The lock goes with the teardown: released when it is done.
            let parts = cleanup::Parts {
                runtime: running.then(|| self.runtime.clone()),
                steps: Vec::new(),
                state_dir: self.state_dir.get_mut().ok().and_then(Option::take),
            };
            cleanup::cleanup_on_drop(parts);
        }
    }
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
        tun::check(&config)?;
        logs::install();
        let log = Logs::new(&config.log)?;
        let state_dir = StateDirLock::acquire(&config.state_dir)?;
        log.span().in_scope(|| cleanup::sweep(&config))?;
        let options = sail::embed::Options::new().run_dir(cleanup::run_dir(&config));
        let runtime = SailRuntime::new(options).map_err(|e| e.to_error())?;
        let engine = Engine::assemble(config, Arc::new(runtime), log)?;
        *engine.inner.state_dir.lock().expect("state dir lock") = Some(state_dir);
        engine.inner.start_local_dns().await?;
        Ok(engine)
    }

    /// An instance on `runtime` (tests: the fake), without the state_dir
    /// lock. Panics when the local proxy state cannot be opened.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn with_runtime(config: EngineConfig, runtime: Arc<dyn Runtime>) -> Engine {
        let log = Logs::new(&config.log).unwrap_or_else(|_| Logs::discard());
        Engine::assemble(config, runtime, log).expect("local proxy state")
    }

    fn assemble(
        config: EngineConfig,
        runtime: Arc<dyn Runtime>,
        log: Logs,
    ) -> Result<Engine, Error> {
        let proxies = proxy::Proxies::open(&config)?;
        log.attach(&runtime);
        let inner = Arc::new(Inner {
            tun: tun::TunState::new(&config),
            network: network::NetworkState::default(),
            routing: routing::RoutingGuard::default(),
            config,
            runtime,
            op: tokio::sync::Mutex::new(()),
            live: Mutex::default(),
            bus: Bus::default(),
            watcher: Mutex::new(None),
            state_dir: Mutex::new(None),
            proxies,
            log,
        });
        *inner.watcher.lock().expect("watcher") = lifecycle::spawn_watcher(&inner);
        Ok(Engine { inner })
    }

    /// Stops the whole instance (any handle may call it; idempotent): stops
    /// accepting, closes listeners and the TUN, removes its routing and DNS.
    /// At most 10 s; what could not be cleaned up in time is returned and
    /// logged. Afterwards lifecycle calls return `ENGINE_SHUT_DOWN`.
    pub async fn shutdown(&self) -> Result<ShutdownReport, Error> {
        let inner = &self.inner;
        if let Some(watcher) = inner.watcher.lock().expect("watcher").take() {
            watcher.abort();
        }
        // An apply or start in flight finishes first, within the limit.
        let deadline = tokio::time::Instant::now() + cleanup::SHUTDOWN_LIMIT;
        let op = tokio::time::timeout_at(deadline, inner.op.lock())
            .await
            .ok();
        let running = inner.live().running;
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let parts = cleanup::Parts {
            runtime: running.then(|| inner.runtime.clone()),
            steps: Vec::new(),
            // Released by the teardown when it is done: another instance
            // may take the directory once it is free.
            state_dir: inner.state_dir.lock().expect("state dir lock").take(),
        };
        // A sleeping re-probe or local proxy retry goes first; one under
        // way held `op` and is done.
        inner.network_stopped();
        inner.local_proxy_stopped();
        // Before the TUN closes: sail's cleanup must not be undone.
        inner.guard_stopped();
        let report = cleanup::cleanup(parts, left).await;
        // Shut down before the operation lock goes, so nothing queued on it
        // (a re-probe, a lifecycle call) acts on the torn-down runtime.
        {
            let mut live = inner.live();
            if !live.shut_down {
                live.shut_down = true;
                live.running = false;
                live.clear_runtime();
                inner.settle(&mut live);
            }
        }
        drop(op);
        inner.bus.close();
        Ok(report)
    }

    /// Validates a request as `apply` would, without an instance (4.1).
    pub fn validate(request: &ApplyRequest) -> Result<(), Error> {
        validate_request(request, Utc::now()).map(|_| ())
    }

    /// Applies a profile with the host's routing mode, selection and pins,
    /// atomically (4.1).
    pub async fn apply(&self, request: ApplyRequest) -> Result<ApplyResult, Error> {
        self.inner.apply(request).await
    }

    /// Starts the applied profile; `PROFILE_NOT_APPLIED` without one (D1).
    pub async fn start(&self) -> Result<(), Error> {
        self.inner.start().await
    }

    /// Stops; the profile stays applied (`Configured`). Idempotent.
    pub async fn stop(&self) -> Result<(), Error> {
        self.inner.stop().await
    }

    /// Selects the node of new connections; the host persists it (4.3).
    pub async fn select_node(&self, node_id: &str) -> Result<(), Error> {
        self.inner.select_node(node_id).await
    }

    /// Pins a node to one ingress, or back to automatic with `None` (4.3).
    pub async fn pin_ingress(
        &self,
        node_id: &str,
        endpoint_key: Option<&str>,
    ) -> Result<(), Error> {
        self.inner.pin_ingress(node_id, endpoint_key).await
    }

    /// The authoritative snapshot (section 5).
    pub fn status(&self) -> Status {
        let config = &self.inner.config;
        let live = self.inner.live();
        let (local_proxy, system_proxy) = (
            self.inner.local_proxy_status(live.running),
            self.inner.system_proxy_status(live.running),
        );
        let applied = live.applied.as_ref();
        Status {
            state: live.state.clone(),
            revision: applied.map(|a| a.profile.revision.clone()),
            routing_mode: applied.map(|a| a.mode),
            selected_node_id: applied.map(|a| a.selected.clone()),
            node_count: applied.map_or(0, |a| a.profile.nodes.len() as u32),
            selected_ingress: live.selected_ingress(),
            nodes: live.node_statuses(),
            system_proxy,
            local_proxy,
            tun_routing: (config.role == Role::Tun)
                .then_some(live.tun_routing.unwrap_or(TunRouting::Ok)),
            dropped_log_lines: self.inner.log.dropped(),
            ..Status::default()
        }
    }

    /// The applied profile's nodes (`list-nodes`); empty without one.
    pub fn nodes(&self) -> Vec<NodeInfo> {
        let live = self.inner.live();
        live.applied
            .as_ref()
            .map(|a| a.profile.nodes.iter().map(selection::node_info).collect())
            .unwrap_or_default()
    }

    /// The selected node (`get-selected-node`); None without a profile.
    pub fn selected_node(&self) -> Option<NodeInfo> {
        let live = self.inner.live();
        let applied = live.applied.as_ref()?;
        applied.node(&applied.selected).map(selection::node_info)
    }

    /// Cumulative bytes as last read from the runtime (every second while
    /// running); `measured_at` is when.
    pub fn traffic(&self) -> Traffic {
        let live = self.inner.live();
        Traffic {
            upload_bytes: live.traffic.upload_bytes,
            download_bytes: live.traffic.download_bytes,
            measured_at: live.traffic_at.unwrap_or_else(now),
        }
    }

    /// The open connections as last read from the runtime (every second
    /// while running), each with the node its outbound chain goes through
    /// (empty: direct).
    pub fn connections(&self) -> Vec<Connection> {
        let live = self.inner.live();
        let nodes = live.applied.as_ref().map(|a| &a.translation.outbound_nodes);
        live.connections
            .iter()
            .map(|c| Connection {
                id: c.id.to_string(),
                node_id: nodes
                    .and_then(|nodes| c.chain.iter().find_map(|tag| nodes.get(tag)))
                    .cloned()
                    .unwrap_or_default(),
                network: c.network.clone(),
                destination: c.destination.clone(),
                upload_bytes: c.upload_bytes,
                download_bytes: c.download_bytes,
                started_at: DateTime::<Utc>::from(c.started),
            })
            .collect()
    }

    /// Versions; hosts show them in diagnostics.
    pub fn version() -> VersionInfo {
        VersionInfo {
            core_version: env!("CARGO_PKG_VERSION").into(),
            sail_version: sail::embed::BUILD.version.into(),
            sail_commit: sail::embed::BUILD.commit.into(),
            profile_schema_version: crate::profile::CURRENT_SCHEMA_VERSION as u32,
            local_proxy_contract_version: LOCAL_PROXY_CONTRACT_VERSION,
        }
    }

    /// Entrance probes (4.5), directly, of the applied profile's ingresses;
    /// `NO_DEFAULT_INTERFACE` (retryable) offline.
    pub async fn probe_entrances(
        &self,
        request: ProbeEntrancesRequest,
    ) -> Result<Vec<EntranceResult>, Error> {
        self.inner.admit()?;
        self.inner
            .probe_entrances(request, &crate::probe::SystemNet)
            .await
    }

    /// An availability probe through the node's outbound (4.5):
    /// `LOCAL_PROXY_DISABLED` on an instance without a local proxy (a TUN
    /// instance too, as Go), then `PROFILE_NOT_APPLIED`, `CORE_NOT_RUNNING`.
    pub async fn probe_availability(
        &self,
        request: ProbeAvailabilityRequest,
    ) -> Result<AvailabilityResult, Error> {
        self.inner.admit()?;
        self.standard()?;
        self.inner.probe_availability(request).await
    }

    /// Local proxy endpoints without secrets (4.6): one per node of the
    /// applied profile, then the routed user.
    pub fn local_proxy_metadata(&self) -> Result<Vec<LocalProxyMetadata>, Error> {
        self.standard()?;
        self.inner.local_proxy_metadata()
    }

    /// A per-node credential (4.6); `NODE_NOT_FOUND` for a node the applied
    /// profile does not have (any before the first apply).
    pub fn local_proxy_credential(&self, node_id: &str) -> Result<LocalProxyCredential, Error> {
        self.standard()?;
        self.inner.local_proxy_credential(node_id)
    }

    /// The routed user's credential; its username is the bare prefix (4.6).
    /// Readable right after `new`.
    pub fn local_proxy_routed_credential(&self) -> Result<LocalProxyCredential, Error> {
        self.standard()?;
        self.inner.local_proxy_routed_credential()
    }

    /// Opens or closes the unauthenticated loopback listener for OS proxy
    /// settings (7891); the OS settings are the host's (4.6).
    pub async fn set_system_proxy_listener(
        &self,
        enabled: bool,
    ) -> Result<SystemProxyStatus, Error> {
        self.inner.admit()?;
        if self.inner.config.role != Role::Standard || !self.inner.config.system_proxy {
            return Err(proxy::system_proxy_unavailable());
        }
        self.inner.set_system_proxy_listener(enabled).await
    }

    /// Subscribes to `kinds` (section 6): one bounded buffer per kind, a
    /// `Lagged` item when this receiver fell behind on one.
    pub fn subscribe(&self, kinds: &[EventKind]) -> EventReceiver {
        EventReceiver {
            subscription: self.inner.bus.subscribe(kinds),
        }
    }

    /// The log lines, with `LogSink::Channel` (section 10): bounded, taken
    /// once; a second call, or another sink, gets a closed receiver.
    pub fn logs(&self) -> LogReceiver {
        self.inner.log.receiver()
    }

    /// The default interface changed (`None`: offline). Its source is
    /// sail's network events (E1b), not wired yet: no monitor of our own.
    #[allow(dead_code)]
    pub(crate) fn on_network(&self, interface: Option<(&str, u32)>) {
        self.inner.on_network(interface);
    }

    /// The TUN routing guard's report, as the guard sends it (tests).
    #[cfg(test)]
    pub(crate) fn on_tun_routing(&self, signal: TunRoutingSignal) {
        self.inner.on_tun_routing(signal);
    }

    fn standard(&self) -> Result<(), Error> {
        if self.inner.config.role != Role::Standard || self.inner.config.local_proxy.is_none() {
            return Err(proxy::local_proxy_disabled());
        }
        Ok(())
    }
}

impl Inner {
    fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().expect("live")
    }

    fn publish(&self, event: Event) {
        self.bus.publish(event);
    }

    /// Reports the state `live` now makes, if it changed (StateChanged).
    fn settle(&self, live: &mut Live) {
        let state = live.state_now();
        if state != live.state {
            let previous = std::mem::replace(&mut live.state, state.clone());
            self.publish(Event::StateChanged {
                at: now(),
                state,
                previous,
            });
        }
    }

    /// Whether a lifecycle call may run.
    fn admit(&self) -> Result<(), Error> {
        let live = self.live();
        if live.shut_down {
            return Err(Error::new(
                codes::ENGINE_SHUT_DOWN,
                false,
                "the instance was shut down",
            ));
        }
        if live.fatal.is_some() {
            return Err(Error::new(
                codes::ENGINE_FATAL,
                false,
                "the instance is fatal: drop it and create another",
            ));
        }
        Ok(())
    }

    /// The Engine's error for a runtime's; a panic in sail makes the
    /// instance Fatal.
    fn runtime_error(&self, error: &RuntimeError) -> Error {
        if error.code == "panicked" {
            let mut live = self.live();
            live.running = false;
            live.run += 1;
            live.clear_runtime();
            live.fatal = Some(FatalReason::Panic);
            self.settle(&mut live);
            drop(live);
            self.guard_stopped();
        }
        error.to_error()
    }

    /// The translation inputs of this instance. The local proxy, the system
    /// proxy listener, rule sets and the TUN are wired by their own modules;
    /// until then they are left out.
    fn options(
        &self,
        mode: RoutingMode,
        selected: &str,
        pins: &BTreeMap<String, String>,
    ) -> translate::Options {
        translate::Options {
            mode,
            selected_node_id: Some(selected.to_owned()),
            pins: pins.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            local_proxy: self.local_proxy_options(),
            system_proxy_port: self.system_proxy_options(),
            log_level: match self.config.log.level {
                LogLevel::Info => "info",
                LogLevel::Debug => "debug",
            }
            .into(),
            tun: self.tun_options(),
            ..translate::Options::default()
        }
    }
}

fn not_applied() -> Error {
    Error::new(
        codes::PROFILE_NOT_APPLIED,
        false,
        "no profile has been applied",
    )
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LocalProxyConfig, Platform};
    use crate::status::EngineState;

    fn assert_send_sync_clone<T: Send + Sync + Clone + 'static>() {}

    /// A fresh state directory for one test.
    fn state_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ppvpn-core-engine-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn engine_is_send_sync_clone() {
        assert_send_sync_clone::<Engine>();
    }

    #[tokio::test]
    async fn shut_down_instances_refuse_lifecycle_calls() {
        let engine = Engine::new(EngineConfig::new(
            Role::Standard,
            Platform::Linux,
            state_dir("shutdown"),
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
            state_dir("tun"),
        ))
        .await
        .unwrap();
        assert_eq!(
            engine.local_proxy_metadata().unwrap_err().code,
            codes::LOCAL_PROXY_DISABLED
        );
        let standard = Engine::new(
            EngineConfig::new(Role::Standard, Platform::Linux, state_dir("standard"))
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

    #[tokio::test]
    async fn state_dir_is_one_instances_until_shutdown() {
        let dir = state_dir("lock");
        let config = || EngineConfig::new(Role::Standard, Platform::Linux, dir.clone());
        let first = Engine::new(config()).await.unwrap();
        let err = Engine::new(config()).await.unwrap_err();
        assert_eq!((err.code, err.retryable), (codes::STATE_DIR_IN_USE, false));
        first.shutdown().await.unwrap();
        let second = Engine::new(config()).await.expect("free after shutdown");
        drop(second);
        Engine::new(config())
            .await
            .expect("free after the last handle's drop");
    }

    #[test]
    fn version_names_the_sail_it_links() {
        let version = Engine::version();
        assert!(!version.sail_version.is_empty());
        assert_eq!(version.sail_version, sail::embed::BUILD.version);
        // sail by git rev: its build.rs takes the commit Cargo checked out.
        assert!(!version.sail_commit.is_empty());
        assert_ne!(version.sail_commit, "unknown");
        assert!(
            version.sail_commit.len() >= 7
                && version.sail_commit.chars().all(|c| c.is_ascii_hexdigit()),
            "{}",
            version.sail_commit
        );
        assert_eq!(
            version.sail_version.split('.').count(),
            3,
            "{}",
            version.sail_version
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
