//! The one host of the process: its runtime, the engine, the socket server
//! and the health loop, from `start` to `stop`.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use ppvpn_core::{
    Engine, EngineConfig, LogConfig, LogLevel, LogSink, NetworkState, Platform, Role, TunConfig,
};
use ppvpn_engine_host::ApiError;
use serde::Deserialize;
use serde_json::json;
use tokio::runtime::Runtime;
use tokio::sync::{broadcast, mpsc, watch};

use crate::log::HostLog;
use crate::{api_error, request_invalid};

/// The iOS packet tunnel's connection cap (docs/host-integration.md 11,
/// #388): the system ends the extension at 50 MiB.
const IOS_MAX_CONNECTIONS: u32 = 128;
/// How long `stop` waits for the runtime's tasks after the engine is down.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

/// `ppvpn_tunnel_start`'s options (JSON; every field optional).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// 0: unlimited. Default: [`IOS_MAX_CONNECTIONS`] on iOS, the mobile
    /// profile's on Android.
    #[serde(default)]
    pub max_connections: Option<u32>,
    #[serde(default)]
    pub log_level: Option<LogLevel>,
}

impl Options {
    pub fn parse(json: &str) -> Result<Self, ApiError> {
        serde_json::from_str(json).map_err(|error| request_invalid(format!("options: {error}")))
    }
}

pub use crate::exit::ExitCallback;

impl ExitCallback {
    pub(crate) fn notify(self, code: &str, message: &str) {
        let reason = json!({ "code": code, "message": message }).to_string();
        if let Err(error) = self.call(CString::new(reason).unwrap_or_default()) {
            eprintln!("tunnel exit callback: {error}");
        }
    }
}

/// What the health loop hears besides the engine's events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    /// The native side reported a network change.
    Network,
    /// The UI process started the engine (a user's connect): a new
    /// recovery episode.
    UserStart,
}

/// The host's channels, shared by the server and the health loop.
#[derive(Clone)]
pub(crate) struct Shared {
    pub engine: Engine,
    /// `true` once the host stops.
    pub shutdown: watch::Receiver<bool>,
    /// Health frames for the watch connections.
    pub health: broadcast::Sender<serde_json::Value>,
    pub signals: mpsc::UnboundedSender<Signal>,
    pub log: Arc<HostLog>,
}

struct Host {
    runtime: Runtime,
    engine: Engine,
    shutdown: watch::Sender<bool>,
    signals: mpsc::UnboundedSender<Signal>,
    socket: PathBuf,
    log: Arc<HostLog>,
    exit: Option<ExitCallback>,
}

// Keep the stop's completion and callback even after the runtime is gone:
// a second caller must not release the native context while the first
// caller is still shutting down, or while its callback is returning.
enum HostSlot {
    Empty,
    Running(Host),
    Stopping(Arc<Stopping>),
}

struct Stopping {
    done: Mutex<bool>,
    finished: Condvar,
    exit: Option<ExitCallback>,
}

struct StopCompletion {
    stopping: Arc<Stopping>,
    socket: PathBuf,
    log: Arc<HostLog>,
}

impl Drop for StopCompletion {
    fn drop(&mut self) {
        // Also run on unwind: the C boundary catches panics, and a later
        // stop must never be left waiting for an abandoned shutdown.
        if let Some(exit) = &self.stopping.exit {
            exit.cancel_and_wait();
        }
        let _ = std::fs::remove_file(&self.socket);
        self.log.line("stopped");
        *lock(&self.stopping.done) = true;
        self.stopping.finished.notify_all();
    }
}

impl Stopping {
    fn wait(&self) {
        // Another caller may be stopping the runtime while waiting for
        // this callback. Waiting here would make them wait on each other.
        if self.exit.as_ref().is_some_and(ExitCallback::is_current) {
            return;
        }
        let mut done = lock(&self.done);
        while !*done {
            done = self.finished.wait(done).unwrap_or_else(|e| e.into_inner());
        }
        drop(done);
        if let Some(exit) = &self.exit {
            exit.cancel_and_wait();
        }
    }
}

static HOST: Mutex<HostSlot> = Mutex::new(HostSlot::Empty);
/// The last network the native side reported, before start too.
static NETWORK: Mutex<Option<NetworkState>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether the device is offline, as the native side last said.
pub(crate) fn offline() -> bool {
    lock(&NETWORK)
        .as_ref()
        .is_some_and(|state| state.interface.is_none())
}

