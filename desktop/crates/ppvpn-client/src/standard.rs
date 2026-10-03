//! Standard mode: an unprivileged `ppvpn-core serve --tun=false
//! --local-proxy=true` child process owned by this app (ported from
//! `src-tauri/src/standard_core.rs`).
//!
//! It serves the per-node local HTTP/SOCKS5 proxies and every speed test
//! (ICMP/TCP entrance probes and Connect availability probes). It starts when
//! the first profile is applied and lives until sign-out or exit, independent
//! of enhanced mode: the privileged core runs TUN only (`--local-proxy=false`),
//! so local proxy ports stay stable while enhanced mode is toggled.
//!
//! The child is supervised: an unexpected exit restarts it with backoff (at
//! most [`RestartBudget::MAX_RESTARTS`] times per minute) and re-applies the
//! last profile; state changes go to the `on_state` sink.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::core_ipc::{self, BoxFuture, CoreCallError, CoreClient, CoreEndpoint, CoreTransport};
use crate::errors::{ClientError, ClientErrorInfo, ErrorCode};
use crate::routing::RoutingModeCell;
use crate::{ClientConfig, LocalProxy, ProbeMethod, ProbeResult, RoutingMode, StandardState};

const READY_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_GRACE: Duration = Duration::from_secs(3);

/// Receives every standard-mode state change.
pub(crate) type StateSink = Arc<dyn Fn(StandardState) + Send + Sync>;

// ---------------------------------------------------------------------------
// IPC path selection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostOs {
    MacOs,
    Linux,
    Windows,
}

impl HostOs {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// Longest usable Unix socket path in bytes (`sun_path` minus the NUL).
    fn max_socket_path(self) -> usize {
        match self {
            Self::MacOs => 104 - 1,
            Self::Linux => 108 - 1,
            Self::Windows => usize::MAX,
        }
    }
}

/// Picks the private IPC endpoint for one core launch. `token` is 32 random
/// hex characters. macOS: `$TMPDIR` (per-user, 0700); Linux:
/// `$XDG_RUNTIME_DIR/ppvpn`, then `<data_dir>/ppvpn-core/run`; either falls back to
/// `/tmp` when the path would overflow `sun_path` (the socket's owner is
/// verified before every call). Windows: an unguessable named pipe.
pub(crate) fn select_ipc_path(
    os: HostOs,
    tmpdir: Option<&str>,
    xdg_runtime_dir: Option<&str>,
    data_dir: &str,
    token: &str,
) -> String {
    if os == HostOs::Windows {
        return format!(r"\\.\pipe\ppvpn-core-user-{token}");
    }
    let short = &token[..token.len().min(12)];
    let name = format!("ppvpn-core-{short}.sock");
    // Joined with '/' by hand (not `Path::join`) so the result does not
    // depend on the host the code runs on.
    let join = |dir: &str, parts: &[&str]| {
        let mut path = dir.trim_end_matches('/').to_string();
        for part in parts {
            path.push('/');
            path.push_str(part);
        }
        path
    };
    let non_empty = |dir: Option<&str>| dir.filter(|dir| !dir.is_empty()).map(str::to_string);
    let candidates: Vec<String> = match os {
        HostOs::MacOs => non_empty(tmpdir)
            .map(|dir| join(&dir, &[&name]))
            .into_iter()
            .collect(),
        _ => non_empty(xdg_runtime_dir)
            .map(|dir| join(&dir, &["ppvpn", &name]))
            .into_iter()
            .chain(
                non_empty(Some(data_dir))
                    .map(|dir| join(&dir, &[crate::storage::CORE_DIR, "run", &name])),
            )
            .collect(),
    };
    candidates
        .into_iter()
        .find(|path| path.len() <= os.max_socket_path())
        .unwrap_or_else(|| format!("/tmp/{name}"))
}

// ---------------------------------------------------------------------------
// Restart policy
// ---------------------------------------------------------------------------

/// Bounded crash-restart policy: exponential backoff, at most
/// `MAX_RESTARTS` restarts inside a sliding `WINDOW`.
#[derive(Debug, Default)]
pub(crate) struct RestartBudget {
    recent: VecDeque<Instant>,
}

impl RestartBudget {
    pub(crate) const MAX_RESTARTS: usize = 3;
    const WINDOW: Duration = Duration::from_secs(60);
    const BASE_DELAY: Duration = Duration::from_millis(500);

    /// Delay before the next restart, or `None` when the budget is spent.
    pub(crate) fn next_delay(&mut self, now: Instant) -> Option<Duration> {
        while self
            .recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Self::WINDOW)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= Self::MAX_RESTARTS {
            return None;
        }
        let delay = Self::BASE_DELAY * 2u32.pow(self.recent.len() as u32);
        self.recent.push_back(now);
        Some(delay)
    }

    pub(crate) fn reset(&mut self) {
        self.recent.clear();
    }

    /// Spends the whole budget at `now`: no automatic restart for a window.
    pub(crate) fn exhaust(&mut self, now: Instant) {
        self.recent = std::iter::repeat_n(now, Self::MAX_RESTARTS).collect();
    }
}

// ---------------------------------------------------------------------------
// Standard core
// ---------------------------------------------------------------------------

