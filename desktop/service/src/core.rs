//! Privileged ppvpn-core lifecycle and authenticated Core API forwarding.
//!
//! One core instance at a time. Every instance shares the same socket,
//! session-secret and state paths, so two rules keep an old instance from
//! hurting its successor:
//!
//! - **Keyed to the instance.** The session secret is read once, when the
//!   instance first answers, and every later Core API call (including the
//!   `/v1/stop` sent while stopping) authenticates with that instance's own
//!   secret. A call meant for an older instance can never be accepted by a
//!   newer one.
//! - **Stopped means exited.** [`CoreManager::stop`] waits until the process
//!   has exited (and reaps it) before it returns. ppvpn-core removes the
//!   socket and secret paths on its way out (its listener unlinks the socket,
//!   and the secret file is removed when `serve` returns), and tears down its
//!   TUN routes; an instance still shutting down while the next one starts
//!   would delete the new instance's files and routes.
//!
//! Core API framing (see [`exchange_http`]): ppvpn-core is a Go `net/http`
//! server. With `Connection: close` it answers as soon as it has the request
//! head, never drains a body its handler did not read (`/v1/start`,
//! `/v1/get-status`, … ignore theirs), and closes the connection right after
//! the response. A request written in several pieces could therefore be
//! answered before its body arrived: writing the rest failed with EPIPE, and
//! closing with the body unread made Linux report ECONNRESET to our read
//! even though the whole response was already queued. The request is now one
//! write, and the response is read up to its length instead of to EOF.

use crate::logfile::RotatingLog;
use crate::protocol::{ConnectPayload, SessionRef, Status, UpdateProfilePayload};
use anyhow::{anyhow, Context, Result};
use log::info;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

pub static CORE: Lazy<Mutex<CoreManager>> = Lazy::new(|| {
    Mutex::new(CoreManager {
        watchers: crate::watch::HUB.clone(),
        ..CoreManager::default()
    })
});

/// Why a core stopped, as told to the session's watchers (`core_stopped`).
pub mod stop_reason {
    /// Its session disconnected.
    pub const DISCONNECTED: &str = "disconnected";
    /// Its lease was not renewed in time.
    pub const LEASE_EXPIRED: &str = "lease_expired";
    /// Another session of the same OS user took the connection over.
    pub const TAKEN_OVER: &str = "taken_over";
    /// A new core replaced it (a new Connect).
    pub const REPLACED: &str = "replaced";
    /// It failed to start.
    pub const START_FAILED: &str = "start_failed";
    /// A profile update and its rollback both failed.
    pub const PROFILE_FAILED: &str = "profile_failed";
    /// The process exited on its own (crashed, killed).
    pub const EXITED: &str = "exited";
    /// The service is shutting down.
    pub const SERVICE_STOPPING: &str = "service_stopping";
}
pub const LEASE_DURATION: Duration = Duration::from_secs(45);
/// How long a stopping core may take to exit before it is killed. ppvpn-core
/// gives its IPC server 5 s to shut down after the data plane has stopped.
const STOP_GRACE: Duration = Duration::from_secs(7);
/// How long a freshly spawned core may take to answer `/v1/get-version`.
/// The first run of a newly installed binary waits for the macOS code
/// assessment (syspolicyd) before any of its code runs: 5 s and more were
/// seen, and every retry killed at an 8 s budget paid it again.
const READY_TIMEOUT: Duration = Duration::from_secs(25);
/// How long the service waits at exit for its system DNS changes (launchd
/// kills it 30 s after SIGTERM).
const DNS_FLUSH_TIMEOUT: Duration = Duration::from_secs(15);
/// Largest Core API response accepted.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[cfg(target_os = "macos")]
const CORE_SOCKET: &str = "/Library/Application Support/PPVPN/core/core.sock";
#[cfg(target_os = "macos")]
const CORE_SECRET: &str = "/Library/Application Support/PPVPN/core/session.secret";
#[cfg(target_os = "macos")]
const CORE_STATE: &str = "/Library/Application Support/PPVPN/core/state";

#[cfg(windows)]
const CORE_SOCKET: &str = r"\\.\pipe\ppvpn-core";
#[cfg(windows)]
const CORE_SECRET: &str = r"C:\ProgramData\PPVPN\core\session.secret";
#[cfg(windows)]
const CORE_STATE: &str = r"C:\ProgramData\PPVPN\core\state";

// /run/ppvpn and /var/lib/ppvpn are created by the systemd unit
// (RuntimeDirectory= / StateDirectory=, see install.rs).
#[cfg(not(any(windows, target_os = "macos")))]
const CORE_SOCKET: &str = "/run/ppvpn/core/core.sock";
#[cfg(not(any(windows, target_os = "macos")))]
const CORE_SECRET: &str = "/run/ppvpn/core/session.secret";
#[cfg(not(any(windows, target_os = "macos")))]
const CORE_STATE: &str = "/var/lib/ppvpn/core/state";

/// Builds the command that launches a fake core (tests only).
#[cfg(test)]
type FakeCoreCommand = Box<dyn Fn(&CoreLayout) -> Command + Send>;

/// Where the privileged core lives and how it is launched.
struct CoreLayout {
    socket: String,
    secret: PathBuf,
    state: PathBuf,
    /// `None`: `ppvpn-core.log` in [`crate::logfile::log_dir`].
    log: Option<PathBuf>,
    /// How long a stopping core may take to exit before it is killed.
    stop_grace: Duration,
    /// Tests launch a fake core instead of the installed binary.
    #[cfg(test)]
    command: Option<FakeCoreCommand>,
}

impl Default for CoreLayout {
    fn default() -> Self {
        Self {
            socket: CORE_SOCKET.to_string(),
            secret: PathBuf::from(CORE_SECRET),
            state: PathBuf::from(CORE_STATE),
            log: None,
            stop_grace: STOP_GRACE,
            #[cfg(test)]
            command: None,
        }
    }
}

#[derive(Default)]
pub struct CoreManager {
    layout: CoreLayout,
    state: Option<RunningCore>,
    /// `ppvpn-core.log`, shared by every instance (opened on first use).
    core_log: Option<std::sync::Arc<Mutex<RotatingLog>>>,
    /// Watch connections, told when a session's core stops.
    watchers: std::sync::Arc<crate::watch::Hub>,
    /// macOS system DNS override (see [`crate::macdns`]); a no-op elsewhere.
    dns: crate::macdns::DnsWorker,
    /// The override was (or may have been) published and is still to be
    /// removed.
    dns_applied: bool,
}

struct RunningCore {
    pid: u32,
    /// Kept (never detached) so the process is reaped when it exits: a
    /// zombie would otherwise still look alive to a pid lookup.
    child: Child,
    /// This instance's Core API session secret; empty until it answered.
    secret: String,
    bin_path: String,
    session: SessionRef,
    /// OS user of the client that started this core (uid on Unix, user SID
    /// on Windows); `None` when it could not be resolved.
    owner_user: Option<String>,
    profile_revision: String,
    profile: Value,
    /// `allowed_rule_set_hosts` `profile` was applied with (empty when the
    /// core predates the field), reused for a rollback.
    rule_set_hosts: Vec<String>,
    /// `routing_mode` `profile` was applied with (`None`: the core predates
    /// it, or the client sent none), reused for a rollback.
    routing_mode: Option<String>,
    lease_deadline: Instant,
    /// Closing it makes the core exit (`--exit-on-stdin-close`).
    stdin: Option<std::process::ChildStdin>,
    #[cfg(windows)]
    _job: std::os::windows::io::OwnedHandle,
}

impl RunningCore {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

#[derive(Debug, Deserialize)]
struct CoreEnvelope {
    ok: bool,
    #[serde(default)]
    data: Value,
    error: Option<CoreError>,
}

#[derive(Debug, Deserialize)]
struct CoreError {
    code: String,
    message: String,
}

impl CoreManager {
    /// Service status for `client_pid`. Session details (id, generation,
    /// revision, lease) are shown only to the owner's OS user; anyone else
    /// learns only that the connection is in use.
    pub fn status(&mut self, client_pid: u32) -> Status {
        let caller_user = process_user(client_pid);
        self.status_for(caller_user.as_deref())
    }

    fn status_for(&mut self, caller_user: Option<&str>) -> Status {
        let live = self
            .state
            .as_mut()
            .is_some_and(|core| core.lease_deadline > Instant::now() && core.alive());
        let view = self.state.as_ref().filter(|_| live).map(|core| CoreView {
            pid: core.pid,
            bin_path: &core.bin_path,
            session: &core.session,
            owner_user: core.owner_user.as_deref(),
            profile_revision: &core.profile_revision,
            lease_remaining: core
                .lease_deadline
                .saturating_duration_since(Instant::now()),
        });
        describe(view, caller_user)
    }

    /// Start (or keep) the TUN core for `body.session`. `client_pid` is the
    /// authenticated caller, used for the same-user takeover check.
    pub fn connect(&mut self, body: ConnectPayload, client_pid: u32) -> Result<u32> {
        validate_session(&body.session)?;
        let caller_user = process_user(client_pid);
        let decision = match self.state.as_mut() {
            None => Ownership::Free,
            Some(current) => {
                let live = current.lease_deadline > Instant::now() && current.alive();
                ownership(
                    &Owner {
                        session: &current.session,
                        user: current.owner_user.as_deref(),
                        live,
                    },
                    &body.session,
                    body.take_over,
                    caller_user.as_deref(),
                )
            }
        };
        let taking_over = decision == Ownership::TakeOver;
        match decision {
            Ownership::Free => {}
            Ownership::Expired => {
                self.stop_because(stop_reason::LEASE_EXPIRED).ok();
            }
            Ownership::Same => {
                if let Some(current) = self.state.as_mut() {
                    current.lease_deadline = Instant::now() + LEASE_DURATION;
                    return Ok(current.pid);
                }
            }
            Ownership::TakeOver => {
                if let Some(current) = self.state.as_ref() {
                    info!(
                        "session {} (generation {}) takes over from session {} (generation {})",
                        body.session.session_id,
                        body.session.generation,
                        current.session.session_id,
                        current.session.generation
                    );
                }
                self.stop_because(stop_reason::TAKEN_OVER).ok();
            }
            Ownership::Refused(code) => return Err(anyhow!(code)),
        }
        let started = self.start_core(body, caller_user);
        if let Err(error) = &started {
            if taking_over {
                // Two TUN cores cannot coexist, so the previous session was
                // stopped first; now nobody is connected.
                log::error!(
                    "takeover: the new session's core failed after the previous session was stopped: {error:#}"
                );
            }
        }
        started
    }