pub(crate) fn start(
    tun_fd: i32,
    socket: &Path,
    state_dir: &Path,
    platform: Platform,
    options: Options,
    exit: Option<ExitCallback>,
) -> Result<(), ApiError> {
    start_with(
        tun_fd,
        socket,
        state_dir,
        platform,
        options,
        exit,
        |config| config,
    )
}

/// [`start`], with the engine's configuration adjusted by `configure`
/// (Android's socket protection; the tests' TUN-less engine).
pub(crate) fn start_with(
    tun_fd: i32,
    socket: &Path,
    state_dir: &Path,
    platform: Platform,
    options: Options,
    exit: Option<ExitCallback>,
    configure: impl FnOnce(EngineConfig) -> EngineConfig,
) -> Result<(), ApiError> {
    let mut slot = lock(&HOST);
    let busy = match &*slot {
        HostSlot::Empty => false,
        HostSlot::Running(_) => true,
        HostSlot::Stopping(stop) => {
            !*lock(&stop.done) || stop.exit.as_ref().is_some_and(ExitCallback::is_running)
        }
    };
    if busy {
        return Err(api_error(
            "TUNNEL_ALREADY_RUNNING",
            "the tunnel host is already running",
            false,
        ));
    }
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs)
        .map_err(|error| request_invalid(format!("state_dir {}: {error}", state_dir.display())))?;
    let log = Arc::new(HostLog::open(&logs.join(crate::log::HOST_LOG)));
    let core_log = crate::log::rotated(&logs.join(crate::log::CORE_LOG));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("ppvpn-tunnel")
        .enable_all()
        .build()
        .map_err(|error| api_error("TUNNEL_INTERNAL", format!("runtime: {error}"), false))?;

    let mut config = EngineConfig::new(Role::Tun, platform, state_dir.to_path_buf())
        .with_tun(TunConfig::new().with_fd(tun_fd))
        .with_log(LogConfig::new(
            options.log_level.unwrap_or_default(),
            LogSink::File { path: core_log },
        ));
    let max_connections = options
        .max_connections
        .or((platform == Platform::Ios).then_some(IOS_MAX_CONNECTIONS));
    if let Some(max) = max_connections {
        config = config.with_max_connections(max);
    }
    let config = configure(config);
    let engine = runtime
        .block_on(Engine::new(config))
        .map_err(ApiError::from)?;
    if let Some(state) = lock(&NETWORK).clone() {
        engine.set_network_state(state).map_err(ApiError::from)?;
    }

    let listener = {
        let _entered = runtime.enter();
        crate::server::bind(socket)?
    };
    let (shutdown, shutdown_rx) = watch::channel(false);
    let (health, _) = broadcast::channel(16);
    let (signals, signals_rx) = mpsc::unbounded_channel();
    let shared = Shared {
        engine: engine.clone(),
        shutdown: shutdown_rx,
        health,
        signals: signals.clone(),
        log: log.clone(),
    };
    runtime.spawn(crate::server::serve(listener, shared.clone()));
    runtime.spawn(crate::health::run(shared, signals_rx, exit.clone()));
    log.line(&format!(
        "started: ppvpn-core {}, {platform:?}, socket {}",
        Engine::version().core_version,
        socket.display()
    ));
    *slot = HostSlot::Running(Host {
        runtime,
        engine,
        shutdown,
        signals,
        socket: socket.to_path_buf(),
        log,
        exit,
    });
    Ok(())
}

pub(crate) fn stop() {
    stop_using(|host| {
        let _ = host.shutdown.send(true);
        if let Err(error) = host.runtime.block_on(host.engine.shutdown()) {
            host.log.line(&format!(
                "engine shutdown: {}: {}",
                error.code, error.message
            ));
        }
        host.runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    });
}

fn stop_using(shutdown: impl FnOnce(Host) + Send + 'static) {
    stop_with_executor(
        shutdown,
        tokio::runtime::Handle::try_current().is_ok(),
        |task| {
            std::thread::Builder::new()
                .name("ppvpn-tunnel-stop".into())
                .spawn(task)
        },
    );
}