/// A launched core as seen by the controller.
pub(crate) struct Launched {
    pub transport: Arc<dyn CoreTransport>,
    /// Resolves with a status text once the core has exited, for any reason.
    pub exited: BoxFuture<'static, String>,
    /// Asks the core to go away (lifetime pipe closed, killed after a grace
    /// period). Dropping it has the same effect.
    pub stop: oneshot::Sender<()>,
    /// `allowed_rule_set_hosts` for every `apply-profile` on this instance;
    /// `None` when the core predates the field (it would reject the body).
    pub rule_set_hosts: Option<Vec<String>>,
    /// The core accepts `routing_mode` (0.5.6+); older ones reject it.
    pub accepts_routing_mode: bool,
    /// The core serves the routed local-proxy user (0.5.12+); older ones
    /// reject the `kind` field.
    pub accepts_routed_proxy: bool,
}

/// Starts one core instance and waits until it answers. The production
/// implementation spawns `ppvpn-core`; tests substitute a fake.
pub(crate) trait CoreLauncher: Send + Sync {
    fn launch(&self) -> BoxFuture<'_, Result<Launched, ClientErrorInfo>>;
}

/// Spawns the bundled `ppvpn-core serve --tun=false --local-proxy=true`.
pub(crate) struct ProcessLauncher {
    binary: PathBuf,
    runtime_dir: PathBuf,
    log_dir: PathBuf,
    data_dir: String,
    platform: String,
    /// Authority of the API the profile comes from (`allowed_rule_set_hosts`).
    rule_set_hosts: Vec<String>,
}

impl ProcessLauncher {
    pub(crate) fn new(config: &ClientConfig) -> Self {
        let binary_name = if cfg!(windows) {
            "ppvpn-core.exe"
        } else {
            "ppvpn-core"
        };
        Self {
            binary: Path::new(&config.core_bin_dir).join(binary_name),
            runtime_dir: Path::new(&config.data_dir).join(crate::storage::CORE_DIR),
            log_dir: PathBuf::from(&config.log_dir),
            data_dir: config.data_dir.clone(),
            platform: config.platform.clone(),
            rule_set_hosts: core_ipc::rule_set_hosts(&config.api_base),
        }
    }

    async fn spawn(&self) -> Result<Launched, ClientErrorInfo> {
        let binary = self.binary.clone();
        let runtime_dir = self.runtime_dir.clone();
        let log_dir = self.log_dir.clone();
        let data_dir = self.data_dir.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            prepare_launch(&binary, &runtime_dir, &log_dir, &data_dir)
        })
        .await
        .map_err(|error| {
            ClientErrorInfo::new(ErrorCode::Internal, format!("prepare core task: {error}"))
        })??;

        let mut command = Command::new(&self.binary);
        command
            .arg("serve")
            .arg("--socket")
            .arg(&prepared.endpoint.socket)
            .arg("--session-secret-file")
            .arg(&prepared.endpoint.secret_file)
            .arg("--state-dir")
            .arg(self.runtime_dir.join("state"))
            .arg("--platform")
            .arg(&self.platform)
            .arg("--local-proxy=true")
            .arg("--tun=false")
            .arg("--exit-on-stdin-close")
            .stdin(Stdio::piped())
            .kill_on_drop(true);
        match prepared
            .log
            .and_then(|file| Some((file.try_clone().ok()?, file)))
        {
            Some((out, err)) => {
                command.stdout(Stdio::from(out)).stderr(Stdio::from(err));
            }
            None => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let mut child = command.spawn().map_err(|error| {
            ClientErrorInfo::new(
                ErrorCode::StandardCoreFailed,
                format!("STANDARD_CORE_SPAWN_FAILED: {error}"),
            )
        })?;
        let stdin = child.stdin.take();
        let client = Arc::new(CoreClient::new(prepared.endpoint.clone()));

        let version = match wait_until_ready(&mut child, client.as_ref()).await {
            Ok(version) => version,
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                cleanup_files(prepared.endpoint.clone()).await;
                return Err(error);
            }
        };
        let rule_set_hosts = core_ipc::core_accepts_rule_set_hosts(&version.core_version)
            .then(|| self.rule_set_hosts.clone());
        if rule_set_hosts.is_none() {
            tracing::info!(
                core = %version.core_version,
                "standard core predates allowed_rule_set_hosts; rule sets stay unpinned"
            );
        }
        let accepts_routing_mode = crate::routing::core_accepts_routing_mode(&version.core_version);
        let accepts_routed_proxy = core_ipc::core_accepts_routed_proxy(&version.core_version);
        let (stop, stop_rx) = oneshot::channel();
        let exited = Box::pin(watch_child(child, stdin, stop_rx, prepared.endpoint));
        Ok(Launched {
            transport: client,
            exited,
            stop,
            rule_set_hosts,
            accepts_routing_mode,
            accepts_routed_proxy,
        })
    }
}

impl CoreLauncher for ProcessLauncher {
    fn launch(&self) -> BoxFuture<'_, Result<Launched, ClientErrorInfo>> {
        Box::pin(self.spawn())
    }
}

/// Owns the child until it exits; a stop request (or a dropped sender)
/// closes the lifetime pipe — `--exit-on-stdin-close` then stops the core,
/// even if this process dies without cleanup — and kills it after a grace
/// period.
async fn watch_child(
    mut child: Child,
    stdin: Option<ChildStdin>,
    stop_rx: oneshot::Receiver<()>,
    endpoint: CoreEndpoint,
) -> String {
    let status = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => status.to_string(),
            Err(error) => format!("wait failed: {error}"),
        },
        _ = stop_rx => {
            drop(stdin);
            if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            "stopped".to_string()
        }
    };
    cleanup_files(endpoint).await;
    status
}

#[derive(Clone)]
struct Desired {
    profile: Arc<Value>,
    revision: String,
}