    /// Spawns a new instance and brings it up. Any failure after the spawn
    /// stops the instance again, so a failed attempt leaves nothing behind.
    fn start_core(&mut self, body: ConnectPayload, caller_user: Option<String>) -> Result<u32> {
        let profile_revision = body
            .profile
            .get("revision")
            .and_then(Value::as_str)
            .filter(|revision| !revision.is_empty())
            .ok_or_else(|| anyhow!("PROFILE_REVISION_REQUIRED"))?
            .to_string();
        let requested_schema = body
            .profile
            .get("schema_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("PROFILE_SCHEMA_REQUIRED"))?;
        check_routing_mode(body.routing_mode.as_deref())?;
        // Whatever ran before must be gone: it shares our paths.
        self.stop_because(stop_reason::REPLACED).ok();
        // Rules a killed core left would misroute the new one's traffic.
        #[cfg(all(target_os = "linux", not(test)))]
        crate::netclean::clean_if_no_core(
            &crate::netclean::SystemRunner,
            core_process_running(),
            "before starting ppvpn-core",
        );
        self.spawn_core(&body, caller_user, profile_revision)?;
        let pid = self.state.as_ref().map(|core| core.pid).unwrap_or_default();
        let brought_up = self.bring_up(
            &body.profile,
            requested_schema,
            body.allowed_rule_set_hosts.clone(),
            body.routing_mode.clone(),
        );
        if let Err(error) = brought_up {
            log::error!("ppvpn-core pid {pid} failed to start: {error:#}");
            self.stop_because(stop_reason::START_FAILED).ok();
            return Err(error);
        }
        info!("ppvpn-core TUN started, pid {pid}");
        self.apply_dns();
        Ok(pid)
    }

    /// Points the macOS system resolver at the TUN. A failure is logged and
    /// the connection kept: the TUN still carries (and hijacks) every query
    /// to a resolver that is not on-link.
    fn apply_dns(&mut self) {
        self.dns_applied = true;
        self.dns.apply();
    }

    /// A core of ours was started (and not stopped since).
    #[cfg_attr(windows, allow(dead_code))]
    pub fn has_core(&self) -> bool {
        self.state.is_some()
    }

    /// At service start: removes the override a killed service left behind,
    /// or, while a core (an orphan of that service) still runs, keeps it
    /// until [`Self::reap_exited`] sees no core left. An override a core of
    /// ours publishes meanwhile is ours.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn clean_dns_leftover(&mut self, core_running: bool) {
        self.dns.clean_leftover(core_running);
    }

    /// Removes the macOS system DNS override if it is in place.
    fn restore_dns(&mut self) {
        if std::mem::take(&mut self.dns_applied) {
            self.dns.remove();
        }
    }

    /// Waits until the queued system DNS changes ran (service exit).
    pub fn flush_dns(&mut self) {
        if !self.dns.flush(DNS_FLUSH_TIMEOUT) {
            log::error!(
                "system DNS changes did not finish within {DNS_FLUSH_TIMEOUT:?}; exiting anyway"
            );
        }
    }

    fn spawn_core(
        &mut self,
        body: &ConnectPayload,
        caller_user: Option<String>,
        profile_revision: String,
    ) -> Result<()> {
        prepare_runtime_dir(&self.layout)?;
        let _ = std::fs::remove_file(&self.layout.secret);
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.layout.socket);

        let (mut cmd, bin_path) = self.core_command()?;
        // The core's output goes through the service, which caps the file
        // (see crate::logfile); without a log file it is discarded.
        let core_log = self.core_log();
        if core_log.is_some() {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
        cmd.stdin(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().context("failed to spawn ppvpn-core")?;
        let pid = child.id();
        if let Some(log) = core_log {
            if let Some(stdout) = child.stdout.take() {
                crate::logfile::forward_output(stdout, log.clone());
            }
            if let Some(stderr) = child.stderr.take() {
                crate::logfile::forward_output(stderr, log);
            }
        }
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("failed to retain ppvpn-core lifetime pipe"));
        };
        #[cfg(windows)]
        let job = match assign_kill_on_close_job(&child) {
            Ok(job) => job,
            Err(error) => {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        self.state = Some(RunningCore {
            pid,
            child,
            secret: String::new(),
            bin_path,
            session: body.session.clone(),
            owner_user: caller_user,
            profile_revision,
            profile: body.profile.clone(),
            rule_set_hosts: Vec::new(),
            routing_mode: None,
            lease_deadline: Instant::now() + LEASE_DURATION,
            stdin: Some(stdin),
            #[cfg(windows)]
            _job: job,
        });
        Ok(())
    }

    /// The shared core log, opened on first use; `None` when it cannot be
    /// opened (the core then runs without a log).
    fn core_log(&mut self) -> Option<std::sync::Arc<Mutex<RotatingLog>>> {
        if self.core_log.is_none() {
            let path = match self.layout.log.clone() {
                Some(path) => path,
                None => match crate::logfile::prepare_log_dir() {
                    Ok(dir) => dir.join(crate::logfile::CORE_LOG),
                    Err(error) => {
                        log::warn!("core log directory unavailable: {error}");
                        return None;
                    }
                },
            };
            match RotatingLog::open(&path, crate::logfile::MAX_BYTES, crate::logfile::KEEP_FILES) {
                Ok(log) => self.core_log = Some(std::sync::Arc::new(Mutex::new(log))),
                Err(error) => {
                    log::warn!("cannot open core log {}: {error}", path.display());
                    return None;
                }
            }
        }
        self.core_log.clone()
    }

    fn core_command(&self) -> Result<(Command, String)> {
        #[cfg(test)]
        if let Some(command) = &self.layout.command {
            return Ok((command(&self.layout), "fake-core".to_string()));
        }
        let bin_path = installed_core_binary()
            .ok_or_else(|| anyhow!("installed ppvpn-core binary is unavailable"))?;
        let platform = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "macos"
        };
        let mut cmd = Command::new(&bin_path);
        cmd.arg("serve")
            .arg("--socket")
            .arg(&self.layout.socket)
            .arg("--session-secret-file")
            .arg(&self.layout.secret)
            .arg("--state-dir")
            .arg(&self.layout.state)
            .args([
                "--platform",
                platform,
                // Local proxies come from the unprivileged standard core;
                // this core only provides TUN.
                "--local-proxy=false",
                "--tun",
                "--tun-stack=mixed",
                "--exit-on-stdin-close",
            ]);
        // macOS: the override points the system resolver at the TUN; the
        // core (0.5.20+) resolves direct domains with the physical default
        // interface's servers itself, re-read on every interface change, so
        // nothing is passed (a fixed list went stale on a Wi-Fi switch).
        if let Some(flag) = debug_log_flag(&self.layout) {
            // One line per routed connection, with the domain: for
            // troubleshooting only, never left on.
            log::warn!(
                "{} exists: ppvpn-core logs at debug level (includes visited domains)",
                flag.display()
            );
            cmd.args(["--log-level", "debug"]);
        }
        Ok((cmd, bin_path.to_string_lossy().to_string()))
    }

