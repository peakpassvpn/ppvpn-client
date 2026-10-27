//! Tunnel mode (#448): the phones' connection. The engine runs in the system
//! VPN's own process (Android's `VpnService`, iOS's packet tunnel), which the
//! app starts and stops through its platform plugin; this controller drives
//! that engine over the tunnel socket ([`crate::tunnel`]): the profile with
//! the routing mode, selection and pins, start, stop, and the host's events.
//!
//! The order on the phone: the app starts the system VPN, then calls
//! `connect`, which waits for the host's socket; `disconnect` stops the
//! engine and the app then stops the system VPN. The app mirrors the phase:
//! when it turns `Off` while the system VPN runs, the app stops it.
//!
//! Health checks run in the tunnel host, not here: the app's process can be
//! suspended in the background at any time. The state is reported as an
//! [`EnhancedState`], so the connection folds it like enhanced mode's.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::core_ipc::{self, ApplyChoices, CoreCallError, CoreStatus};
use crate::errors::{ClientError, ClientErrorInfo, ErrorCode};
use crate::tunnel::TunnelTransport;
use crate::{ConnectionPhase, EnhancedState};

/// How long `connect` waits for the host's socket after the app started the
/// system VPN.
const READY_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(15)
};
const READY_POLL: Duration = if cfg!(test) {
    Duration::from_millis(20)
} else {
    Duration::from_millis(200)
};
/// `/v1/stop` on disconnect; the app stops the system VPN after it anyway.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// The launch-time look for a tunnel that is already up.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) type StateSink = Arc<dyn Fn(EnhancedState) + Send + Sync>;

/// What every apply carries, shared with the client.
pub(crate) struct TunnelConfig {
    pub rule_set_hosts: Vec<String>,
    pub routing: crate::routing::RoutingModeCell,
    pub ingress_pins: crate::ingress::PinsCell,
    pub selection: crate::session::SelectionSource,
}

struct Profile {
    value: Value,
    revision: String,
}

#[derive(Default)]
struct Data {
    state: EnhancedState,
    /// Bumped by every connect, disconnect and restore; a watch of an older
    /// generation changes nothing.
    generation: u64,
    profile: Option<Arc<Profile>>,
    /// The revision the host's engine runs.
    applied: Option<String>,
    watch: Option<JoinHandle<()>>,
}

struct Inner {
    transport: TunnelTransport,
    config: TunnelConfig,
    sink: StateSink,
    /// Serialises connect, disconnect and applies.
    operation: tokio::sync::Mutex<()>,
    data: Mutex<Data>,
}

#[derive(Clone)]
pub(crate) struct TunnelMode {
    inner: Arc<Inner>,
}

impl TunnelMode {
    /// The host's socket in the app's directory `dir`.
    pub(crate) fn new(dir: &Path, config: TunnelConfig, sink: StateSink) -> Self {
        Self {
            inner: Arc::new(Inner {
                transport: TunnelTransport::in_dir(dir),
                config,
                sink,
                operation: tokio::sync::Mutex::new(()),
                data: Mutex::new(Data::default()),
            }),
        }
    }

    /// The client reads the state through its sink.
    #[cfg(all(test, unix))]
    fn state(&self) -> EnhancedState {
        self.inner.with_data(|data| data.state.clone())
    }

    /// Keeps `profile` for the next connect and applies it now while on.
    pub(crate) async fn set_profile(
        &self,
        profile: &[u8],
        revision: &str,
    ) -> Result<(), ClientError> {
        let value: Value = serde_json::from_slice(profile).map_err(|error| {
            ClientError::failed(
                ErrorCode::ProfileInvalid,
                format!("PROFILE_JSON_INVALID: {error}"),
            )
        })?;
        if revision.is_empty() {
            return Err(ClientError::failed(
                ErrorCode::ProfileInvalid,
                "PROFILE_REVISION_REQUIRED",
            ));
        }
        let profile = Arc::new(Profile {
            value,
            revision: revision.to_string(),
        });
        let _operation = self.inner.operation.lock().await;
        let stale = self.inner.with_data(|data| {
            data.profile = Some(profile);
            data.state.phase == ConnectionPhase::On && data.applied.as_deref() != Some(revision)
        });
        if stale {
            self.inner.apply().await?;
        }
        Ok(())
    }