struct Running {
    instance: u64,
    transport: Arc<dyn CoreTransport>,
    rule_set_hosts: Option<Vec<String>>,
    accepts_routing_mode: bool,
    accepts_routed_proxy: bool,
    applied_revision: Option<String>,
    /// `routing_mode` `applied_revision` was applied with (`None`: the core
    /// predates it).
    applied_mode: Option<RoutingMode>,
    started: bool,
    stop: Option<oneshot::Sender<()>>,
    supervisor: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Slot {
    next_instance: u64,
    running: Option<Running>,
    desired: Option<Desired>,
    restarts: RestartBudget,
    state: StandardState,
}

struct Inner {
    launcher: Arc<dyn CoreLauncher>,
    on_state: StateSink,
    /// Serialises spawn / apply / stop so concurrent refreshes cannot start
    /// two cores.
    operation: tokio::sync::Mutex<()>,
    slot: Mutex<Slot>,
    /// Sent with every `apply-profile` (shared with the client).
    routing: RoutingModeCell,
}

/// Handle to the standard-mode core; cheap to clone.
#[derive(Clone)]
pub(crate) struct StandardCore {
    inner: Arc<Inner>,
}

impl StandardCore {
    /// Creates the controller; nothing is launched until
    /// [`Self::apply_profile`].
    #[cfg(test)]
    pub(crate) fn with_launcher(launcher: Arc<dyn CoreLauncher>, on_state: StateSink) -> Self {
        Self::with_routing(launcher, on_state, RoutingModeCell::default())
    }

    /// [`Self::with_launcher`], applying profiles in `routing`'s mode.
    pub(crate) fn with_routing(
        launcher: Arc<dyn CoreLauncher>,
        on_state: StateSink,
        routing: RoutingModeCell,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                launcher,
                on_state,
                operation: tokio::sync::Mutex::new(()),
                slot: Mutex::new(Slot::default()),
                routing,
            }),
        }
    }

    /// Applies the desired profile again when the routing mode changed
    /// since it was applied (no respawn: the core swaps its runtime). A
    /// no-op without a profile or a running core.
    pub(crate) async fn reapply(&self) -> Result<(), ClientError> {
        let _operation = self.inner.operation.lock().await;
        let ready = self
            .inner
            .slot
            .lock()
            .map(|slot| slot.desired.is_some() && slot.running.is_some())
            .unwrap_or(false);
        if !ready {
            return Ok(());
        }
        self.inner.ensure_applied().await
    }

    pub(crate) fn state(&self) -> StandardState {
        self.inner
            .slot
            .lock()
            .map(|slot| slot.state.clone())
            .unwrap_or_default()
    }

    /// Ensures the core runs `profile` (JSON bytes) at `revision`, spawning it
    /// first when needed. Same revision on a live core is a no-op. The profile
    /// is remembered and re-applied after a crash restart.
    ///
    /// A new revision (or a first start) refills the crash-restart budget. The
    /// same revision after a failure is one more attempt only: it spawns or
    /// applies once, and a crash right after it fails again instead of
    /// restarting, so periodic retries of a broken core stay one per call.
    pub(crate) async fn apply_profile(
        &self,
        profile: &[u8],
        revision: &str,
    ) -> Result<(), ClientError> {
        if revision.is_empty() {
            return Err(ClientError::failed(
                ErrorCode::ProfileInvalid,
                "PROFILE_REVISION_REQUIRED",
            ));
        }
        let profile: Value = serde_json::from_slice(profile).map_err(|error| {
            ClientError::failed(
                ErrorCode::ProfileInvalid,
                format!("PROFILE_JSON_INVALID: {error}"),
            )
        })?;
        let _operation = self.inner.operation.lock().await;
        if let Ok(mut slot) = self.inner.slot.lock() {
            let retry = matches!(slot.state, StandardState::Failed { .. })
                && slot
                    .desired
                    .as_ref()
                    .is_some_and(|desired| desired.revision == revision);
            slot.desired = Some(Desired {
                profile: Arc::new(profile),
                revision: revision.to_string(),
            });
            if retry {
                slot.restarts.exhaust(Instant::now());
            } else {
                slot.restarts.reset();
            }
        }
        self.inner.ensure_applied().await
    }

    /// Stops the core gracefully and forgets the profile.
    pub(crate) async fn stop(&self) {
        let _operation = self.inner.operation.lock().await;
        let running = self.inner.slot.lock().ok().and_then(|mut slot| {
            slot.desired = None;
            slot.running.take()
        });
        let was_running = running.is_some();
        if let Some(running) = running {
            stop_instance(running).await;
        }
        if was_running || !matches!(self.state(), StandardState::Stopped) {
            self.inner.publish(StandardState::Stopped);
        }
    }

    /// Non-blocking stop for app exit: closes the lifetime pipe (the core
    /// exits on its own) and kills it after a grace period. Does not wait.
    pub(crate) fn shutdown(&self) {
        let running = self.inner.slot.lock().ok().and_then(|mut slot| {
            slot.desired = None;
            slot.running.take()
        });
        if let Some(mut running) = running {
            if let Some(stop) = running.stop.take() {
                let _ = stop.send(());
            }
        }
        self.inner.publish(StandardState::Stopped);
    }

    /// Core API transport of the ready core.
    pub(crate) fn transport(&self) -> Result<Arc<dyn CoreTransport>, ClientError> {
        let slot = self
            .inner
            .slot
            .lock()
            .map_err(|_| ClientError::StandardNotReady)?;
        match slot.running.as_ref() {
            Some(running) if running.started => Ok(running.transport.clone()),
            _ => Err(ClientError::StandardNotReady),
        }
    }

    /// Speed test through this core; see [`core_ipc::run_probe`]. Exactly one
    /// result per requested node reaches `on_result` before this returns.
    pub(crate) async fn probe(
        &self,
        method: ProbeMethod,
        node_ids: Vec<String>,
        on_result: &(dyn Fn(ProbeResult) + Send + Sync),
    ) -> Result<(), ClientError> {
        let transport = self.transport()?;
        core_ipc::run_probe(transport, method, node_ids, None, on_result).await
    }

    /// Per-node loopback proxies with their credentials.
    pub(crate) async fn local_proxies(&self) -> Result<Vec<LocalProxy>, ClientError> {
        let transport = self.transport()?;
        core_ipc::local_proxies(transport.as_ref())
            .await
            .map_err(|error| error.into_client_error(ErrorCode::StandardCoreFailed))
    }

    /// The routed local-proxy user (Profile rules, then the selected node);
    /// `None` when this core predates it (0.5.12).
    pub(crate) async fn routed_local_proxy(&self) -> Result<Option<LocalProxy>, ClientError> {
        let (transport, supported) = {
            let slot = self
                .inner
                .slot
                .lock()
                .map_err(|_| ClientError::StandardNotReady)?;
            match slot.running.as_ref() {
                Some(running) if running.started => {
                    (running.transport.clone(), running.accepts_routed_proxy)
                }
                _ => return Err(ClientError::StandardNotReady),
            }
        };
        if !supported {
            return Ok(None);
        }
        core_ipc::routed_local_proxy(transport.as_ref())
            .await
            .map_err(|error| error.into_client_error(ErrorCode::StandardCoreFailed))
    }
}