fn stop_with_executor(
    shutdown: impl FnOnce(Host) + Send + 'static,
    offload: bool,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<std::thread::JoinHandle<()>>,
) {
    let (host, stopping) = {
        let mut slot = lock(&HOST);
        match &*slot {
            HostSlot::Empty => return,
            HostSlot::Stopping(stopping) => {
                let stopping = stopping.clone();
                drop(slot);
                stopping.wait();
                return;
            }
            HostSlot::Running(host) => {
                let stopping = Arc::new(Stopping {
                    done: Mutex::new(false),
                    finished: Condvar::new(),
                    exit: host.exit.clone(),
                });
                let HostSlot::Running(host) =
                    std::mem::replace(&mut *slot, HostSlot::Stopping(stopping.clone()))
                else {
                    unreachable!();
                };
                (host, stopping)
            }
        }
    };
    // Never hold HOST across shutdown or across a native callback.
    // The original caller owns completion, including when it is the
    // callback itself inside a Tokio context. A cleanup worker waiting
    // for that callback would deadlock with the caller joining the worker.
    // Construct it before dispatch so a failed spawn also finishes the stop.
    let _completion = StopCompletion {
        stopping,
        socket: host.socket.clone(),
        log: host.log.clone(),
    };
    if offload {
        // Runtime::block_on and dropping Runtime both panic inside a
        // Tokio context, even a different runtime's. A native caller may
        // have entered its own runtime; finish outside that context.
        // Do not move the runtime into the spawn request: when thread
        // creation fails, that request is dropped on the caller's Tokio
        // thread, where dropping Runtime would panic again. Hand it over
        // only after the worker exists.
        let (send, receive) = std::sync::mpsc::sync_channel::<Box<dyn FnOnce() + Send>>(1);
        match spawn(Box::new(move || {
            if let Ok(task) = receive.recv() {
                task();
            }
        })) {
            Ok(worker) => {
                assert!(send.send(Box::new(move || shutdown(host))).is_ok());
                if let Err(panic) = worker.join() {
                    std::panic::resume_unwind(panic);
                }
            }
            Err(error) => {
                let _ = host.shutdown.send(true);
                host.log.line(&format!(
                    "shutdown worker: {error}; stopping the runtime in background"
                ));
                // Tokio's nonblocking shutdown is valid in a caller's
                // runtime too. Completion still cancels/drains the native
                // callback before the caller may free its context.
                host.runtime.shutdown_background();
            }
        }
    } else {
        shutdown(host);
    }
}

pub(crate) fn network_changed(state: NetworkState) -> Result<(), ApiError> {
    *lock(&NETWORK) = Some(state.clone());
    let slot = lock(&HOST);
    if let HostSlot::Running(host) = &*slot {
        host.engine
            .set_network_state(state)
            .map_err(ApiError::from)?;
        let _ = host.signals.send(Signal::Network);
    }
    Ok(())
}

/// The running host's engine and a handle to its runtime, if any.
fn engine() -> Option<(tokio::runtime::Handle, Engine)> {
    match &*lock(&HOST) {
        HostSlot::Running(host) => Some((host.runtime.handle().clone(), host.engine.clone())),
        _ => None,
    }
}

pub(crate) fn sleep() {
    if let Some((runtime, engine)) = engine() {
        runtime.block_on(engine.sleep());
    }
}

pub(crate) fn wake() {
    if let Some((runtime, engine)) = engine() {
        runtime.block_on(engine.wake());
    }
}