    /// Applies the profile again (the routing mode changed) while on.
    pub(crate) async fn reapply(&self) -> Result<(), ClientError> {
        let _operation = self.inner.operation.lock().await;
        if self.inner.is_on() {
            self.inner.apply().await?;
        }
        Ok(())
    }

    /// Starts the engine in the system VPN the app just started.
    pub(crate) async fn enable(&self) -> Result<(), ClientError> {
        let _operation = self.inner.operation.lock().await;
        if self.inner.is_on() {
            return Ok(());
        }
        self.inner.connect().await
    }

    /// Stops the engine; always ends in `Off`. A host that is already gone
    /// is not an error.
    pub(crate) async fn disable(&self) -> Result<(), ClientError> {
        let _operation = self.inner.operation.lock().await;
        let begun = self.inner.with_data(|data| {
            data.generation += 1;
            if let Some(watch) = data.watch.take() {
                watch.abort();
            }
            data.applied = None;
            (data.state.phase != ConnectionPhase::Off).then(|| {
                data.state = EnhancedState {
                    phase: ConnectionPhase::Disconnecting,
                    ..EnhancedState::default()
                };
                data.generation
            })
        });
        let Some(generation) = begun else {
            return Ok(());
        };
        self.inner.emit();
        match core_ipc::stop(&self.inner.transport, STOP_TIMEOUT).await {
            Ok(()) => tracing::info!("tunnel mode: engine stopped"),
            Err(error) => tracing::info!("tunnel mode: stop not confirmed ({})", error.detail()),
        }
        self.inner
            .transition(generation, ConnectionPhase::Off, None);
        Ok(())
    }

    /// At launch: a tunnel that is already up (the app was closed while
    /// connected) is attached to, without applying anything.
    pub(crate) async fn restore(&self) {
        let _operation = self.inner.operation.lock().await;
        let status =
            tokio::time::timeout(RESTORE_TIMEOUT, core_ipc::get_status(&self.inner.transport))
                .await;
        let Ok(Ok(status)) = status else {
            return;
        };
        if !status.is_serving() {
            return;
        }
        let generation = self.inner.with_data(|data| {
            data.generation += 1;
            data.applied = status.revision.clone();
            data.state = EnhancedState {
                phase: ConnectionPhase::On,
                ..EnhancedState::default()
            };
            data.generation
        });
        tracing::info!("tunnel mode: attached to the running tunnel");
        self.inner.emit();
        self.inner.spawn_watch(generation);
    }

    /// Selects `node_id` in the running engine; a no-op while not on.
    pub(crate) async fn select_node(&self, node_id: &str) -> Result<(), ClientError> {
        if !self.inner.is_on() {
            return Ok(());
        }
        core_ipc::select_node(&self.inner.transport, node_id)
            .await
            .map_err(|error| error.into_client_error(ErrorCode::ConnectFailed))
    }

    /// The running engine's status; `None` while not on.
    pub(crate) async fn core_status(&self) -> Option<CoreStatus> {
        if !self.inner.is_on() {
            return None;
        }
        core_ipc::get_status(&self.inner.transport).await.ok()
    }

    /// The running engine's traffic counters; `None` while not on.
    pub(crate) async fn traffic_totals(&self) -> Option<(u64, u64)> {
        if !self.inner.is_on() {
            return None;
        }
        core_ipc::get_traffic(&self.inner.transport)
            .await
            .ok()
            .map(|traffic| traffic.totals())
    }

    /// App exit: the tunnel outlives the app, so only the watch ends.
    pub(crate) fn shutdown(&self) {
        self.inner.with_data(|data| {
            if let Some(watch) = data.watch.take() {
                watch.abort();
            }
        });
    }
}