impl Inner {
    fn publish(&self, state: StandardState) {
        if let Ok(mut slot) = self.slot.lock() {
            slot.state = state.clone();
        }
        (self.on_state)(state);
    }

    fn fail(&self, error: ClientErrorInfo) -> ClientError {
        self.publish(StandardState::Failed {
            error: error.clone(),
        });
        error.into()
    }

    /// Spawns (if needed) and applies the desired profile. Caller holds
    /// `operation`.
    async fn ensure_applied(self: &Arc<Self>) -> Result<(), ClientError> {
        let Some(desired) = self.slot.lock().ok().and_then(|slot| slot.desired.clone()) else {
            return Err(ClientError::StandardNotReady);
        };
        let needs_spawn = self
            .slot
            .lock()
            .map(|slot| slot.running.is_none())
            .unwrap_or(true);
        if needs_spawn {
            self.publish(StandardState::Starting);
            let running = match self.spawn_instance().await {
                Ok(running) => running,
                Err(error) => return Err(self.fail(error)),
            };
            if let Ok(mut slot) = self.slot.lock() {
                slot.running = Some(running);
            }
        }
        let Some((transport, hosts, applied, applied_mode, mode, started)) =
            self.slot.lock().ok().and_then(|slot| {
                slot.running.as_ref().map(|r| {
                    (
                        r.transport.clone(),
                        r.rule_set_hosts.clone(),
                        r.applied_revision.clone(),
                        r.applied_mode,
                        r.accepts_routing_mode.then(|| self.routing.get()),
                        r.started,
                    )
                })
            })
        else {
            return Err(self.fail(ClientErrorInfo::new(
                ErrorCode::StandardCoreFailed,
                "STANDARD_CORE_EXITED",
            )));
        };
        if started && applied.as_deref() == Some(desired.revision.as_str()) && applied_mode == mode
        {
            self.publish(StandardState::Ready {
                revision: desired.revision,
            });
            return Ok(());
        }

        // ApplyProfile swaps a running runtime atomically and rolls back on
        // failure, so only the first revision needs an explicit start.
        if let Err(error) =
            core_ipc::apply_profile(transport.as_ref(), &desired.profile, hosts.as_deref(), mode)
                .await
        {
            let info = error.info(ErrorCode::StandardCoreFailed);
            return Err(match (started, applied) {
                // The previous revision keeps serving; report without
                // leaving the ready state.
                (true, Some(previous)) => {
                    self.publish(StandardState::Ready { revision: previous });
                    info.into()
                }
                _ => self.fail(info),
            });
        }
        if !started {
            if let Err(error) = core_ipc::start(transport.as_ref()).await {
                return Err(self.fail(error.info(ErrorCode::StandardCoreFailed)));
            }
        }
        if let Ok(mut slot) = self.slot.lock() {
            if let Some(running) = slot.running.as_mut() {
                running.started = true;
                running.applied_revision = Some(desired.revision.clone());
                running.applied_mode = mode;
            }
        }
        self.publish(StandardState::Ready {
            revision: desired.revision,
        });
        Ok(())
    }

    async fn spawn_instance(self: &Arc<Self>) -> Result<Running, ClientErrorInfo> {
        let launched = self.launcher.launch().await?;
        let instance = self
            .slot
            .lock()
            .map(|mut slot| {
                slot.next_instance += 1;
                slot.next_instance
            })
            .unwrap_or(0);
        let inner = Arc::downgrade(self);
        let exited = launched.exited;
        let supervisor = tokio::spawn(async move {
            let status = exited.await;
            if let Some(inner) = Weak::upgrade(&inner) {
                inner.on_exit(instance, status);
            }
        });
        Ok(Running {
            instance,
            transport: launched.transport,
            rule_set_hosts: launched.rule_set_hosts,
            accepts_routing_mode: launched.accepts_routing_mode,
            accepts_routed_proxy: launched.accepts_routed_proxy,
            applied_revision: None,
            applied_mode: None,
            started: false,
            stop: Some(launched.stop),
            supervisor: Some(supervisor),
        })
    }