#[cfg(test)]
pub(crate) fn test_lock() -> MutexGuard<'static, ()> {
    static TESTS: Mutex<()> = Mutex::new(());
    lock(&TESTS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    fn send(stream: &mut UnixStream, value: &Value) {
        let body = serde_json::to_vec(value).unwrap();
        stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&body).unwrap();
    }

    fn receive(stream: &mut UnixStream) -> Option<Value> {
        let mut length = [0u8; 4];
        stream.read_exact(&mut length).ok()?;
        let mut body = vec![0u8; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).ok()?;
        Some(serde_json::from_slice(&body).unwrap())
    }

    fn call(socket: &Path, request: Value) -> Value {
        let mut stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        send(&mut stream, &request);
        receive(&mut stream).unwrap()
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ppvpn-tunnel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The fd of /dev/null as the TUN: the engine is never started here.
    fn fake_tun() -> std::fs::File {
        std::fs::File::open("/dev/null").unwrap()
    }

    fn start_at(socket: &Path, state: &Path, fd: i32) -> Result<(), ApiError> {
        start(
            fd,
            socket,
            state,
            Platform::Android,
            Options::default(),
            None,
        )
    }

    struct ExitProbe {
        entered: std::sync::mpsc::Sender<()>,
        proceed: Mutex<std::sync::mpsc::Receiver<()>>,
        self_stop: bool,
        tokio_stop: bool,
        failed_spawn: bool,
        stopped: std::sync::mpsc::Sender<()>,
    }

    extern "C" fn on_exit(context: *mut std::ffi::c_void, _: *const std::ffi::c_char) {
        // The test drains the callback before dropping its probe.
        let probe = unsafe { &*(context as *const ExitProbe) };
        if probe.self_stop {
            let stop_host = || {
                if probe.failed_spawn {
                    stop_with_executor(
                        |_| panic!("no worker can run this task"),
                        true,
                        |_| Err(std::io::Error::other("test: no thread available")),
                    );
                } else {
                    stop();
                }
            };
            if probe.tokio_stop {
                let caller = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                caller.block_on(async {
                    stop_host();
                });
            } else {
                stop_host();
            }
        }
        let _ = probe.entered.send(());
        let _ = lock(&probe.proceed).recv();
        // Also exercise a callback asking to stop while another caller
        // already owns shutdown and is draining this callback.
        stop();
        let _ = probe.stopped.send(());
    }

    fn with_exit(dir: &Path, tun: &std::fs::File, probe: &ExitProbe) -> ExitCallback {
        use std::os::fd::AsRawFd;
        let exit = ExitCallback::new(on_exit, std::ptr::from_ref(probe).cast_mut().cast());
        start(
            tun.as_raw_fd(),
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            Platform::Ios,
            Options::default(),
            Some(exit.clone()),
        )
        .unwrap();
        exit
    }

    #[test]
    fn concurrent_stops_drain_a_callback_that_itself_calls_stop() {
        concurrent_stops(false);
    }

    #[test]
    fn a_failed_shutdown_spawn_still_drains_a_running_callback() {
        concurrent_stops(true);
    }

    fn concurrent_stops(failed_spawn: bool) {
        let _serial = test_lock();
        let dir = dir("exit-drain");
        let tun = fake_tun();
        let (entered, ready) = std::sync::mpsc::channel();
        let (proceed, wait) = std::sync::mpsc::channel();
        let (stopped, callback_done) = std::sync::mpsc::channel();
        let probe = ExitProbe {
            entered,
            proceed: Mutex::new(wait),
            self_stop: false,
            tokio_stop: false,
            failed_spawn: false,
            stopped,
        };
        let exit = with_exit(&dir, &tun, &probe);
        exit.clone().notify("TEST_EXIT", "test");
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let (done, all_done) = std::sync::mpsc::channel();
        let callers: Vec<_> = (0..2)
            .map(|_| {
                let done = done.clone();
                std::thread::spawn(move || {
                    if failed_spawn {
                        stop_with_executor(
                            |_| panic!("no worker can run this task"),
                            true,
                            |_| Err(std::io::Error::other("test: no thread available")),
                        );
                    } else {
                        stop();
                    }
                    done.send(()).unwrap();
                })
            })
            .collect();
        // Wait until a real host shutdown owns the slot, rather than
        // merely assuming the stopping thread has been scheduled.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if matches!(&*lock(&HOST), HostSlot::Stopping(_)) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(all_done.recv_timeout(Duration::from_millis(50)).is_err());
        proceed.send(()).unwrap();
        callback_done.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..2 {
            all_done.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        for caller in callers {
            caller.join().unwrap();
        }
        exit.cancel_and_wait();
        stop();
        assert!(!dir.join("tunnel.sock").exists());
        use std::os::fd::AsRawFd;
        start_at(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            tun.as_raw_fd(),
        )
        .unwrap();
        // An old generation cannot invoke the native callback again.
        exit.notify("LATE_EXIT", "old host");
        assert_eq!(
            call(&dir.join("tunnel.sock"), json!({"path":"/v1/get-version"}))["ok"],
            true
        );
        stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_self_stopped_callback_is_drained_before_a_foreign_stop_or_new_start() {
        self_stopped_callback(false, false);
    }

    #[test]
    fn a_self_stopped_callback_in_a_tokio_context_does_not_wait_for_itself() {
        self_stopped_callback(true, false);
    }

    #[test]
    fn a_callback_in_tokio_can_stop_when_its_shutdown_worker_cannot_start() {
        self_stopped_callback(true, true);
    }

    fn self_stopped_callback(tokio_stop: bool, failed_spawn: bool) {
        let _serial = test_lock();
        let dir = dir("exit-self-stop");
        let tun = fake_tun();
        let (entered, ready) = std::sync::mpsc::channel();
        let (proceed, wait) = std::sync::mpsc::channel();
        let (stopped, callback_done) = std::sync::mpsc::channel();
        let probe = ExitProbe {
            entered,
            proceed: Mutex::new(wait),
            self_stop: true,
            tokio_stop,
            failed_spawn,
            stopped,
        };
        let exit = with_exit(&dir, &tun, &probe);
        exit.clone().notify("TEST_EXIT", "test");
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        use std::os::fd::AsRawFd;
        assert_eq!(
            start_at(
                &dir.join("tunnel.sock"),
                &dir.join("state"),
                tun.as_raw_fd()
            )
            .unwrap_err()
            .code,
            "TUNNEL_ALREADY_RUNNING"
        );
        let (done, finished) = std::sync::mpsc::channel();
        let foreign = std::thread::spawn(move || {
            stop();
            done.send(()).unwrap();
        });
        assert!(finished.recv_timeout(Duration::from_millis(50)).is_err());
        proceed.send(()).unwrap();
        callback_done.recv_timeout(Duration::from_secs(5)).unwrap();
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        foreign.join().unwrap();
        exit.cancel_and_wait();
        start_at(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            tun.as_raw_fd(),
        )
        .unwrap();
        stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_start_never_owns_or_calls_the_native_context() {
        let _serial = test_lock();
        let dir = dir("exit-failed-start");
        let tun = fake_tun();
        extern "C" fn count(context: *mut std::ffi::c_void, _: *const std::ffi::c_char) {
            let calls = unsafe { &*(context as *const std::sync::atomic::AtomicUsize) };
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let exit = ExitCallback::new(count, std::ptr::from_ref(&calls).cast_mut().cast());
        use std::os::fd::AsRawFd;
        let error = start(
            tun.as_raw_fd(),
            &dir.join("missing/socket"),
            &dir.join("state"),
            Platform::Ios,
            Options::default(),
            Some(exit),
        )
        .unwrap_err();
        assert_eq!(error.code, "TUNNEL_SOCKET_FAILED");
        stop();
        stop();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        start_at(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            tun.as_raw_fd(),
        )
        .unwrap();
        stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_shutdown_panic_finishes_the_stop_and_allows_another_start() {
        let _serial = test_lock();
        let dir = dir("stop-unwind");
        let tun = fake_tun();
        use std::os::fd::AsRawFd;
        start_at(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            tun.as_raw_fd(),
        )
        .unwrap();
        let panic = std::panic::catch_unwind(|| {
            stop_using(|_host| panic!("test: shutdown unwound"));
        });
        assert!(panic.is_err());
        let (done, finished) = std::sync::mpsc::channel();
        let caller = std::thread::spawn(move || {
            stop();
            done.send(()).unwrap();
        });
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        caller.join().unwrap();
        assert!(!dir.join("tunnel.sock").exists());
        start_at(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            tun.as_raw_fd(),
        )
        .unwrap();
        stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stop_from_a_callers_tokio_context_finishes_outside_that_context() {
        let _serial = test_lock();
        let dir = dir("stop-entered-runtime");
        let tun = fake_tun();
        use std::os::fd::AsRawFd;
        start_at(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            tun.as_raw_fd(),
        )
        .unwrap();
        let caller = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        caller.block_on(async {
            stop();
        });
        assert!(!dir.join("tunnel.sock").exists());
        stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_shutdown_spawn_finishes_stop() {
        failed_shutdown_spawn(false);
    }

    #[test]
    fn a_failed_shutdown_spawn_in_tokio_does_not_drop_a_runtime_in_tokio() {
        failed_shutdown_spawn(true);
    }

    fn failed_shutdown_spawn(in_tokio: bool) {
        let _serial = test_lock();
        let dir = dir("stop-spawn-failure");
        let tun = fake_tun();
        use std::os::fd::AsRawFd;
        start_at(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            tun.as_raw_fd(),
        )
        .unwrap();
        let stop = || {
            stop_with_executor(
                |_| panic!("no shutdown task can run without its worker"),
                true,
                |_| Err(std::io::Error::other("test: no thread available")),
            )
        };
        if in_tokio {
            let caller = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            caller.block_on(async {
                stop();
            });
        } else {
            stop();
        }
        let (done, finished) = std::sync::mpsc::channel();
        let caller = std::thread::spawn(move || {
            super::stop();
            done.send(()).unwrap();
        });
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        caller.join().unwrap();
        assert!(!dir.join("tunnel.sock").exists());
        start_at(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            tun.as_raw_fd(),
        )
        .unwrap();
        super::stop();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn serves_core_api_on_the_socket_until_stopped() {
        let _serial = test_lock();
        let dir = dir("serve");
        let socket = dir.join("tunnel.sock");
        let state = dir.join("state");
        let tun = fake_tun();
        use std::os::fd::AsRawFd;
        // A network reported before start is kept for it.
        network_changed(NetworkState::offline()).unwrap();
        start_at(&socket, &state, tun.as_raw_fd()).unwrap();
        assert!(offline());

        let version = call(&socket, json!({"path": "/v1/get-version"}));
        assert_eq!(version["ok"], json!(true), "{version}");
        assert_eq!(version["data"]["core_api_version"], json!(1));
        let status = call(&socket, json!({"path": "/v1/get-status", "body": {}}));
        assert_eq!(status["data"]["state"], json!("stopped"), "{status}");
        let unknown = call(&socket, json!({"path": "/v1/nothing"}));
        assert_eq!(unknown["ok"], json!(false));
        assert_eq!(unknown["error"]["code"], json!("CORE_API_UNSUPPORTED"));
        let invalid = call(&socket, json!({"body": {}}));
        assert_eq!(invalid["error"]["code"], json!("REQUEST_INVALID"));
        let started = call(&socket, json!({"path": "/v1/start"}));
        assert_eq!(
            started["error"]["code"],
            json!("PROFILE_NOT_APPLIED"),
            "{started}"
        );

        let mut watch = UnixStream::connect(&socket).unwrap();
        watch
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        send(&mut watch, &json!({"path": "/v1/watch", "body": {}}));
        let first = receive(&mut watch).unwrap();
        assert_eq!(first["data"]["kind"], json!("status"), "{first}");
        assert_eq!(first["data"]["status"]["state"], json!("stopped"));

        assert_eq!(
            start_at(&socket, &state, tun.as_raw_fd()).unwrap_err().code,
            "TUNNEL_ALREADY_RUNNING"
        );
        network_changed(NetworkState::new("wlan0", ppvpn_core::NetworkKind::Wifi)).unwrap();
        assert!(!offline());
        sleep();
        wake();

        stop();
        // The watch ends with the host, the socket file goes, the fd stays.
        assert!(receive(&mut watch).is_none());
        assert!(!socket.exists());
        assert!(tun.metadata().is_ok());
        assert!(state.join("logs").join(crate::log::HOST_LOG).exists());
        stop();

        // And it starts again.
        start_at(&socket, &state, tun.as_raw_fd()).unwrap();
        assert_eq!(
            call(&socket, json!({"path": "/v1/get-version"}))["ok"],
            json!(true)
        );
        stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_socket_path_too_long_for_sun_path_is_bound_relative() {
        let _serial = test_lock();
        let dir = dir("long");
        let deep = dir.join("x".repeat(60)).join("y".repeat(60));
        std::fs::create_dir_all(&deep).unwrap();
        let socket = deep.join("tunnel.sock");
        assert!(socket.as_os_str().len() >= 104);
        let tun = fake_tun();
        use std::os::fd::AsRawFd;
        let cwd = std::env::current_dir().unwrap();
        start_at(&socket, &dir.join("state"), tun.as_raw_fd()).unwrap();
        assert!(socket.exists());
        // A client connects the same way: from the directory.
        let mut stream = UnixStream::connect("tunnel.sock").unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        send(&mut stream, &json!({"path": "/v1/get-version"}));
        assert_eq!(receive(&mut stream).unwrap()["ok"], json!(true));
        stop();
        std::env::set_current_dir(cwd).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_parse() {
        assert_eq!(Options::parse("{}").unwrap(), Options::default());
        let options = Options::parse(r#"{"max_connections": 0, "log_level": "debug"}"#).unwrap();
        assert_eq!(options.max_connections, Some(0));
        assert_eq!(options.log_level, Some(LogLevel::Debug));
        assert_eq!(
            Options::parse(r#"{"max_conections": 1}"#).unwrap_err().code,
            "REQUEST_INVALID"
        );
        assert_eq!(
            Options::parse(r#"{"log_level": "trace"}"#)
                .unwrap_err()
                .code,
            "REQUEST_INVALID"
        );
    }
}