impl Inner {
    fn with_data<T>(&self, change: impl FnOnce(&mut Data) -> T) -> T {
        change(
            &mut self
                .data
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn is_on(&self) -> bool {
        self.with_data(|data| data.state.phase == ConnectionPhase::On)
    }

    fn emit(&self) {
        (self.sink)(self.with_data(|data| data.state.clone()));
    }

    /// Moves to `phase` unless a newer generation began; an `Error` is
    /// retryable.
    fn transition(
        &self,
        generation: u64,
        phase: ConnectionPhase,
        reason: Option<ClientErrorInfo>,
    ) -> bool {
        let applied = self.with_data(|data| {
            if data.generation != generation {
                return false;
            }
            data.state = EnhancedState {
                phase,
                retryable: phase == ConnectionPhase::Error,
                reason,
                ..EnhancedState::default()
            };
            true
        });
        if applied {
            self.emit();
        }
        applied
    }

    fn choices(&self) -> ApplyChoices {
        ApplyChoices::new(self.config.selection.get(), &self.config.ingress_pins.get())
    }

    async fn apply_profile(&self, profile: &Profile) -> Result<(), CoreCallError> {
        core_ipc::apply_profile(
            &self.transport,
            &profile.value,
            Some(&self.config.rule_set_hosts),
            Some(self.config.routing.get()),
            &self.choices(),
        )
        .await?;
        let revision = profile.revision.clone();
        self.with_data(|data| data.applied = Some(revision));
        Ok(())
    }

    /// Applies the current profile to the running engine; caller holds
    /// `operation`.
    async fn apply(&self) -> Result<(), ClientError> {
        let Some(profile) = self.with_data(|data| data.profile.clone()) else {
            return Ok(());
        };
        self.apply_profile(&profile)
            .await
            .map_err(|error| error.into_client_error(ErrorCode::ConnectFailed))
    }

    /// The host's status once it answers: the system VPN was just started
    /// and the host may not listen yet, or not have its engine yet
    /// (`TUNNEL_NOT_READY`).
    async fn wait_ready(&self) -> Result<CoreStatus, CoreCallError> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let error = match core_ipc::get_status(&self.transport).await {
                Ok(status) => return Ok(status),
                Err(error) => error,
            };
            let waiting = match &error {
                CoreCallError::Transport(_) => true,
                CoreCallError::Api { retryable, .. } => *retryable,
            };
            if !waiting || Instant::now() + READY_POLL > deadline {
                return Err(CoreCallError::Transport(format!(
                    "TUNNEL_NOT_READY: {}",
                    error.detail()
                )));
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }

    /// Caller holds `operation`.
    async fn connect(self: &Arc<Self>) -> Result<(), ClientError> {
        let Some(profile) = self.with_data(|data| data.profile.clone()) else {
            return Err(ClientError::failed(ErrorCode::Internal, "PROFILE_REQUIRED"));
        };
        let generation = self.with_data(|data| {
            data.generation += 1;
            if let Some(watch) = data.watch.take() {
                watch.abort();
            }
            data.applied = None;
            data.state = EnhancedState {
                phase: ConnectionPhase::Connecting,
                ..EnhancedState::default()
            };
            data.generation
        });
        self.emit();
        let result = async {
            let before = self.wait_ready().await?;
            self.apply_profile(&profile).await?;
            if !before.is_serving() {
                core_ipc::start(&self.transport).await?;
            }
            let status = core_ipc::get_status(&self.transport).await?;
            if status.is_serving() {
                Ok(())
            } else {
                Err(CoreCallError::Transport(format!(
                    "TUNNEL_ENGINE_NOT_RUNNING: {}",
                    status.state
                )))
            }
        }
        .await;
        match result {
            Ok(()) => {
                tracing::info!(revision = %profile.revision, "tunnel mode: on");
                if self.transition(generation, ConnectionPhase::On, None) {
                    self.spawn_watch(generation);
                }
                Ok(())
            }
            Err(error) => {
                let info = error.info(ErrorCode::ConnectFailed);
                tracing::warn!("tunnel mode: connect failed ({})", info.detail);
                if let Err(error) = core_ipc::stop(&self.transport, STOP_TIMEOUT).await {
                    tracing::info!("tunnel mode: stop after the failure ({})", error.detail());
                }
                self.transition(generation, ConnectionPhase::Error, Some(info.clone()));
                Err(info.into())
            }
        }
    }

    /// Follows the host's events for `generation` until the host closes the
    /// watch (it stopped).
    fn spawn_watch(self: &Arc<Self>, generation: u64) {
        let inner = self.clone();
        let task = tokio::spawn(async move {
            let ended = match inner.transport.watch().await {
                Ok(mut watch) => loop {
                    match watch.next().await {
                        Ok(Some(status)) => inner.on_status(generation, &status),
                        Ok(None) => break "TUNNEL_STOPPED".to_string(),
                        Err(error) => break format!("TUNNEL_STOPPED: {}", error.detail()),
                    }
                },
                Err(error) => format!("TUNNEL_STOPPED: {}", error.detail()),
            };
            inner.on_stopped(generation, ended);
        });
        self.with_data(|data| {
            if data.generation == generation {
                data.watch = Some(task);
            } else {
                task.abort();
            }
        });
    }

    /// The engine's state from the host: serving is on, `fatal` an error
    /// the user can retry, anything else (stopped by the host) too.
    fn on_status(&self, generation: u64, status: &CoreStatus) {
        let phase =
            self.with_data(|data| (data.generation == generation).then_some(data.state.phase));
        if !matches!(phase, Some(ConnectionPhase::On | ConnectionPhase::Error)) {
            return;
        }
        if status.is_serving() {
            if phase != Some(ConnectionPhase::On) {
                self.transition(generation, ConnectionPhase::On, None);
            }
            return;
        }
        let info = ClientErrorInfo::new(
            ErrorCode::ConnectFailed,
            format!("TUNNEL_ENGINE_{}", status.state.to_uppercase()),
        );
        if phase == Some(ConnectionPhase::On) {
            tracing::warn!("tunnel mode: the engine is {}", status.state);
            self.transition(generation, ConnectionPhase::Error, Some(info));
        }
    }

    /// The host closed the watch: the system VPN stopped (another VPN took
    /// over, the user turned it off in the system settings, the system ended
    /// the process).
    fn on_stopped(&self, generation: u64, detail: String) {
        let current = self.with_data(|data| {
            let current = data.generation == generation && data.state.phase != ConnectionPhase::Off;
            if current {
                data.watch = None;
                data.applied = None;
            }
            current
        });
        if current {
            tracing::warn!("tunnel mode: the tunnel stopped ({detail})");
            self.transition(
                generation,
                ConnectionPhase::Error,
                Some(ClientErrorInfo::new(ErrorCode::ConnectFailed, detail)),
            );
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::tunnel::{read_frame, write_frame, SOCKET_FILE};
    use serde_json::json;
    use tokio::net::UnixListener;
    use tokio::sync::watch;

    const PROFILE: &[u8] = br#"{"revision":"r1","nodes":[]}"#;

    /// A tunnel host with one engine: `state` is its state, watches get a
    /// status frame for each change and end when `closed` turns true.
    struct Host {
        state: Arc<watch::Sender<String>>,
        closed: Arc<watch::Sender<bool>>,
        requests: Arc<Mutex<Vec<Value>>>,
    }

    impl Host {
        fn paths(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|request| request["path"].as_str().unwrap_or_default().to_string())
                .collect()
        }
    }

    fn status(state: &str) -> Value {
        json!({ "state": state, "revision": "r1" })
    }

    fn host(dir: &Path, initial: &str) -> Host {
        let listener = UnixListener::bind(dir.join(SOCKET_FILE)).unwrap();
        let state = Arc::new(watch::channel(initial.to_string()).0);
        let closed = Arc::new(watch::channel(false).0);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (state_tx, closed_tx, seen) = (state.clone(), closed.clone(), requests.clone());
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let (state, closed, seen) = (state_tx.clone(), closed_tx.clone(), seen.clone());
                tokio::spawn(async move {
                    let Ok(Some(frame)) = read_frame(&mut stream).await else {
                        return;
                    };
                    let request: Value = serde_json::from_slice(&frame).unwrap();
                    seen.lock().unwrap().push(request.clone());
                    let ok = |data: Value| json!({ "ok": true, "data": data }).to_string();
                    let answer = match request["path"].as_str().unwrap() {
                        "/v1/watch" => {
                            let mut states = state.subscribe();
                            let mut closing = closed.subscribe();
                            loop {
                                let current = states.borrow_and_update().clone();
                                let frame =
                                    ok(json!({ "kind": "status", "status": status(&current) }));
                                if write_frame(&mut stream, frame.as_bytes()).await.is_err() {
                                    return;
                                }
                                tokio::select! {
                                    changed = states.changed() => if changed.is_err() { return },
                                    _ = closing.wait_for(|closed| *closed) => return,
                                }
                            }
                        }
                        "/v1/get-status" => ok(status(&state.borrow())),
                        "/v1/start" => {
                            state.send_replace("running".into());
                            ok(json!({}))
                        }
                        "/v1/stop" => {
                            state.send_replace("stopped".into());
                            ok(json!({}))
                        }
                        "/v1/apply-profile" => ok(json!({ "applied": true })),
                        "/v1/get-traffic" => ok(json!({ "upload_bytes": 5, "download_bytes": 7 })),
                        _ => ok(json!({})),
                    };
                    let _ = write_frame(&mut stream, answer.as_bytes()).await;
                });
            }
        });
        Host {
            state,
            closed,
            requests,
        }
    }

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ppvpn-tm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mode(dir: &Path) -> (TunnelMode, Arc<Mutex<Vec<ConnectionPhase>>>) {
        let phases = Arc::new(Mutex::new(Vec::new()));
        let seen = phases.clone();
        let mode = TunnelMode::new(
            dir,
            TunnelConfig {
                rule_set_hosts: vec!["www.example.com".into()],
                routing: Default::default(),
                ingress_pins: Default::default(),
                selection: crate::session::SelectionSource::new(|| Some("hk".into())),
            },
            Arc::new(move |state: EnhancedState| seen.lock().unwrap().push(state.phase)),
        );
        (mode, phases)
    }