    /// Waits for the new instance, then checks the schema, applies the
    /// profile and starts the data plane.
    fn bring_up(
        &mut self,
        profile: &Value,
        requested_schema: u64,
        requested_hosts: Vec<String>,
        requested_mode: Option<String>,
    ) -> Result<()> {
        // Per-phase durations at info level: a slow first connect after an
        // install (binary first launch, TUN creation, rule-set downloads)
        // shows which step took the time.
        let mut phase = Instant::now();
        let mut lap = |name: &str| {
            info!(
                "ppvpn-core start: {name} took {} ms",
                phase.elapsed().as_millis()
            );
            phase = Instant::now();
        };
        let ready = self.wait_until_ready();
        lap("spawn until ready");
        ready?;
        let version = self.call_api_unchecked("/v1/get-version", serde_json::json!({}))?;
        let supported_schema = version
            .get("profile_schema_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("CORE_SCHEMA_CAPABILITY_MISSING"))?;
        if requested_schema != supported_schema {
            return Err(anyhow!("PROFILE_SCHEMA_UNSUPPORTED"));
        }
        let hosts = pinned_hosts(&version, requested_hosts);
        let mode = routing_mode_for(&version, requested_mode);
        lap("get-version");
        let applied = self.call_api_unchecked(
            "/v1/apply-profile",
            apply_body(profile, &hosts, mode.as_deref()),
        );
        lap("apply-profile");
        applied?;
        if let Some(core) = self.state.as_mut() {
            core.rule_set_hosts = hosts;
            core.routing_mode = mode;
        }
        let started = self.call_api_unchecked("/v1/start", serde_json::json!({}));
        lap("start (TUN, routes)");
        started?;
        Ok(())
    }

    /// Waits until the new instance published its secret and answers with
    /// it, and binds that secret to the instance.
    fn wait_until_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut last = None;
        loop {
            let Some(core) = self.state.as_mut() else {
                return Err(anyhow!("ppvpn-core is not running"));
            };
            if !core.alive() {
                return Err(anyhow!("ppvpn-core exited during startup"));
            }
            if let Ok(secret) = std::fs::read_to_string(&self.layout.secret) {
                let secret = secret.trim();
                if !secret.is_empty() {
                    match send_http(
                        &self.layout.socket,
                        "/v1/get-version",
                        secret,
                        b"{}",
                        Duration::from_secs(2),
                    ) {
                        Ok(response) if response.ok => {
                            core.secret = secret.to_string();
                            return Ok(());
                        }
                        Ok(response) => last = Some(envelope_error(response)),
                        Err(error) => last = Some(error),
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(last.unwrap_or_else(|| anyhow!("ppvpn-core startup timed out")));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn renew_lease(&mut self, session: &SessionRef, client_pid: u32) -> Result<Status> {
        let caller_user = process_user(client_pid);
        let core = self.authorize(session, caller_user.as_deref())?;
        core.lease_deadline = Instant::now() + LEASE_DURATION;
        Ok(self.status_for(caller_user.as_deref()))
    }

    pub fn disconnect(&mut self, session: &SessionRef, client_pid: u32) -> Result<()> {
        let caller_user = process_user(client_pid);
        self.authorize(session, caller_user.as_deref())?;
        self.stop_because(stop_reason::DISCONNECTED)
    }

    /// Registers a watch connection for the owner's session (see
    /// [`crate::watch`]). Under `self` only for the check and the
    /// registration: a stop can then never slip in between unnoticed.
    pub fn watch(
        &mut self,
        session: &SessionRef,
        client_pid: u32,
        closer: Option<crate::watch::Closer>,
    ) -> Result<crate::watch::Registration> {
        let caller_user = process_user(client_pid);
        self.authorize(session, caller_user.as_deref())?;
        self.watchers
            .register(session.clone(), closer)
            .map_err(|code| anyhow!(code))
    }

    #[cfg(all(test, unix))]
    fn lease_deadline_for_test(&mut self, remaining: Duration) {
        if let Some(core) = self.state.as_mut() {
            core.lease_deadline = Instant::now() + remaining;
        }
    }

    pub fn expire_lease(&mut self) {
        let expired = self
            .state
            .as_ref()
            .is_some_and(|core| core.lease_deadline <= Instant::now());
        if expired {
            info!("connection lease expired; stopping privileged core");
            self.stop_because(stop_reason::LEASE_EXPIRED).ok();
        }
    }

    #[cfg(all(test, unix))]
    pub fn watchers(&self) -> std::sync::Arc<crate::watch::Hub> {
        self.watchers.clone()
    }

    /// A core that exited on its own (crashed, killed) is reaped and its
    /// session's watchers are told right away, instead of when the client
    /// next renews its lease.
    pub fn reap_exited(&mut self) {
        let exited = self
            .state
            .as_mut()
            .and_then(|core| match core.child.try_wait() {
                Ok(Some(status)) => Some((core.pid, status.to_string())),
                Ok(None) => None,
                Err(error) => Some((core.pid, format!("unknown status ({error})"))),
            });
        if let Some((pid, status)) = exited {
            log::warn!("ppvpn-core pid {pid} exited on its own: {status}");
            self.stop_because(stop_reason::EXITED).ok();
        } else if self.dns_applied && self.state.is_none() {
            // No core left to take the override down with it.
            log::warn!("no ppvpn-core running; restoring the system DNS");
            self.restore_dns();
        }
        if self.state.is_none() {
            self.dns.check_orphan(core_process_running);
        }
    }

    /// One Core API call for the owner's session. Holds `self` for the
    /// whole exchange; the IPC path uses [`call_api`], which does not.
    #[cfg(all(test, unix))]
    pub fn call_api(
        &mut self,
        session: &SessionRef,
        path: &str,
        body: Value,
        client_pid: u32,
    ) -> Result<Value> {
        self.prepare_call(session, path, client_pid)?.send(body)
    }

    /// Authorizes a Core API call for the owner's session (renewing its
    /// lease) and returns what it needs, bound to the current instance.
    fn prepare_call(
        &mut self,
        session: &SessionRef,
        path: &str,
        client_pid: u32,
    ) -> Result<CoreCall> {
        let caller_user = process_user(client_pid);
        let core = self.authorize(session, caller_user.as_deref())?;
        core.lease_deadline = Instant::now() + LEASE_DURATION;
        self.target(path)
    }

    /// A Core API call to the current instance, with its own secret.
    fn target(&mut self, path: &str) -> Result<CoreCall> {
        if !ALLOWED_PATHS.contains(&path) {
            return Err(anyhow!("Core API path is not allowed"));
        }
        let core = self
            .state
            .as_mut()
            .filter(|core| !core.secret.is_empty())
            .ok_or_else(|| anyhow!("ppvpn-core is not running"))?;
        if !core.alive() {
            return Err(anyhow!("ppvpn-core is not running"));
        }
        Ok(CoreCall {
            socket: self.layout.socket.clone(),
            secret: core.secret.clone(),
            pid: core.pid,
            path: path.to_string(),
        })
    }

    /// One Core API call to the current instance while holding `self`
    /// (bring-up only: nothing else can use the instance yet).
    fn call_api_unchecked(&mut self, path: &str, body: Value) -> Result<Value> {
        self.target(path)?.send(body)
    }

    /// The caller must run as the owner's OS user *and* present the owner's
    /// session: a session id alone never grants access.
    fn authorize(
        &mut self,
        session: &SessionRef,
        caller_user: Option<&str>,
    ) -> Result<&mut RunningCore> {
        validate_session(session)?;
        let core = self
            .state
            .as_mut()
            .ok_or_else(|| anyhow!("CONNECTION_NOT_ACTIVE"))?;
        access(
            &core.session,
            core.owner_user.as_deref(),
            session,
            caller_user,
        )
        .map_err(|code| anyhow!(code))?;
        if core.lease_deadline <= Instant::now() || !core.alive() {
            return Err(anyhow!("CONNECTION_LEASE_EXPIRED"));
        }
        Ok(core)
    }

    /// Stops the current instance and returns once its process has exited:
    /// `/v1/stop` (routes are removed while the core is still healthy), then
    /// SIGTERM and closing its lifetime pipe, then a kill after
    /// the grace period ([`STOP_GRACE`]). Only then are the shared paths cleared for the next
    /// instance.
    #[cfg(all(test, unix))]
    pub fn stop(&mut self) -> Result<()> {
        self.stop_because("stopped")
    }

    /// [`Self::stop`], telling the session's watchers `reason` (see
    /// [`stop_reason`]) before the core is asked to stop.
    pub fn stop_because(&mut self, reason: &str) -> Result<()> {
        if let Some(mut core) = self.state.take() {
            self.watchers.core_stopped(&core.session, reason);
            // Queued first: the system DNS goes back while the TUN still
            // answers, not seconds after it is gone (scutil can be slow).
            self.restore_dns();
            let started = Instant::now();
            if core.alive() {
                if !core.secret.is_empty() {
                    let stopped = send_http(
                        &self.layout.socket,
                        "/v1/stop",
                        &core.secret,
                        b"{}",
                        STOP_REQUEST_TIMEOUT,
                    );
                    info!(
                        "ppvpn-core pid {} /v1/stop answered in {} ms ({})",
                        core.pid,
                        started.elapsed().as_millis(),
                        match &stopped {
                            Ok(response) if response.ok => "ok".to_string(),
                            Ok(_) => "error".to_string(),
                            Err(error) => format!("{error:#}"),
                        }
                    );
                }
                info!("stopping ppvpn-core pid {}", core.pid);
                terminate(&core.child);
            }
            drop(core.stdin.take());
            let grace = self.layout.stop_grace;
            if !wait_for_exit(&mut core.child, grace) {
                log::warn!(
                    "ppvpn-core pid {} did not exit within {:?}; killing it",
                    core.pid,
                    grace
                );
                let _ = core.child.kill();
                let _ = core.child.wait();
            }
            info!(
                "ppvpn-core pid {} stopped in {} ms",
                core.pid,
                started.elapsed().as_millis()
            );
            // A killed core (on its own, or after the grace period) could
            // not remove its policy rules; left in place they break the next
            // connection's direct path. Runs under the core lock: `ip` is
            // quick, and the time is logged.
            #[cfg(all(target_os = "linux", not(test)))]
            {
                let cleaning = Instant::now();
                crate::netclean::clean_if_no_core(
                    &crate::netclean::SystemRunner,
                    core_process_running(),
                    "after ppvpn-core stopped",
                );
                info!(
                    "routing cleanup after ppvpn-core stopped took {} ms",
                    cleaning.elapsed().as_millis()
                );
            }
        }
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.layout.socket);
        let _ = std::fs::remove_file(&self.layout.secret);
        self.restore_dns();
        Ok(())
    }
}

/// The first steps of the service's shutdown, once `ipc::begin_stopping`
/// refuses new work: every watch connection is told `stopping` and closed
/// (within about `grace`, whatever its client does), and only then is the
/// core lock taken for the core's stop, which the caller does with the
/// returned guard.
pub fn stop_sequence<'a>(
    core: &'a Mutex<CoreManager>,
    watchers: &crate::watch::Hub,
    grace: Duration,
) -> parking_lot::MutexGuard<'a, CoreManager> {
    watchers.close_all(grace);
    core.lock()
}

/// Core API paths a client may call (the service also uses these itself).
const ALLOWED_PATHS: &[&str] = &[
    "/v1/get-version",
    "/v1/validate-profile",
    "/v1/apply-profile",
    "/v1/start",
    "/v1/stop",
    "/v1/get-status",
    "/v1/list-nodes",
    "/v1/select-node",
    "/v1/get-selected-node",
    // Core 0.5.7+: a node fixed to one ingress (live, no engine rebuild).
    "/v1/pin-ingress",
    "/v1/probe-entrances",
    "/v1/probe-availability",
    "/v1/get-local-proxy-metadata",
    "/v1/get-local-proxy-credential",
    "/v1/get-traffic",
    "/v1/get-connections",
];

/// Upper bound for the whole exchange of one Core API call. Status-type
/// calls are answered from memory; probes run for the `timeout_ms` the
/// client asked for (per ingress, up to two waves, as the client's own
/// deadline in crates/ppvpn-client/src/core_ipc.rs).
fn call_timeout(path: &str, body: &Value) -> Duration {
    match path {
        "/v1/get-version" | "/v1/get-status" | "/v1/get-traffic" | "/v1/get-selected-node" => {
            Duration::from_secs(2)
        }
        "/v1/list-nodes"
        | "/v1/select-node"
        | "/v1/get-connections"
        | "/v1/get-local-proxy-metadata"
        | "/v1/get-local-proxy-credential" => Duration::from_secs(5),
        "/v1/probe-entrances" | "/v1/probe-availability" => {
            let asked = body
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(10_000)
                .min(60_000);
            Duration::from_millis(asked.saturating_mul(2)) + Duration::from_secs(5)
        }
        "/v1/stop" => STOP_REQUEST_TIMEOUT,
        // validate / apply profile, start.
        _ => Duration::from_secs(30),
    }
}

/// How long `/v1/stop` may take before the core is stopped by force.
const STOP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// One Core API call, bound to the instance it was prepared for: it
/// authenticates with that instance's secret, so it can never reach a later
/// one. Sent without holding [`CORE`], so a slow call never delays
/// Disconnect, a lease renewal or a status query.
#[derive(Clone)]
struct CoreCall {
    socket: String,
    secret: String,
    pid: u32,
    path: String,
}

impl CoreCall {
    fn send(&self, body: Value) -> Result<Value> {
        let timeout = call_timeout(&self.path, &body);
        let request_body = serde_json::to_vec(&body)?;
        let response = send_http(
            &self.socket,
            &self.path,
            &self.secret,
            &request_body,
            timeout,
        )?;
        if response.ok {
            return Ok(response.data);
        }
        Err(envelope_error(response))
    }
}

/// `CoreApi`: authorizes under `core`, then forwards the call without it.
pub fn call_api(
    core: &Mutex<CoreManager>,
    session: &SessionRef,
    path: &str,
    body: Value,
    client_pid: u32,
) -> Result<Value> {
    let call = core.lock().prepare_call(session, path, client_pid)?;
    let data = call.send(body)?;
    // The data plane of the same instance was started or stopped without
    // Connect / Disconnect: the system DNS follows it.
    if path == "/v1/start" || path == "/v1/stop" {
        let mut manager = core.lock();
        if manager
            .state
            .as_ref()
            .is_some_and(|current| current.pid == call.pid)
        {
            if path == "/v1/start" {
                manager.apply_dns();
            } else {
                manager.restore_dns();
            }
        }
    }
    Ok(data)
}

/// A privileged ppvpn-core is running: the installed binary, run by root
/// (ours, or an orphan of an earlier service instance). The app's standard
/// core has the same name but runs as the user, and never counts: it runs
/// whenever the app does.
#[cfg_attr(windows, allow(dead_code))]
pub fn core_process_running() -> bool {
    let Some(binary) = installed_core_binary() else {
        return false;
    };
    let started = Instant::now();
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::new()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet),
    );
    let running = system.processes().values().any(|process| {
        process
            .exe()
            .is_some_and(|exe| is_core_binary(exe, &binary))
            && runs_as_root(process)
    });
    info!(
        "privileged ppvpn-core running: {running} (checked in {} ms)",
        started.elapsed().as_millis()
    );
    running
}

/// `exe` is `binary`, also once replaced by an upgrade (Linux shows the old
/// inode as "… (deleted)").
fn is_core_binary(exe: &Path, binary: &Path) -> bool {
    let exe = exe.to_string_lossy();
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
    Path::new(exe) == binary
}

#[cfg(unix)]
fn runs_as_root(process: &sysinfo::Process) -> bool {
    process.user_id().is_some_and(|uid| **uid == 0)
}

#[cfg(not(unix))]
fn runs_as_root(_: &sysinfo::Process) -> bool {
    true
}

/// Runs the installed core once (`ppvpn-core version`) so the first launch
/// of a newly installed binary pays the macOS code assessment here, at
/// service start, and not inside the first Connect's startup budget.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn warm_up_core_binary() {
    let Some(binary) = installed_core_binary() else {
        return;
    };
    let started = Instant::now();
    let result = Command::new(&binary)
        .arg("version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match result {
        Ok(status) => info!(
            "ppvpn-core warm-up run ({status}) took {} ms",
            started.elapsed().as_millis()
        ),
        Err(error) => log::warn!("ppvpn-core warm-up run failed: {error}"),
    }
}