    /// Called by the supervisor when an instance exited. Deliberate stops
    /// have already removed the instance from the slot and are ignored.
    fn on_exit(self: &Arc<Self>, instance: u64, status: String) {
        let delay = {
            let Ok(mut slot) = self.slot.lock() else {
                return;
            };
            if slot.running.as_ref().map(|r| r.instance) != Some(instance) {
                return;
            }
            slot.running = None;
            if slot.desired.is_some() {
                slot.restarts.next_delay(Instant::now())
            } else {
                None
            }
        };
        tracing::warn!(%status, "standard core exited unexpectedly");
        let Some(delay) = delay else {
            self.publish(StandardState::Failed {
                error: ClientErrorInfo::new(
                    ErrorCode::StandardCoreFailed,
                    format!("STANDARD_CORE_EXITED: {status}"),
                ),
            });
            return;
        };
        self.publish(StandardState::Starting);
        let inner = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _operation = inner.operation.lock().await;
            let still_wanted = inner
                .slot
                .lock()
                .map(|slot| slot.running.is_none() && slot.desired.is_some())
                .unwrap_or(false);
            if still_wanted {
                if let Err(error) = inner.ensure_applied().await {
                    tracing::warn!(?error, "standard core restart failed");
                }
            }
        });
    }
}

struct Prepared {
    endpoint: CoreEndpoint,
    log: Option<std::fs::File>,
}

/// Blocking launch preparation: binary check, private runtime directory,
/// endpoint choice and log file.
fn prepare_launch(
    binary: &Path,
    runtime_dir: &Path,
    log_dir: &Path,
    data_dir: &str,
) -> Result<Prepared, ClientErrorInfo> {
    if !binary.is_file() {
        return Err(ClientErrorInfo::new(
            ErrorCode::CoreBinaryMissing,
            format!("STANDARD_CORE_BINARY_MISSING: {}", binary.display()),
        ));
    }
    std::fs::create_dir_all(runtime_dir.join("state")).map_err(|error| {
        ClientErrorInfo::new(
            ErrorCode::StandardCoreFailed,
            format!("create core dir: {error}"),
        )
    })?;
    #[cfg(unix)]
    let expected_uid = {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let _ = std::fs::set_permissions(runtime_dir, std::fs::Permissions::from_mode(0o700));
        std::fs::metadata(runtime_dir)
            .ok()
            .map(|metadata| metadata.uid())
    };
    #[cfg(not(unix))]
    let expected_uid = None;

    let secret_file = runtime_dir.join("session.secret");
    let _ = std::fs::remove_file(&secret_file);
    let token = uuid::Uuid::new_v4().simple().to_string();
    let tmpdir = std::env::temp_dir();
    let xdg = std::env::var("XDG_RUNTIME_DIR").ok();
    let socket = select_ipc_path(
        HostOs::current(),
        tmpdir.to_str(),
        xdg.as_deref(),
        data_dir,
        &token,
    );
    let log = std::fs::create_dir_all(log_dir).ok().and_then(|_| {
        std::fs::File::options()
            .create(true)
            .append(true)
            .open(log_dir.join(format!("ppvpn-core.{}.log", utc_date(SystemTime::now()))))
            .ok()
    });
    Ok(Prepared {
        endpoint: CoreEndpoint {
            socket,
            secret_file,
            expected_uid,
        },
        log,
    })
}

/// Polls `GetVersion` until the core answers, it exits, or the timeout hits.
async fn wait_until_ready(
    child: &mut Child,
    client: &CoreClient,
) -> Result<core_ipc::CoreVersion, ClientErrorInfo> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut last = "STANDARD_CORE_START_TIMEOUT".to_string();
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(ClientErrorInfo::new(
                ErrorCode::StandardCoreFailed,
                format!("STANDARD_CORE_EXITED: {status}"),
            ));
        }
        match core_ipc::get_version_within(client, Duration::from_secs(2)).await {
            Ok(version) if version.core_api_version > 1 => {
                return Err(ClientErrorInfo::new(
                    ErrorCode::CoreIncompatible,
                    format!("CORE_API_UNSUPPORTED: {}", version.core_api_version),
                ));
            }
            Ok(version) => return Ok(version),
            Err(CoreCallError::Api { code, .. }) if code == "CORE_API_UNSUPPORTED" => {
                return Err(ClientErrorInfo::new(ErrorCode::CoreIncompatible, code));
            }
            Err(error) => last = format!("STANDARD_CORE_START_TIMEOUT: {}", error.detail()),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(ClientErrorInfo::new(ErrorCode::StandardCoreFailed, last))
}

async fn stop_instance(mut running: Running) {
    let _ = core_ipc::stop(running.transport.as_ref(), Duration::from_secs(2)).await;
    if let Some(stop) = running.stop.take() {
        let _ = stop.send(());
    }
    if let Some(supervisor) = running.supervisor.take() {
        let _ = tokio::time::timeout(STOP_GRACE + Duration::from_secs(2), supervisor).await;
    }
}

async fn cleanup_files(endpoint: CoreEndpoint) {
    let _ = tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            if std::fs::symlink_metadata(&endpoint.socket).is_ok_and(|m| m.file_type().is_socket())
            {
                let _ = std::fs::remove_file(&endpoint.socket);
            }
        }
        let _ = std::fs::remove_file(&endpoint.secret_file);
    })
    .await;
}