    async fn eventually(what: &str, check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn connect_applies_with_the_selection_starts_and_is_on() {
        let dir = temp_dir();
        let host = host(&dir, "configured");
        let (mode, phases) = mode(&dir);
        mode.set_profile(PROFILE, "r1").await.unwrap();
        mode.enable().await.unwrap();
        assert_eq!(mode.state().phase, ConnectionPhase::On);
        assert_eq!(
            phases.lock().unwrap()[..2],
            [ConnectionPhase::Connecting, ConnectionPhase::On]
        );
        let paths = host.paths();
        assert_eq!(
            paths[..4],
            [
                "/v1/get-status",
                "/v1/apply-profile",
                "/v1/start",
                "/v1/get-status"
            ]
        );
        let apply = host.requests.lock().unwrap()[1].clone();
        assert_eq!(apply["body"]["selected_node_id"], "hk");
        assert_eq!(
            apply["body"]["allowed_rule_set_hosts"],
            json!(["www.example.com"])
        );
        assert_eq!(mode.traffic_totals().await, Some((5, 7)));
    }

    #[tokio::test]
    async fn connect_waits_for_the_host_to_listen() {
        let dir = temp_dir();
        let (mode, _) = mode(&dir);
        mode.set_profile(PROFILE, "r1").await.unwrap();
        let late = dir.clone();
        let started = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            host(&late, "configured")
        });
        mode.enable().await.unwrap();
        assert_eq!(mode.state().phase, ConnectionPhase::On);
        drop(started.await.unwrap());
    }

    #[tokio::test]
    async fn no_host_is_a_retryable_error() {
        let dir = temp_dir();
        let (mode, _) = mode(&dir);
        mode.set_profile(PROFILE, "r1").await.unwrap();
        let error = mode.enable().await.unwrap_err();
        let state = mode.state();
        assert_eq!(state.phase, ConnectionPhase::Error);
        assert!(state.retryable);
        let reason = state.reason.unwrap();
        assert_eq!(reason.code, ErrorCode::ConnectFailed);
        assert!(
            reason.detail.starts_with("TUNNEL_NOT_READY"),
            "{}",
            reason.detail
        );
        assert!(error.info().is_some());
    }

    #[tokio::test]
    async fn the_host_stopping_is_an_error() {
        let dir = temp_dir();
        let host = host(&dir, "configured");
        let (mode, _) = mode(&dir);
        mode.set_profile(PROFILE, "r1").await.unwrap();
        mode.enable().await.unwrap();
        eventually("the watch", || {
            host.paths().contains(&"/v1/watch".to_string())
        })
        .await;
        host.closed.send_replace(true);
        eventually("the error", || mode.state().phase == ConnectionPhase::Error).await;
        let reason = mode.state().reason.unwrap();
        assert_eq!(reason.detail, "TUNNEL_STOPPED");
    }

    #[tokio::test]
    async fn a_fatal_engine_is_an_error_and_running_again_is_on() {
        let dir = temp_dir();
        let host = host(&dir, "configured");
        let (mode, _) = mode(&dir);
        mode.set_profile(PROFILE, "r1").await.unwrap();
        mode.enable().await.unwrap();
        eventually("the watch", || {
            host.paths().contains(&"/v1/watch".to_string())
        })
        .await;
        host.state.send_replace("fatal".into());
        eventually("the error", || mode.state().phase == ConnectionPhase::Error).await;
        assert_eq!(mode.state().reason.unwrap().detail, "TUNNEL_ENGINE_FATAL");
        host.state.send_replace("degraded".into());
        eventually("on again", || mode.state().phase == ConnectionPhase::On).await;
    }

    #[tokio::test]
    async fn disconnect_stops_the_engine_and_the_end_of_the_watch_is_not_an_error() {
        let dir = temp_dir();
        let host = host(&dir, "configured");
        let (mode, phases) = mode(&dir);
        mode.set_profile(PROFILE, "r1").await.unwrap();
        mode.enable().await.unwrap();
        mode.disable().await.unwrap();
        assert_eq!(mode.state(), EnhancedState::default());
        assert!(host.paths().contains(&"/v1/stop".to_string()));
        host.closed.send_replace(true);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(mode.state().phase, ConnectionPhase::Off);
        assert_eq!(phases.lock().unwrap().last(), Some(&ConnectionPhase::Off));
    }

    #[tokio::test]
    async fn disconnect_without_a_host_ends_off() {
        let dir = temp_dir();
        let (mode, _) = mode(&dir);
        mode.disable().await.unwrap();
        assert_eq!(mode.state().phase, ConnectionPhase::Off);
    }

    #[tokio::test]
    async fn a_new_profile_is_applied_while_on_and_kept_while_off() {
        let dir = temp_dir();
        let host = host(&dir, "configured");
        let (mode, _) = mode(&dir);
        mode.set_profile(PROFILE, "r1").await.unwrap();
        assert!(host.paths().is_empty());
        mode.enable().await.unwrap();
        mode.set_profile(PROFILE, "r1").await.unwrap();
        mode.set_profile(br#"{"revision":"r2","nodes":[]}"#, "r2")
            .await
            .unwrap();
        let applies = host
            .paths()
            .iter()
            .filter(|path| *path == "/v1/apply-profile")
            .count();
        assert_eq!(applies, 2);
    }

    #[tokio::test]
    async fn restore_attaches_to_a_running_tunnel() {
        let dir = temp_dir();
        let host = host(&dir, "running");
        let (mode, _) = mode(&dir);
        mode.restore().await;
        assert_eq!(mode.state().phase, ConnectionPhase::On);
        assert!(!host.paths().contains(&"/v1/apply-profile".to_string()));
        mode.set_profile(PROFILE, "r1").await.unwrap();
        assert!(!host.paths().contains(&"/v1/apply-profile".to_string()));
    }

    #[tokio::test]
    async fn restore_without_a_tunnel_stays_off() {
        let (mode, phases) = mode(&temp_dir());
        mode.restore().await;
        assert_eq!(mode.state().phase, ConnectionPhase::Off);
        assert!(phases.lock().unwrap().is_empty());
    }
}