/// `UpdateProfile`: checks the schema and applies the new profile (rolling
/// back on failure) without holding `core` during the Core API calls, then
/// records the new revision if the same instance is still the session's.
pub fn update_profile(
    core: &Mutex<CoreManager>,
    body: UpdateProfilePayload,
    client_pid: u32,
) -> Result<Status> {
    let caller_user = process_user(client_pid);
    let revision = body
        .profile
        .get("revision")
        .and_then(Value::as_str)
        .filter(|revision| !revision.is_empty())
        .ok_or_else(|| anyhow!("PROFILE_REVISION_REQUIRED"))?
        .to_string();
    let requested_schema = body
        .profile
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("PROFILE_SCHEMA_REQUIRED"))?;
    check_routing_mode(body.routing_mode.as_deref())?;
    let (call, previous, previous_hosts, previous_mode) = {
        let mut manager = core.lock();
        let current = manager.authorize(&body.session, caller_user.as_deref())?;
        let previous = current.profile.clone();
        let previous_hosts = current.rule_set_hosts.clone();
        let previous_mode = current.routing_mode.clone();
        (
            manager.target("/v1/get-version")?,
            previous,
            previous_hosts,
            previous_mode,
        )
    };
    let on = |path: &str| CoreCall {
        path: path.to_string(),
        ..call.clone()
    };

    let version = on("/v1/get-version").send(serde_json::json!({}))?;
    let supported_schema = version
        .get("profile_schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("CORE_SCHEMA_CAPABILITY_MISSING"))?;
    if requested_schema != supported_schema {
        return Err(anyhow!("PROFILE_SCHEMA_UNSUPPORTED"));
    }
    let hosts = pinned_hosts(&version, body.allowed_rule_set_hosts.clone());
    let mode = routing_mode_for(&version, body.routing_mode.clone());
    // ApplyProfile is already transactional while the core is running: it
    // replaces the live runtime and rolls the previous one back if the
    // candidate cannot start. Calling Reload afterwards would restart the
    // successfully applied revision a second time.
    let apply = on("/v1/apply-profile").send(apply_body(&body.profile, &hosts, mode.as_deref()));
    if let Err(error) = apply {
        // Re-apply the previous revision once to cover an ambiguous local
        // IPC failure where the core committed the candidate but the
        // response was lost. If ApplyProfile itself rejected the
        // candidate, this is an idempotent no-op.
        let rollback = on("/v1/apply-profile").send(apply_body(
            &previous,
            &previous_hosts,
            previous_mode.as_deref(),
        ));
        return match rollback {
            Ok(_) => Err(anyhow!("PROFILE_UPDATE_ROLLED_BACK: {error}")),
            Err(rollback_error) => {
                let mut manager = core.lock();
                if manager
                    .state
                    .as_ref()
                    .is_some_and(|current| current.pid == call.pid)
                {
                    manager.stop_because(stop_reason::PROFILE_FAILED).ok();
                }
                Err(anyhow!(
                    "PROFILE_UPDATE_AND_ROLLBACK_FAILED: {error}; {rollback_error}"
                ))
            }
        };
    }
    let mut manager = core.lock();
    let current = manager.authorize(&body.session, caller_user.as_deref())?;
    if current.pid != call.pid {
        return Err(anyhow!("CONNECTION_NOT_ACTIVE"));
    }
    current.profile = body.profile;
    current.rule_set_hosts = hosts;
    current.routing_mode = mode;
    current.profile_revision = revision;
    current.lease_deadline = Instant::now() + LEASE_DURATION;
    Ok(manager.status_for(caller_user.as_deref()))
}

/// Asks the core to shut down gracefully: SIGTERM on Unix. Windows has no
/// equivalent; closing the lifetime pipe does it there.
#[cfg(unix)]
fn terminate(child: &Child) {
    // SAFETY: plain kill(2). The child has not been reaped (we own its
    // `Child`), so its pid cannot have been reused.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate(_: &Child) {}

/// Polls until `child` has exited (reaping it); `false` on timeout.
fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => return false,
            // Not our child any more (cannot happen for a `Child` we own).
            Err(_) => return true,
        }
    }
}

fn envelope_error(response: CoreEnvelope) -> anyhow::Error {
    let error = response
        .error
        .map(|e| format!("{}: {}", e.code, e.message))
        .unwrap_or_else(|| "core operation failed".to_string());
    anyhow!(error)
}

/// First core release whose `apply-profile` accepts `allowed_rule_set_hosts`;
/// older cores decode strictly and reject the unknown field.
const RULE_SET_HOSTS_MIN_CORE: (u64, u64, u64) = (0, 5, 0);

/// `core_version` (from `GetVersion`) accepts `allowed_rule_set_hosts`. A
/// pre-release suffix counts as that release. Mirrors
/// `ppvpn-client` `core_ipc::core_accepts_rule_set_hosts`.
fn core_accepts_rule_set_hosts(core_version: &str) -> bool {
    core_version_at_least(core_version, RULE_SET_HOSTS_MIN_CORE)
}

/// First core release whose `apply-profile` accepts `routing_mode`.
const ROUTING_MODE_MIN_CORE: (u64, u64, u64) = (0, 5, 6);

/// `core_version` is `minimum` or newer (a pre-release counts as that
/// release; anything unparsable does not).
fn core_version_at_least(core_version: &str, minimum: (u64, u64, u64)) -> bool {
    let core = core_version.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let (Some(Some(major)), Some(Some(minor)), patch) = (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let patch = match patch {
        None => 0,
        Some(Some(patch)) => patch,
        Some(None) => return false,
    };
    (major, minor, patch) >= minimum
}

/// Rejects a `routing_mode` other than `rules` / `global` before anything
/// starts.
fn check_routing_mode(requested: Option<&str>) -> Result<()> {
    match requested {
        None | Some("rules" | "global") => Ok(()),
        Some(_) => Err(anyhow!("ROUTING_MODE_INVALID")),
    }
}

/// The client's routing mode when the core accepts it, otherwise none (the
/// core then routes by the profile's rules, as before the mode existed).
fn routing_mode_for(version: &Value, requested: Option<String>) -> Option<String> {
    let core_version = version
        .get("core_version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let requested = requested?;
    if core_version_at_least(core_version, ROUTING_MODE_MIN_CORE) {
        return Some(requested);
    }
    info!("ppvpn-core {core_version} predates routing_mode; applying the profile's rules");
    None
}

/// The client's hosts when the core (its `GetVersion` data) accepts them,
/// otherwise none: the profile then applies without rule-set downloads.
fn pinned_hosts(version: &Value, requested: Vec<String>) -> Vec<String> {
    let core_version = version
        .get("core_version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if requested.is_empty() || core_accepts_rule_set_hosts(core_version) {
        return requested;
    }
    info!("ppvpn-core {core_version} predates allowed_rule_set_hosts; rule sets stay unpinned");
    Vec::new()
}

/// `apply-profile` request body.
fn apply_body(
    profile: &Value,
    allowed_rule_set_hosts: &[String],
    routing_mode: Option<&str>,
) -> Value {
    let mut body = serde_json::json!({ "profile": profile });
    if !allowed_rule_set_hosts.is_empty() {
        body["allowed_rule_set_hosts"] = serde_json::json!(allowed_rule_set_hosts);
    }
    if let Some(mode) = routing_mode {
        body["routing_mode"] = serde_json::json!(mode);
    }
    body
}

fn validate_session(session: &SessionRef) -> Result<()> {
    if session.session_id.is_empty() || uuid::Uuid::parse_str(&session.session_id).is_err() {
        return Err(anyhow!("INVALID_SESSION_ID"));
    }
    if session.generation == 0 {
        return Err(anyhow!("INVALID_GENERATION"));
    }
    Ok(())
}

/// One Core API exchange that gives up after `timeout` in total, whatever
/// the core does. It runs on its own thread: a named pipe has no read
/// timeout, and a Unix socket's applies per read. A thread left behind ends
/// when the core answers, closes the connection or exits (Unix: at the
/// latest one second after `timeout` without data).
fn send_http(
    socket: &str,
    path: &str,
    secret: &str,
    body: &[u8],
    timeout: Duration,
) -> Result<CoreEnvelope> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let (socket, owned_path, secret, body) = (
        socket.to_string(),
        path.to_string(),
        secret.to_string(),
        body.to_vec(),
    );
    std::thread::Builder::new()
        .name("core-api".to_string())
        .spawn(move || {
            let _ = sender.send(open_and_exchange(
                &socket,
                &owned_path,
                &secret,
                &body,
                timeout,
            ));
        })
        .context("spawn Core API thread")?;
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(anyhow!(
            "CORE_IPC_TIMEOUT: {path}: no answer within {timeout:?}"
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(anyhow!("CORE_IPC_FAILED: {path}: exchange thread ended"))
        }
    }
}

#[cfg(unix)]
fn open_and_exchange(
    socket: &str,
    path: &str,
    secret: &str,
    body: &[u8],
    timeout: Duration,
) -> Result<CoreEnvelope> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket).context("connect privileged core socket")?;
    // Only a backstop that ends the thread: `send_http` enforces `timeout`.
    stream
        .set_read_timeout(Some(timeout + Duration::from_secs(1)))
        .ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
    exchange_http(&mut stream, path, secret, body)
}