/// `YYYY-MM-DD` in UTC (log file names; no date crate needed).
pub(crate) fn utc_date(now: SystemTime) -> String {
    let days = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn macos_uses_tmpdir_and_falls_back_when_sun_path_overflows() {
        let tmp = "/var/folders/xy/abcdefghijklmnopqrstuvwx0000gn/T/";
        let path = select_ipc_path(HostOs::MacOs, Some(tmp), None, "/unused", TOKEN);
        assert_eq!(path, format!("{tmp}ppvpn-core-0123456789ab.sock"));
        assert!(path.len() <= 103);

        let long = format!("/Users/{}/T", "x".repeat(80));
        let path = select_ipc_path(HostOs::MacOs, Some(&long), None, "/unused", TOKEN);
        assert_eq!(path, "/tmp/ppvpn-core-0123456789ab.sock");

        // Exactly at the limit is still accepted.
        let name_len = "/ppvpn-core-0123456789ab.sock".len();
        let edge = format!("/{}", "d".repeat(103 - name_len - 1));
        let path = select_ipc_path(HostOs::MacOs, Some(&edge), None, "/unused", TOKEN);
        assert_eq!(path.len(), 103);
        assert!(path.starts_with(&edge));
        let over = format!("{edge}e");
        assert!(
            select_ipc_path(HostOs::MacOs, Some(&over), None, "/unused", TOKEN)
                .starts_with("/tmp/")
        );

        assert!(select_ipc_path(HostOs::MacOs, None, None, "/unused", TOKEN).starts_with("/tmp/"));
    }

    #[test]
    fn linux_prefers_xdg_runtime_dir_then_data_dir() {
        let path = select_ipc_path(
            HostOs::Linux,
            Some("/tmp"),
            Some("/run/user/1000"),
            "/data/u/.local/share/ppvpn",
            TOKEN,
        );
        assert_eq!(path, "/run/user/1000/ppvpn/ppvpn-core-0123456789ab.sock");
        let path = select_ipc_path(
            HostOs::Linux,
            None,
            None,
            "/data/u/.local/share/ppvpn",
            TOKEN,
        );
        assert_eq!(
            path,
            "/data/u/.local/share/ppvpn/ppvpn-core/run/ppvpn-core-0123456789ab.sock"
        );
        let path = select_ipc_path(HostOs::Linux, None, Some(""), "/data/u/data", TOKEN);
        assert!(path.starts_with("/data/u/data/ppvpn-core/run/"));
        let deep = format!("/home/{}", "y".repeat(120));
        let path = select_ipc_path(HostOs::Linux, None, None, &deep, TOKEN);
        assert_eq!(path, "/tmp/ppvpn-core-0123456789ab.sock");
    }

    #[test]
    fn windows_uses_a_random_named_pipe() {
        let path = select_ipc_path(HostOs::Windows, None, None, r"C:\data", TOKEN);
        assert_eq!(path, format!(r"\\.\pipe\ppvpn-core-user-{TOKEN}"));
    }

    #[test]
    fn restart_budget_backs_off_and_refills_after_the_window() {
        let start = Instant::now();
        let mut budget = RestartBudget::default();
        assert_eq!(budget.next_delay(start), Some(Duration::from_millis(500)));
        assert_eq!(budget.next_delay(start), Some(Duration::from_millis(1000)));
        assert_eq!(budget.next_delay(start), Some(Duration::from_millis(2000)));
        assert_eq!(budget.next_delay(start), None);
        assert_eq!(
            budget.next_delay(start + Duration::from_secs(61)),
            Some(Duration::from_millis(500))
        );
        budget.reset();
        assert_eq!(budget.next_delay(start), Some(Duration::from_millis(500)));
    }

    #[test]
    fn utc_date_formats_known_days() {
        assert_eq!(utc_date(UNIX_EPOCH), "1970-01-01");
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29"
        );
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(1_790_640_000)),
            "2026-09-29"
        );
    }

    fn test_config(bin_dir: &str, data_dir: &str) -> ClientConfig {
        ClientConfig {
            api_base: "http://localhost".into(),
            data_dir: data_dir.into(),
            log_dir: format!("{data_dir}/logs"),
            core_bin_dir: bin_dir.into(),
            platform: "macos".into(),
            app_version: "0.0.0".into(),
        }
    }

    fn process_launcher(config: &ClientConfig) -> Arc<dyn CoreLauncher> {
        Arc::new(ProcessLauncher::new(config))
    }

    /// Launches fake cores whose exit can be triggered to simulate a crash.
    struct CrashyLauncher {
        core: Arc<crate::core_ipc::tests::FakeCore>,
        crash: Mutex<Vec<oneshot::Sender<()>>>,
        /// The next launches fail (like a missing binary).
        fail: std::sync::atomic::AtomicBool,
        launches: std::sync::atomic::AtomicUsize,
        /// What [`Launched::rule_set_hosts`] reports for each instance.
        rule_set_hosts: Option<Vec<String>>,
    }

    impl CrashyLauncher {
        fn new(core: Arc<crate::core_ipc::tests::FakeCore>) -> Arc<Self> {
            Self::with_hosts(core, None)
        }

        fn with_hosts(
            core: Arc<crate::core_ipc::tests::FakeCore>,
            rule_set_hosts: Option<Vec<String>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                core,
                crash: Mutex::new(Vec::new()),
                fail: Default::default(),
                launches: Default::default(),
                rule_set_hosts,
            })
        }

        fn launches(&self) -> usize {
            self.launches.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// Crashes the newest instance.
        fn crash_last(&self) {
            if let Some(crash) = self.crash.lock().unwrap().pop() {
                let _ = crash.send(());
            }
        }
    }

    impl CoreLauncher for CrashyLauncher {
        fn launch(&self) -> BoxFuture<'_, Result<Launched, ClientErrorInfo>> {
            Box::pin(async move {
                self.launches
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(ClientErrorInfo::new(
                        ErrorCode::CoreBinaryMissing,
                        "test launch failure",
                    ));
                }
                let (stop, stopped) = oneshot::channel::<()>();
                let (crash, crashed) = oneshot::channel::<()>();
                self.crash.lock().unwrap().push(crash);
                Ok(Launched {
                    transport: self.core.clone(),
                    exited: Box::pin(async move {
                        tokio::select! {
                            _ = stopped => "stopped".to_string(),
                            _ = crashed => "signal: 9".to_string(),
                        }
                    }),
                    stop,
                    rule_set_hosts: self.rule_set_hosts.clone(),
                    // A core that pins hosts stands for a current one.
                    accepts_routing_mode: self.rule_set_hosts.is_some(),
                    accepts_routed_proxy: self.rule_set_hosts.is_some(),
                })
            })
        }
    }

    #[tokio::test]
    async fn the_routed_proxy_is_asked_only_of_a_core_that_serves_it() {
        let core = crate::core_ipc::tests::FakeCore::new(|path, _| {
            let value = match path {
                "/v1/get-local-proxy-credential" => serde_json::json!({
                    "kind": "routed", "node_id": "", "listen": "127.0.0.1", "port": 7890,
                    "username": "abc", "password": "secret"
                }),
                _ => serde_json::json!({"applied": true}),
            };
            (Duration::ZERO, Ok(value))
        });
        let asked = |core: &crate::core_ipc::tests::FakeCore| {
            core.calls
                .lock()
                .unwrap()
                .iter()
                .any(|(path, _)| path == "/v1/get-local-proxy-credential")
        };

        // An older core (no pinned hosts here stands for one): no request.
        let old = StandardCore::with_launcher(CrashyLauncher::new(core.clone()), Arc::new(|_| {}));
        assert!(matches!(
            old.routed_local_proxy().await,
            Err(ClientError::StandardNotReady)
        ));
        old.apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap();
        assert!(old.routed_local_proxy().await.unwrap().is_none());
        assert!(!asked(&core));
        old.stop().await;

        let current = StandardCore::with_launcher(
            CrashyLauncher::with_hosts(core.clone(), Some(vec!["127.0.0.1".into()])),
            Arc::new(|_| {}),
        );
        current
            .apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap();
        let routed = current.routed_local_proxy().await.unwrap().unwrap();
        assert_eq!((routed.username.as_str(), routed.port), ("abc", 7890));
        assert!(asked(&core));
        current.stop().await;
    }

    #[tokio::test]
    async fn crash_restarts_and_reapplies_the_profile() {
        let core = crate::core_ipc::tests::FakeCore::new(|_, _| {
            (Duration::ZERO, Ok(serde_json::json!({"applied": true})))
        });
        let launcher = CrashyLauncher::new(core.clone());
        let standard = StandardCore::with_launcher(launcher.clone(), Arc::new(|_| {}));
        standard
            .apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap();
        assert!(matches!(standard.state(), StandardState::Ready { .. }));
        // Same revision again: no second apply.
        standard
            .apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap();
        let applies = |core: &crate::core_ipc::tests::FakeCore| {
            core.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(path, _)| path == "/v1/apply-profile")
                .count()
        };
        assert_eq!(applies(&core), 1);

        let crash = launcher.crash.lock().unwrap().remove(0);
        let _ = crash.send(());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(matches!(standard.state(), StandardState::Starting));
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            matches!(standard.state(), StandardState::Ready { ref revision } if revision == "r1")
        );
        assert_eq!(applies(&core), 2, "profile re-applied after restart");
        assert_eq!(launcher.crash.lock().unwrap().len(), 1, "one new instance");

        standard.stop().await;
        assert!(matches!(standard.state(), StandardState::Stopped));
        assert!(matches!(
            standard.transport(),
            Err(ClientError::StandardNotReady)
        ));
    }

    #[tokio::test]
    async fn retrying_a_failed_core_makes_one_attempt_per_call() {
        let core = crate::core_ipc::tests::FakeCore::new(|_, _| {
            (Duration::ZERO, Ok(serde_json::json!({"applied": true})))
        });
        let launcher = CrashyLauncher::new(core.clone());
        let standard = StandardCore::with_launcher(launcher.clone(), Arc::new(|_| {}));
        let profile = br#"{"revision":"r1"}"#;

        // The first start fails.
        launcher
            .fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(standard.apply_profile(profile, "r1").await.is_err());
        assert!(matches!(standard.state(), StandardState::Failed { .. }));
        assert_eq!(launcher.launches(), 1);

        // Still broken: each retry of the same revision launches once.
        assert!(standard.apply_profile(profile, "r1").await.is_err());
        assert_eq!(launcher.launches(), 2);

        // Fixed: the retry starts it.
        launcher
            .fail
            .store(false, std::sync::atomic::Ordering::SeqCst);
        standard.apply_profile(profile, "r1").await.unwrap();
        assert!(matches!(standard.state(), StandardState::Ready { .. }));
        assert_eq!(launcher.launches(), 3);

        // A crash right after a retry is not restarted automatically.
        launcher.crash_last();
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(matches!(standard.state(), StandardState::Failed { .. }));
        assert_eq!(launcher.launches(), 3, "no restart storm");

        // The next retry gets exactly one more launch.
        standard.apply_profile(profile, "r1").await.unwrap();
        assert!(matches!(standard.state(), StandardState::Ready { .. }));
        assert_eq!(launcher.launches(), 4);

        // A new revision refills the budget: a crash is restarted again.
        standard
            .apply_profile(br#"{"revision":"r2"}"#, "r2")
            .await
            .unwrap();
        launcher.crash_last();
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(
            matches!(standard.state(), StandardState::Ready { ref revision } if revision == "r2")
        );
        assert_eq!(launcher.launches(), 5);

        standard.stop().await;
    }

    fn apply_bodies(core: &crate::core_ipc::tests::FakeCore) -> Vec<serde_json::Value> {
        core.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path == "/v1/apply-profile")
            .map(|(_, body)| body.clone())
            .collect()
    }

    #[tokio::test]
    async fn apply_pins_the_api_host_when_the_core_accepts_it() {
        let core = crate::core_ipc::tests::FakeCore::new(|_, _| {
            (Duration::ZERO, Ok(serde_json::json!({"applied": true})))
        });
        let launcher =
            CrashyLauncher::with_hosts(core.clone(), Some(vec!["api.example.com".into()]));
        let standard = StandardCore::with_launcher(launcher, Arc::new(|_| {}));
        standard
            .apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap();
        standard
            .apply_profile(br#"{"revision":"r2"}"#, "r2")
            .await
            .unwrap();
        let expected = |revision: &str| {
            serde_json::json!({
                "profile": {"revision": revision},
                "allowed_rule_set_hosts": ["api.example.com"],
                "routing_mode": "rules",
            })
        };
        assert_eq!(apply_bodies(&core), vec![expected("r1"), expected("r2")]);
        standard.stop().await;
    }

    #[tokio::test]
    async fn a_new_routing_mode_reapplies_the_same_revision() {
        let core = crate::core_ipc::tests::FakeCore::new(|_, _| {
            (Duration::ZERO, Ok(serde_json::json!({"applied": true})))
        });
        let launcher =
            CrashyLauncher::with_hosts(core.clone(), Some(vec!["api.example.com".into()]));
        let routing = RoutingModeCell::default();
        let standard = StandardCore::with_routing(launcher, Arc::new(|_| {}), routing.clone());
        // Nothing running yet: nothing to re-apply.
        standard.reapply().await.unwrap();
        assert!(apply_bodies(&core).is_empty());
        standard
            .apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap();
        routing.set(RoutingMode::Global);
        standard.reapply().await.unwrap();
        // Same revision, same mode: no call.
        standard.reapply().await.unwrap();
        standard
            .apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap();
        let modes: Vec<Value> = apply_bodies(&core)
            .into_iter()
            .map(|body| body["routing_mode"].clone())
            .collect();
        assert_eq!(
            modes,
            vec![serde_json::json!("rules"), serde_json::json!("global")]
        );
        standard.stop().await;
    }

    #[tokio::test]
    async fn apply_sends_the_bare_profile_to_an_older_core() {
        let core = crate::core_ipc::tests::FakeCore::new(|_, _| {
            (Duration::ZERO, Ok(serde_json::json!({"applied": true})))
        });
        let standard =
            StandardCore::with_launcher(CrashyLauncher::new(core.clone()), Arc::new(|_| {}));
        standard
            .apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap();
        assert_eq!(
            apply_bodies(&core),
            vec![serde_json::json!({"profile": {"revision": "r1"}})]
        );
        standard.stop().await;
    }

    #[test]
    fn the_process_launcher_pins_the_configured_api_host() {
        let mut config = test_config("/nonexistent", "/nonexistent");
        config.api_base = "https://api.example.com".into();
        assert_eq!(
            ProcessLauncher::new(&config).rule_set_hosts,
            vec!["api.example.com".to_string()]
        );
        config.api_base = "https://api.example.com:8443/api/v1".into();
        assert_eq!(
            ProcessLauncher::new(&config).rule_set_hosts,
            vec!["api.example.com:8443".to_string()]
        );
    }

    #[tokio::test]
    async fn missing_binary_reports_core_binary_missing() {
        let dir =
            std::env::temp_dir().join(format!("ppvpn-std-test-{}", uuid::Uuid::new_v4().simple()));
        let states = Arc::new(Mutex::new(Vec::new()));
        let sink = states.clone();
        let core = StandardCore::with_launcher(
            process_launcher(&test_config("/nonexistent/bin", dir.to_str().unwrap())),
            Arc::new(move |state| sink.lock().unwrap().push(state)),
        );
        let error = core
            .apply_profile(br#"{"revision":"r1"}"#, "r1")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ClientError::Failed {
                code: ErrorCode::CoreBinaryMissing,
                ..
            }
        ));
        assert!(matches!(core.state(), StandardState::Failed { .. }));
        assert!(matches!(
            states.lock().unwrap().first(),
            Some(StandardState::Starting)
        ));
        assert!(matches!(
            core.transport(),
            Err(ClientError::StandardNotReady)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn invalid_profile_json_is_rejected_before_spawning() {
        let core = StandardCore::with_launcher(
            process_launcher(&test_config("/nonexistent", "/nonexistent")),
            Arc::new(|_| {}),
        );
        let error = core.apply_profile(b"not json", "r1").await.unwrap_err();
        assert!(matches!(
            error,
            ClientError::Failed {
                code: ErrorCode::ProfileInvalid,
                ..
            }
        ));
        assert!(matches!(core.state(), StandardState::Stopped));
    }
}