#[cfg(windows)]
fn open_and_exchange(
    socket: &str,
    path: &str,
    secret: &str,
    body: &[u8],
    _timeout: Duration,
) -> Result<CoreEnvelope> {
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(socket)
        .context("open privileged core named pipe")?;
    exchange_http(&mut pipe, path, secret, body)
}

/// One Core API request/response on a fresh connection.
///
/// The request goes out in a single write. ppvpn-core may answer (and close)
/// before it read a body its handler ignores, so a failed write is not
/// final: the answer can already be waiting. The response is read up to its
/// `Content-Length` (or last chunk), not to EOF, so a reset that follows a
/// complete response (Linux reports ECONNRESET when the core closes with our
/// body unread) cannot discard it.
fn exchange_http<T: Read + Write>(
    stream: &mut T,
    path: &str,
    secret: &str,
    body: &[u8],
) -> Result<CoreEnvelope> {
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {secret}\r\n\
         X-Core-API-Version: 1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);
    let written = stream.write_all(&request).and_then(|()| stream.flush());
    let response = match (read_http_response(stream), written) {
        (Ok(response), _) => response,
        (Err(read_error), Err(write_error)) => {
            return Err(anyhow!(
                "CORE_IPC_FAILED: {path}: write request: {write_error}; read response: {read_error}"
            ))
        }
        (Err(read_error), Ok(())) => {
            return Err(anyhow!(
                "CORE_IPC_FAILED: {path}: read response: {read_error}"
            ))
        }
    };
    serde_json::from_slice(&response)
        .map_err(|error| anyhow!("CORE_IPC_FAILED: {path}: decode Core API envelope: {error}"))
}

/// Reads one HTTP response and returns its (de-chunked) body.
fn read_http_response<R: Read>(reader: &mut R) -> std::io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => {
                return match http_body(&raw, true) {
                    Ok(Some(body)) => Ok(body),
                    Ok(None) => Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        format!("connection closed after {} response bytes", raw.len()),
                    )),
                    Err(error) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                }
            }
            Ok(read) => {
                raw.extend_from_slice(&chunk[..read]);
                if raw.len() > MAX_RESPONSE_BYTES {
                    return Err(std::io::Error::other("response too large"));
                }
                match http_body(&raw, false) {
                    Ok(Some(body)) => return Ok(body),
                    Ok(None) => {}
                    Err(error) => {
                        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// The body of the response in `raw` once it is complete; `Ok(None)` while
/// more bytes are needed. `eof`: the connection has closed, which completes a
/// response delimited by the close.
fn http_body(raw: &[u8], eof: bool) -> std::result::Result<Option<Vec<u8>>, String> {
    let Some(split) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| "invalid HTTP head".to_string())?;
    let mut lines = head.split("\r\n");
    if !lines.next().unwrap_or_default().starts_with("HTTP/1.") {
        return Err("invalid HTTP status line".to_string());
    }
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "invalid Content-Length".to_string())?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
    }
    let rest = &raw[split + 4..];
    if chunked {
        // Go chunks any body larger than its 2 KiB buffer, even when the
        // request says `Connection: close`.
        return decode_chunked(rest);
    }
    match content_length {
        Some(length) if rest.len() >= length => Ok(Some(rest[..length].to_vec())),
        Some(_) => Ok(None),
        None if eof => Ok(Some(rest.to_vec())),
        None => Ok(None),
    }
}

/// Decodes a chunked body; `Ok(None)` while it is still incomplete.
fn decode_chunked(mut data: &[u8]) -> std::result::Result<Option<Vec<u8>>, String> {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = data.windows(2).position(|window| window == b"\r\n") else {
            return Ok(None);
        };
        let size_text =
            std::str::from_utf8(&data[..line_end]).map_err(|_| "bad chunk size".to_string())?;
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size =
            usize::from_str_radix(size_text, 16).map_err(|_| "bad chunk size".to_string())?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(Some(body));
        }
        if data.len() < size.saturating_add(2) {
            return Ok(None);
        }
        body.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

/// `debug-log` next to the core's state directory (only an administrator
/// can create it there) turns on ppvpn-core's debug log for the next
/// connection.
fn debug_log_flag(layout: &CoreLayout) -> Option<PathBuf> {
    let flag = layout.state.parent()?.join("debug-log");
    flag.is_file().then_some(flag)
}

fn installed_core_binary() -> Option<PathBuf> {
    let dir = service_exe_dir()?;
    let candidates = if cfg!(windows) {
        vec![dir.join("ppvpn-core.exe")]
    } else {
        vec![dir.join("ppvpn-core")]
    };
    candidates.into_iter().find(|path| path.is_file())
}

fn prepare_runtime_dir(layout: &CoreLayout) -> Result<()> {
    let secret_parent = layout
        .secret
        .parent()
        .ok_or_else(|| anyhow!("invalid core secret path"))?;
    std::fs::create_dir_all(secret_parent)?;
    std::fs::create_dir_all(&layout.state)?;
    harden_runtime_permissions(secret_parent)?;
    Ok(())
}

#[cfg(windows)]
fn harden_runtime_permissions(path: &Path) -> Result<()> {
    let status = Command::new("icacls.exe")
        .arg(path)
        .args([
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-18:(OI)(CI)F",
            "*S-1-5-32-544:(OI)(CI)F",
        ])
        .status()
        .context("apply private core runtime ACL")?;
    if !status.success() {
        return Err(anyhow!(
            "CORE_STATE_ACL_FAILED:{}",
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn harden_runtime_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

fn service_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// The core's current owner, as seen by a new `Connect`.
struct Owner<'a> {
    session: &'a SessionRef,
    user: Option<&'a str>,
    /// Lease valid and core process alive.
    live: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Ownership {
    /// No core yet.
    Free,
    /// The previous owner's lease lapsed or its core died: replace it.
    Expired,
    /// The caller already owns the core: renew and reuse it.
    Same,
    /// Same OS user asked to take over: stop the other session's core.
    TakeOver,
    Refused(&'static str),
}

/// A live core, as `GetStatus` may describe it.
struct CoreView<'a> {
    pid: u32,
    bin_path: &'a str,
    session: &'a SessionRef,
    owner_user: Option<&'a str>,
    profile_revision: &'a str,
    lease_remaining: Duration,
}

/// `GetStatus` for `caller_user`: session details only for the owner's user.
fn describe(core: Option<CoreView<'_>>, caller_user: Option<&str>) -> Status {
    let idle = Status {
        running: false,
        pid: -1,
        bin_path: String::new(),
        mode: String::new(),
        service_version: env!("CARGO_PKG_VERSION").to_string(),
        service_build_id: crate::protocol::SERVICE_BUILD_ID.to_string(),
        session_id: None,
        generation: 0,
        profile_revision: None,
        lease_expires_at_ms: None,
    };
    let Some(core) = core else {
        return idle;
    };
    if !same_user(core.owner_user, caller_user) {
        // In use by someone else: no session, no pid, nothing to replay.
        return Status {
            running: true,
            mode: "transparent".to_string(),
            ..idle
        };
    }
    Status {
        running: true,
        pid: core.pid as i64,
        bin_path: core.bin_path.to_string(),
        mode: "transparent".to_string(),
        session_id: Some(core.session.session_id.clone()),
        generation: core.session.generation,
        profile_revision: Some(core.profile_revision.to_string()),
        lease_expires_at_ms: Some(
            crate::protocol::now_epoch()
                .saturating_mul(1000)
                .saturating_add(core.lease_remaining.as_millis() as u64),
        ),
        ..idle
    }
}

/// Both identities known and equal. An unresolvable user matches nobody.
fn same_user(owner: Option<&str>, caller: Option<&str>) -> bool {
    matches!((owner, caller), (Some(owner), Some(caller)) if owner == caller)
}

/// Access to an existing session: same OS user first, then same session.
fn access(
    owner_session: &SessionRef,
    owner_user: Option<&str>,
    session: &SessionRef,
    caller_user: Option<&str>,
) -> std::result::Result<(), &'static str> {
    if !same_user(owner_user, caller_user) {
        return Err("CONNECTION_OWNED_BY_ANOTHER_USER");
    }
    if owner_session != session {
        return Err("STALE_OR_FOREIGN_SESSION");
    }
    Ok(())
}

fn ownership(
    current: &Owner<'_>,
    requested: &SessionRef,
    take_over: bool,
    caller_user: Option<&str>,
) -> Ownership {
    if !current.live {
        return Ownership::Expired;
    }
    if !same_user(current.user, caller_user) {
        // Never offered, never allowed: another OS user's connection.
        return Ownership::Refused("CONNECTION_OWNED_BY_ANOTHER_USER");
    }
    if current.session == requested {
        return Ownership::Same;
    }
    if take_over {
        Ownership::TakeOver
    } else {
        Ownership::Refused("CONNECTION_OWNED_BY_ANOTHER_SESSION")
    }
}

/// OS user of `pid`: uid on Unix, user SID on Windows.
fn process_user(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::new().with_user(sysinfo::UpdateKind::Always),
    );
    sys.process(Pid::from_u32(pid))
        .and_then(|process| process.user_id())
        .map(|user| user.to_string())
}

#[cfg(windows)]
fn assign_kill_on_close_job(
    child: &std::process::Child,
) -> Result<std::os::windows::io::OwnedHandle> {
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::null_mut;
    use winapi::shared::ntdef::HANDLE;
    use winapi::um::jobapi2::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    };
    use winapi::um::winnt::{
        JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    // The job object is a last-resort crash boundary. Normal Stop/Shutdown
    // still calls /v1/stop first so the core can remove routes gracefully.
    let raw_job = unsafe { CreateJobObjectW(null_mut(), null_mut()) };
    if raw_job.is_null() {
        return Err(std::io::Error::last_os_error()).context("create core job object");
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw_job.cast()) };
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            raw_job,
            JobObjectExtendedLimitInformation,
            (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if configured == 0 {
        return Err(std::io::Error::last_os_error()).context("configure core job object");
    }
    let assigned = unsafe { AssignProcessToJobObject(raw_job, child.as_raw_handle() as HANDLE) };
    if assigned == 0 {
        return Err(std::io::Error::last_os_error()).context("assign core process to job object");
    }
    Ok(job)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn session(id: &str, generation: u64) -> SessionRef {
        SessionRef {
            session_id: id.to_string(),
            generation,
        }
    }

    fn owner<'a>(session: &'a SessionRef, user: Option<&'a str>, live: bool) -> Owner<'a> {
        Owner {
            session,
            user,
            live,
        }
    }

    #[test]
    fn same_user_can_take_over() {
        let current = session("a", 1);
        let caller = session("b", 7);
        assert_eq!(
            ownership(
                &owner(&current, Some("501"), true),
                &caller,
                true,
                Some("501")
            ),
            Ownership::TakeOver
        );
        // Without the flag the same user is told it is another session.
        assert_eq!(
            ownership(
                &owner(&current, Some("501"), true),
                &caller,
                false,
                Some("501")
            ),
            Ownership::Refused("CONNECTION_OWNED_BY_ANOTHER_SESSION")
        );
        assert_eq!(
            ownership(
                &owner(&current, Some("501"), true),
                &current,
                false,
                Some("501")
            ),
            Ownership::Same
        );
    }

    #[test]
    fn another_user_is_refused_even_with_the_owners_session() {
        let current = session("a", 1);
        let caller = session("b", 7);
        for take_over in [false, true] {
            assert_eq!(
                ownership(
                    &owner(&current, Some("501"), true),
                    &caller,
                    take_over,
                    Some("502")
                ),
                Ownership::Refused("CONNECTION_OWNED_BY_ANOTHER_USER")
            );
        }
        // Knowing the session id does not make another user the owner.
        assert_eq!(
            ownership(
                &owner(&current, Some("501"), true),
                &current,
                false,
                Some("502")
            ),
            Ownership::Refused("CONNECTION_OWNED_BY_ANOTHER_USER")
        );
        // Unresolvable identities match nobody.
        assert_eq!(
            ownership(&owner(&current, None, true), &caller, true, Some("501")),
            Ownership::Refused("CONNECTION_OWNED_BY_ANOTHER_USER")
        );
        assert_eq!(
            ownership(&owner(&current, Some("501"), true), &caller, true, None),
            Ownership::Refused("CONNECTION_OWNED_BY_ANOTHER_USER")
        );
        // An expired owner is replaced whoever asks.
        assert_eq!(
            ownership(
                &owner(&current, Some("501"), false),
                &caller,
                false,
                Some("502")
            ),
            Ownership::Expired
        );
    }

    #[test]
    fn session_calls_need_the_owners_user_and_session() {
        let owned = session("a", 1);
        assert_eq!(access(&owned, Some("501"), &owned, Some("501")), Ok(()));
        assert_eq!(
            access(&owned, Some("501"), &owned, Some("502")),
            Err("CONNECTION_OWNED_BY_ANOTHER_USER")
        );
        assert_eq!(
            access(&owned, Some("501"), &session("b", 2), Some("501")),
            Err("STALE_OR_FOREIGN_SESSION")
        );
        assert_eq!(
            access(&owned, None, &owned, None),
            Err("CONNECTION_OWNED_BY_ANOTHER_USER")
        );
    }

    #[test]
    fn take_over_is_required_in_the_payload() {
        let without: Result<ConnectPayload, _> = serde_json::from_value(serde_json::json!({
            "session_id": "a", "generation": 1, "profile": {}
        }));
        assert!(without.is_err());
        let with: ConnectPayload = serde_json::from_value(serde_json::json!({
            "session_id": "a", "generation": 1, "profile": {}, "take_over": true
        }))
        .unwrap();
        assert!(with.take_over);
    }

    #[test]
    fn rule_set_hosts_are_optional_in_the_payloads() {
        let old: ConnectPayload = serde_json::from_value(serde_json::json!({
            "session_id": "a", "generation": 1, "profile": {}, "take_over": false
        }))
        .unwrap();
        assert!(old.allowed_rule_set_hosts.is_empty());
        let new: ConnectPayload = serde_json::from_value(serde_json::json!({
            "session_id": "a", "generation": 1, "profile": {}, "take_over": false,
            "allowed_rule_set_hosts": ["api.dev.example.com"]
        }))
        .unwrap();
        assert_eq!(new.allowed_rule_set_hosts, ["api.dev.example.com"]);
        let update: UpdateProfilePayload = serde_json::from_value(serde_json::json!({
            "session_id": "a", "generation": 1, "profile": {},
            "allowed_rule_set_hosts": ["api.example.com:8443"]
        }))
        .unwrap();
        assert_eq!(update.allowed_rule_set_hosts, ["api.example.com:8443"]);
    }

    #[test]
    fn apply_body_pins_hosts_only_for_cores_that_accept_them() {
        let hosts = || vec!["api.dev.example.com".to_string()];
        let v = |core: &str| serde_json::json!({ "core_version": core });
        assert_eq!(pinned_hosts(&v("0.5.0"), hosts()), hosts());
        assert_eq!(pinned_hosts(&v("0.5.0-rc.1"), hosts()), hosts());
        assert_eq!(pinned_hosts(&v("1.2"), hosts()), hosts());
        assert!(pinned_hosts(&v("0.4.5"), hosts()).is_empty());
        assert!(pinned_hosts(&serde_json::json!({}), hosts()).is_empty());
        assert!(pinned_hosts(&v("0.5.0"), Vec::new()).is_empty());

        let profile = serde_json::json!({ "revision": "r1" });
        assert_eq!(
            apply_body(&profile, &hosts(), None),
            serde_json::json!({ "profile": { "revision": "r1" },
                "allowed_rule_set_hosts": ["api.dev.example.com"] })
        );
        assert_eq!(
            apply_body(&profile, &[], None),
            serde_json::json!({ "profile": { "revision": "r1" } })
        );
    }

    #[test]
    fn the_routing_mode_goes_only_to_cores_that_accept_it() {
        let v = |core: &str| serde_json::json!({ "core_version": core });
        let global = || Some("global".to_string());
        assert_eq!(routing_mode_for(&v("0.5.6"), global()), global());
        assert_eq!(routing_mode_for(&v("0.6.0-rc.1"), global()), global());
        assert_eq!(routing_mode_for(&v("0.5.5"), global()), None);
        assert_eq!(routing_mode_for(&v("0.5.6"), None), None);
        assert!(check_routing_mode(None).is_ok());
        assert!(check_routing_mode(Some("rules")).is_ok());
        assert!(check_routing_mode(Some("global")).is_ok());
        assert_eq!(
            check_routing_mode(Some("direct")).unwrap_err().to_string(),
            "ROUTING_MODE_INVALID"
        );
        let profile = serde_json::json!({ "revision": "r1" });
        assert_eq!(
            apply_body(&profile, &[], Some("global")),
            serde_json::json!({ "profile": { "revision": "r1" }, "routing_mode": "global" })
        );
        let old: UpdateProfilePayload = serde_json::from_value(serde_json::json!({
            "session_id": "a", "generation": 1, "profile": {}
        }))
        .unwrap();
        assert_eq!(old.routing_mode, None);
        let new: UpdateProfilePayload = serde_json::from_value(serde_json::json!({
            "session_id": "a", "generation": 1, "profile": {}, "routing_mode": "global"
        }))
        .unwrap();
        assert_eq!(new.routing_mode.as_deref(), Some("global"));
    }

    #[test]
    fn status_hides_the_session_from_other_users() {
        let owned = session("a", 3);
        let view = || CoreView {
            pid: 42,
            bin_path: "/x/ppvpn-core",
            session: &owned,
            owner_user: Some("501"),
            profile_revision: "r1",
            lease_remaining: Duration::from_secs(30),
        };
        let own = describe(Some(view()), Some("501"));
        assert!(own.running);
        assert_eq!(own.session_id.as_deref(), Some("a"));
        assert_eq!(own.generation, 3);
        assert_eq!(own.pid, 42);

        let other = describe(Some(view()), Some("502"));
        assert!(other.running);
        assert_eq!(other.session_id, None);
        assert_eq!(other.generation, 0);
        assert_eq!(other.pid, -1);
        assert_eq!(other.profile_revision, None);
        assert_eq!(other.lease_expires_at_ms, None);

        let unknown = describe(Some(view()), None);
        assert_eq!(unknown.session_id, None);
        assert!(!describe(None, Some("501")).running);

        // Every status names this build, idle, owned or foreign.
        for status in [own, other, unknown, describe(None, Some("501"))] {
            assert_eq!(status.service_build_id, crate::protocol::SERVICE_BUILD_ID);
        }
    }

    #[test]
    fn build_id_is_a_sha256_hex_digest() {
        let id = crate::protocol::SERVICE_BUILD_ID;
        assert_eq!(id.len(), 64, "{id}");
        assert!(id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    // --- Core API framing ---------------------------------------------------

    /// A connection to ppvpn-core as Go's `net/http` treats it, worst case:
    /// the request head is answered as soon as it is complete, the body of a
    /// handler that ignores it is never read, and the connection is closed
    /// right after the response. Writes after that fail with EPIPE; once the
    /// response has been read, the close reports ECONNRESET when our body
    /// was left unread (Linux), instead of EOF.
    struct GoLikeCore {
        received: Vec<u8>,
        answered: bool,
        body_unread: bool,
        response: std::io::Cursor<Vec<u8>>,
    }

    impl GoLikeCore {
        fn new(response: &str) -> Self {
            Self {
                received: Vec::new(),
                answered: false,
                body_unread: false,
                response: std::io::Cursor::new(response.as_bytes().to_vec()),
            }
        }
    }

    impl Write for GoLikeCore {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.answered {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.received.extend_from_slice(buf);
            if let Some(end) = self.received.windows(4).position(|w| w == b"\r\n\r\n") {
                self.answered = true;
                self.body_unread = self.received.len() > end + 4;
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Read for GoLikeCore {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.answered {
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            // Hand the response out in small pieces, like a socket might.
            let limit = buf.len().min(7);
            match self.response.read(&mut buf[..limit])? {
                0 if self.body_unread => Err(std::io::ErrorKind::ConnectionReset.into()),
                read => Ok(read),
            }
        }
    }

    const OK_RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
        Content-Length: 32\r\nConnection: close\r\n\r\n{\"ok\":true,\"data\":{\"state\":\"x\"}}";

    #[test]
    fn a_response_before_the_body_was_read_still_counts() {
        // `/v1/start` succeeded in the core, which closed without reading
        // `{}`: the service used to report EPIPE / ECONNRESET and tear the
        // freshly started core down.
        let mut core = GoLikeCore::new(OK_RESPONSE);
        let envelope = exchange_http(&mut core, "/v1/start", "s", b"{}").unwrap();
        assert!(envelope.ok);
        assert_eq!(envelope.data["state"], "x");
        assert!(core.body_unread, "the fake left the body unread");
        let request = String::from_utf8(core.received).unwrap();
        assert!(request.starts_with("POST /v1/start HTTP/1.1\r\n"));
        assert!(request.contains("Authorization: Bearer s\r\n"));
        assert!(request.ends_with("Content-Length: 2\r\nConnection: close\r\n\r\n{}"));
    }

    #[test]
    fn chunked_and_close_delimited_responses_are_read() {
        let body = "{\"ok\":true,\"data\":[1,2,3]}";
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            10,
            &body[..10],
            body.len() - 10,
            &body[10..]
        );
        let envelope =
            exchange_http(&mut GoLikeCore::new(&chunked), "/v1/list-nodes", "s", b"{}").unwrap();
        assert_eq!(envelope.data, serde_json::json!([1, 2, 3]));

        let until_eof = format!("HTTP/1.1 200 OK\r\n\r\n{body}");
        let mut reader = std::io::Cursor::new(until_eof.into_bytes());
        assert_eq!(read_http_response(&mut reader).unwrap(), body.as_bytes());
    }

    #[test]
    fn a_truncated_response_is_an_error() {
        let truncated = "HTTP/1.1 200 OK\r\nContent-Length: 32\r\n\r\n{\"ok\":true";
        let mut reader = std::io::Cursor::new(truncated.as_bytes().to_vec());
        assert_eq!(
            read_http_response(&mut reader).unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        let error = exchange_http(&mut GoLikeCore::new(truncated), "/v1/start", "s", b"{}")
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("CORE_IPC_FAILED: /v1/start: read response"),
            "{error}"
        );
    }

    // --- lifecycle against a fake core process --------------------------------

    /// Re-run of this test binary acting as ppvpn-core (see `fake_core`);
    /// a no-op in a normal test run.
    #[cfg(unix)]
    #[test]
    fn fake_core_process() {
        if let Ok(socket) = std::env::var("PPVPN_FAKE_CORE_SOCKET") {
            fake_core::run(&socket);
        }
    }

    /// A stand-in for `ppvpn-core serve --exit-on-stdin-close`: same paths,
    /// same secret handshake, the same Go-like connection handling as
    /// [`GoLikeCore`] on a real socket, and the same shutdown (on SIGTERM or
    /// stdin EOF: finish, then remove the socket and secret paths).
    #[cfg(unix)]
    mod fake_core {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        static STOP: AtomicBool = AtomicBool::new(false);

        extern "C" fn on_term(_: libc::c_int) {
            STOP.store(true, Ordering::SeqCst);
        }

        fn env(name: &str) -> String {
            std::env::var(name).unwrap_or_default()
        }

        fn event(line: &str) {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(env("PPVPN_FAKE_CORE_EVENTS"))
                .unwrap();
            writeln!(file, "{} {line}", env("PPVPN_FAKE_CORE_INSTANCE")).unwrap();
        }

        pub(super) fn run(socket: &str) -> ! {
            // SAFETY: installs an async-signal-safe handler (one atomic store).
            unsafe {
                libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
            }
            let secret_path = env("PPVPN_FAKE_CORE_SECRET");
            let instance = env("PPVPN_FAKE_CORE_INSTANCE");
            let secret = format!("secret-of-instance-{instance}-{}", uuid::Uuid::new_v4());
            let staging = format!("{secret_path}.tmp");
            std::fs::write(&staging, &secret).unwrap();
            std::fs::rename(&staging, &secret_path).unwrap();
            let _ = std::fs::remove_file(socket);
            let listener = UnixListener::bind(socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            std::thread::spawn(|| {
                let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
                STOP.store(true, Ordering::SeqCst);
            });
            event("ready");
            while !STOP.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let secret = secret.clone();
                        std::thread::spawn(move || serve(stream, &secret));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            event("stopping");
            // The data plane and the IPC server take a while to shut down;
            // only then do the listener and `serve` remove their paths.
            let linger = env("PPVPN_FAKE_CORE_LINGER_MS").parse().unwrap_or(0);
            std::thread::sleep(Duration::from_millis(linger));
            if env("PPVPN_FAKE_CORE_HANG") == "1" {
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            drop(listener);
            let _ = std::fs::remove_file(socket);
            let _ = std::fs::remove_file(&secret_path);
            event("exited");
            std::process::exit(0)
        }

        fn serve(mut stream: UnixStream, secret: &str) {
            stream.set_nonblocking(false).unwrap();
            // Read the head byte by byte: never more than the head.
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => return,
                }
            }
            let head = String::from_utf8_lossy(&head).to_string();
            let path = head.split(' ').nth(1).unwrap_or_default().to_string();
            let header = |name: &str| {
                head.lines()
                    .find_map(|line| line.strip_prefix(name))
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            };
            let authorized = header("Authorization:") == format!("Bearer {secret}");
            if authorized && path == "/v1/apply-profile" {
                // Handlers that decode the request read their body.
                let length = header("Content-Length:").parse().unwrap_or(0);
                let mut body = vec![0u8; length];
                let _ = stream.read_exact(&mut body);
            }
            let instance = env("PPVPN_FAKE_CORE_INSTANCE");
            if authorized && path == env("PPVPN_FAKE_CORE_SLOW_PATH") {
                event(&format!("{path} begun"));
                let delay = env("PPVPN_FAKE_CORE_SLOW_MS").parse().unwrap_or(0);
                std::thread::sleep(Duration::from_millis(delay));
            }
            let data = match path.as_str() {
                _ if !authorized => None,
                "/v1/get-version" => Some(serde_json::json!({
                    "core_version": "fake", "profile_schema_version": 1
                })),
                "/v1/start" if env("PPVPN_FAKE_CORE_FAIL_START") == "1" => None,
                "/v1/get-status" => Some(serde_json::json!({
                    "state": "running", "instance": instance
                })),
                _ => Some(serde_json::json!({})),
            };
            event(&format!(
                "{path}{}",
                if authorized { "" } else { " unauthenticated" }
            ));
            let envelope = match data {
                Some(data) => serde_json::json!({ "ok": true, "data": data }),
                None if !authorized => serde_json::json!({
                    "ok": false,
                    "error": { "code": "UNAUTHENTICATED", "message": "valid session authentication is required" }
                }),
                None => serde_json::json!({
                    "ok": false,
                    "error": { "code": "CORE_OPERATION_FAILED", "message": "core operation failed" }
                }),
            };
            let body = envelope.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            // Closed right away, whatever of the request was left unread.
        }
    }

    /// How each fake core instance behaves, by launch order.
    #[cfg(unix)]
    #[derive(Clone, Copy, Default)]
    pub(crate) struct FakePlan {
        fail_start: bool,
        linger_ms: u64,
        hang: bool,
        /// This Core API path answers only after `slow_ms`.
        slow_path: &'static str,
        slow_ms: u64,
    }

    #[cfg(unix)]
    pub(crate) struct FakeCores {
        dir: PathBuf,
        events: PathBuf,
    }

    #[cfg(unix)]
    impl Drop for FakeCores {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[cfg(unix)]
    impl FakeCores {
        fn events(&self) -> Vec<String> {
            std::fs::read_to_string(&self.events)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    /// A manager whose cores are fake-core processes in a private directory
    /// (short path: Unix socket paths are limited to ~100 bytes).
    #[cfg(unix)]
    pub(crate) fn fake_manager(
        plans: Vec<FakePlan>,
        stop_grace: Duration,
    ) -> (CoreManager, FakeCores) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!(
            "pc-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let events = dir.join("events");
        let launched = AtomicUsize::new(0);
        let events_path = events.clone();
        let layout = CoreLayout {
            socket: dir.join("c.sock").to_string_lossy().to_string(),
            secret: dir.join("run").join("session.secret"),
            state: dir.join("state"),
            log: Some(dir.join("core.log")),
            stop_grace,
            command: Some(Box::new(move |layout: &CoreLayout| {
                let index = launched.fetch_add(1, Ordering::SeqCst);
                let plan = plans.get(index).copied().unwrap_or_default();
                let mut command = Command::new(std::env::current_exe().unwrap());
                command
                    .args([
                        "--exact",
                        "core::tests::fake_core_process",
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env("PPVPN_FAKE_CORE_SOCKET", &layout.socket)
                    .env("PPVPN_FAKE_CORE_SECRET", &layout.secret)
                    .env("PPVPN_FAKE_CORE_EVENTS", &events_path)
                    .env("PPVPN_FAKE_CORE_INSTANCE", (index + 1).to_string())
                    .env("PPVPN_FAKE_CORE_LINGER_MS", plan.linger_ms.to_string())
                    .env(
                        "PPVPN_FAKE_CORE_FAIL_START",
                        if plan.fail_start { "1" } else { "0" },
                    )
                    .env("PPVPN_FAKE_CORE_HANG", if plan.hang { "1" } else { "0" })
                    .env("PPVPN_FAKE_CORE_SLOW_PATH", plan.slow_path)
                    .env("PPVPN_FAKE_CORE_SLOW_MS", plan.slow_ms.to_string());
                command
            })),
        };
        let manager = CoreManager {
            layout,
            state: None,
            core_log: None,
            watchers: Default::default(),
            dns: crate::macdns::DnsWorker::default(),
            dns_applied: false,
        };
        (manager, FakeCores { dir, events })
    }

    /// The system DNS override follows the core: published only once its
    /// TUN started, removed when it stops, is stopped over the Core API, or
    /// dies on its own.
    #[cfg(unix)]
    #[test]
    fn the_system_dns_override_follows_the_tun_core() {
        use crate::macdns::{DnsWorker, FakeSystem, TunDns};
        let failing = FakePlan {
            fail_start: true,
            ..FakePlan::default()
        };
        let (mut manager, _cores) = fake_manager(
            vec![
                failing,
                FakePlan::default(),
                FakePlan::default(),
                FakePlan::default(),
            ],
            Duration::from_secs(5),
        );
        let system = FakeSystem::default();
        manager.dns = DnsWorker::inline(TunDns::new(Box::new(system.clone()), true));
        let published = || system.store.lock().unwrap().is_some();
        let me = std::process::id();

        let first = new_session(1);
        assert!(manager.connect(connect_payload(&first), me).is_err());
        assert!(!published(), "a core that failed to start gets no DNS");
        assert!(!system
            .calls()
            .iter()
            .any(|call| call.contains("set State:")));

        let second = new_session(2);
        let pid = manager.connect(connect_payload(&second), me).unwrap();
        assert!(published(), "published once the TUN started");
        manager.disconnect(&second, me).unwrap();
        assert!(!process_exists(pid));
        assert!(!published(), "removed on disconnect");

        let third = new_session(3);
        let pid = manager.connect(connect_payload(&third), me).unwrap();
        assert!(published());
        manager.lease_deadline_for_test(Duration::from_secs(30));
        // SAFETY: plain kill(2) of our own child.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while manager.state.as_mut().is_some_and(RunningCore::alive) {
            assert!(Instant::now() < deadline, "the killed core never exited");
            std::thread::sleep(Duration::from_millis(20));
        }
        manager.reap_exited();
        assert!(
            !published(),
            "a dead core's override is removed by the watchdog"
        );
        manager.stop().unwrap();
        assert!(!published());

        // The data plane stopped / started again over the Core API.
        let fourth = new_session(4);
        manager.connect(connect_payload(&fourth), me).unwrap();
        let core = Mutex::new(manager);
        let empty = || serde_json::json!({});
        call_api(&core, &fourth, "/v1/stop", empty(), me).unwrap();
        assert!(!published(), "removed with the data plane");
        call_api(&core, &fourth, "/v1/start", empty(), me).unwrap();
        assert!(published(), "back with the data plane");
        core.lock().stop().unwrap();
        assert!(!published());
    }

    #[test]
    fn a_debug_log_flag_file_turns_on_the_core_debug_log() {
        let dir = std::env::temp_dir().join(format!(
            "pc-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let layout = CoreLayout {
            state: dir.join("state"),
            ..CoreLayout::default()
        };
        assert_eq!(debug_log_flag(&layout), None);
        std::fs::write(dir.join("debug-log"), "").unwrap();
        assert_eq!(debug_log_flag(&layout), Some(dir.join("debug-log")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_installed_core_binary_counts() {
        let binary = Path::new("/opt/ppvpn/ppvpn-core");
        assert!(is_core_binary(Path::new("/opt/ppvpn/ppvpn-core"), binary));
        assert!(is_core_binary(
            Path::new("/opt/ppvpn/ppvpn-core (deleted)"),
            binary
        ));
        // The app's standard core: same name, another path.
        assert!(!is_core_binary(
            Path::new("/Applications/PPVPN.app/Contents/MacOS/ppvpn-core"),
            binary
        ));
    }

    /// An override left by a killed service while its core was still
    /// shutting down goes once that core is gone, not only at startup.
    #[cfg(unix)]
    #[test]
    fn an_orphaned_dns_override_is_removed_once_no_core_runs() {
        use crate::macdns::{apply_script, DnsWorker, FakeSystem, TunDns};
        let (mut manager, _cores) = fake_manager(vec![FakePlan::default()], Duration::from_secs(5));
        let system = FakeSystem::default();
        *system.store.lock().unwrap() = Some(apply_script(true));
        manager.dns = DnsWorker::inline(TunDns::new(Box::new(system.clone()), true));
        let published = || system.store.lock().unwrap().is_some();

        manager.clean_dns_leftover(true);
        assert!(published(), "kept while the orphan still runs");
        manager.dns.check_orphan(|| true);
        assert!(published());
        manager.dns.check_orphan(|| false);
        assert!(!published(), "removed once no core runs");

        // A core of ours that publishes its own takes the entry over.
        *system.store.lock().unwrap() = Some(apply_script(true));
        manager.clean_dns_leftover(true);
        let me = std::process::id();
        let session = new_session(1);
        manager.connect(connect_payload(&session), me).unwrap();
        manager.dns.check_orphan(|| false);
        assert!(published(), "ours now");
        manager.stop().unwrap();
        assert!(!published());

        // Nothing running at startup: removed right away.
        *system.store.lock().unwrap() = Some(apply_script(true));
        manager.clean_dns_leftover(false);
        assert!(!published());
    }

    #[cfg(unix)]
    pub(crate) fn connect_payload(session: &SessionRef) -> ConnectPayload {
        ConnectPayload {
            session: session.clone(),
            profile: serde_json::json!({ "revision": "r1", "schema_version": 1 }),
            take_over: false,
            // The fake core reports "fake" as its version: not pinned.
            allowed_rule_set_hosts: vec!["api.dev.example.com".into()],
            routing_mode: Some("rules".into()),
        }
    }

    #[cfg(unix)]
    pub(crate) fn new_session(generation: u64) -> SessionRef {
        session(&uuid::Uuid::new_v4().to_string(), generation)
    }

    #[cfg(unix)]
    pub(crate) fn process_exists(pid: u32) -> bool {
        // SAFETY: signal 0 only checks that the pid exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[cfg(unix)]
    #[test]
    fn retry_after_a_failed_start_gets_a_core_that_stays_up() {
        // The failed instance takes 300 ms to shut down and then removes the
        // shared socket and secret paths, as ppvpn-core does.
        let failing = FakePlan {
            fail_start: true,
            linger_ms: 300,
            ..FakePlan::default()
        };
        let (mut manager, cores) =
            fake_manager(vec![failing, FakePlan::default()], Duration::from_secs(5));
        let me = std::process::id();
        let first = new_session(1);
        let error = manager
            .connect(connect_payload(&first), me)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "CORE_OPERATION_FAILED: core operation failed");
        assert!(manager.state.is_none(), "the failed instance is gone");
        assert!(cores.events().contains(&"1 exited".to_string()));

        let retry = new_session(2);
        let pid = manager.connect(connect_payload(&retry), me).unwrap();
        // Longer than the failed instance's shutdown: had it still been
        // running, it would have removed the new instance's paths by now.
        std::thread::sleep(Duration::from_millis(600));
        let status = manager
            .call_api(&retry, "/v1/get-status", serde_json::json!({}), me)
            .unwrap();
        assert_eq!(status["instance"], "2", "calls reach the new instance");
        assert!(process_exists(pid));

        // The failed attempt's session can neither use nor stop it.
        assert_eq!(
            manager
                .call_api(&first, "/v1/get-status", serde_json::json!({}), me)
                .unwrap_err()
                .to_string(),
            "STALE_OR_FOREIGN_SESSION"
        );
        assert!(manager.disconnect(&first, me).is_err());
        assert!(manager.state.as_mut().is_some_and(RunningCore::alive));
        let events = cores.events();
        assert!(!events.iter().any(|line| line.starts_with("2 /v1/stop")));
        assert!(!events.iter().any(|line| line.contains("unauthenticated")));

        manager.disconnect(&retry, me).unwrap();
        assert!(!process_exists(pid), "stopped and reaped");
        let events = cores.events();
        assert!(events.contains(&"2 /v1/stop".to_string()));
        assert!(events.contains(&"2 exited".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn a_restarted_service_starts_over_whatever_the_previous_one_left() {
        let (mut manager, cores) = fake_manager(vec![FakePlan::default()], Duration::from_secs(5));
        // What a previous service process may leave: its core's socket, still
        // bound by an orphaned listener, and that core's secret.
        std::fs::create_dir_all(manager.layout.secret.parent().unwrap()).unwrap();
        std::fs::write(
            &manager.layout.secret,
            "secret-of-a-core-that-is-gone-000000",
        )
        .unwrap();
        let orphan = std::os::unix::net::UnixListener::bind(&manager.layout.socket).unwrap();
        orphan.set_nonblocking(true).unwrap();

        let me = std::process::id();
        let session = new_session(9);
        manager.connect(connect_payload(&session), me).unwrap();
        let status = manager
            .call_api(&session, "/v1/get-status", serde_json::json!({}), me)
            .unwrap();
        assert_eq!(status["instance"], "1");
        assert!(orphan.accept().is_err(), "nothing went to the old socket");
        manager.stop().unwrap();
        assert!(cores.events().contains(&"1 exited".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn a_core_that_does_not_exit_is_killed_before_stop_returns() {
        let hanging = FakePlan {
            hang: true,
            ..FakePlan::default()
        };
        let (mut manager, cores) = fake_manager(vec![hanging], Duration::from_millis(300));
        let me = std::process::id();
        let session = new_session(1);
        let pid = manager.connect(connect_payload(&session), me).unwrap();
        let started = Instant::now();
        manager.stop().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!process_exists(pid), "killed and reaped");
        assert!(cores.events().contains(&"1 stopping".to_string()));
        assert!(!Path::new(&manager.layout.socket).exists());
        assert!(!manager.layout.secret.exists());
    }

    #[cfg(unix)]
    fn wait_for_event(cores: &FakeCores, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cores.events().iter().any(|event| event == line) {
            assert!(
                Instant::now() < deadline,
                "no {line:?} in {:?}",
                cores.events()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn disconnect_does_not_wait_for_a_slow_core_call() {
        // A speed test in flight when the user turns enhanced mode off.
        let slow = FakePlan {
            slow_path: "/v1/probe-entrances",
            slow_ms: 20_000,
            ..FakePlan::default()
        };
        let (manager, cores) = fake_manager(vec![slow], Duration::from_secs(5));
        let core = Mutex::new(manager);
        let me = std::process::id();
        let session = new_session(1);
        let pid = core.lock().connect(connect_payload(&session), me).unwrap();

        std::thread::scope(|scope| {
            let probe = scope.spawn(|| {
                call_api(
                    &core,
                    &session,
                    "/v1/probe-entrances",
                    serde_json::json!({ "node_id": "n1", "timeout_ms": 10_000 }),
                    me,
                )
            });
            wait_for_event(&cores, "1 /v1/probe-entrances begun");

            // Status and lease renewal are not held up either.
            let started = Instant::now();
            assert!(core.lock().renew_lease(&session, me).is_ok());
            assert!(core.lock().status(me).running);
            core.lock().disconnect(&session, me).unwrap();
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "disconnect took {:?}",
                started.elapsed()
            );
            assert!(cores.events().contains(&"1 /v1/stop".to_string()));
            assert!(!process_exists(pid));
            // The probe fails with its core gone instead of hanging.
            assert!(probe.join().unwrap().is_err());
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_core_call_gives_up_after_its_timeout() {
        let slow = FakePlan {
            slow_path: "/v1/get-status",
            slow_ms: 10_000,
            ..FakePlan::default()
        };
        let (manager, _cores) = fake_manager(vec![slow], Duration::from_secs(1));
        let core = Mutex::new(manager);
        let me = std::process::id();
        let session = new_session(1);
        core.lock().connect(connect_payload(&session), me).unwrap();
        let started = Instant::now();
        let error = call_api(&core, &session, "/v1/get-status", serde_json::json!({}), me)
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("CORE_IPC_TIMEOUT: /v1/get-status"),
            "{error}"
        );
        let took = started.elapsed();
        assert!(
            took >= Duration::from_secs(2) && took < Duration::from_secs(3),
            "{took:?}"
        );
        core.lock().stop().unwrap();
    }

    #[test]
    fn core_call_timeouts_are_bounded() {
        let none = serde_json::json!({});
        assert_eq!(
            call_timeout("/v1/get-status", &none),
            Duration::from_secs(2)
        );
        assert_eq!(
            call_timeout("/v1/get-traffic", &none),
            Duration::from_secs(2)
        );
        assert_eq!(
            call_timeout("/v1/list-nodes", &none),
            Duration::from_secs(5)
        );
        assert_eq!(
            call_timeout("/v1/apply-profile", &none),
            Duration::from_secs(30)
        );
        assert_eq!(
            call_timeout(
                "/v1/probe-entrances",
                &serde_json::json!({ "timeout_ms": 3000 })
            ),
            Duration::from_secs(11)
        );
        assert_eq!(
            call_timeout(
                "/v1/probe-availability",
                &serde_json::json!({ "timeout_ms": u64::MAX })
            ),
            Duration::from_secs(125)
        );
        for path in ALLOWED_PATHS {
            assert!(call_timeout(path, &none) <= Duration::from_secs(30));
        }
    }

    #[test]
    fn clients_may_pin_an_ingress_but_not_reach_the_rest_of_the_core_api() {
        assert!(ALLOWED_PATHS.contains(&"/v1/pin-ingress"));
        assert!(!ALLOWED_PATHS.contains(&"/v1/set-system-proxy"));
    }
}
