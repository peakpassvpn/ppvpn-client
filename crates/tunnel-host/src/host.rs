//! The one host of the process: its runtime, the engine, the socket server
//! and the health loop, from `start` to `stop`.

use std::ffi::{c_char, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
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

/// The native side's `on_exit`.
pub struct ExitCallback {
    function: extern "C" fn(*mut c_void, *const c_char),
    context: usize,
}

impl ExitCallback {
    pub fn new(function: extern "C" fn(*mut c_void, *const c_char), context: *mut c_void) -> Self {
        Self {
            function,
            context: context as usize,
        }
    }

    /// Calls it on a thread of its own: the native side may call
    /// `ppvpn_tunnel_stop` from it, which waits for this host's runtime.
    pub(crate) fn call(self, code: &str, message: &str) {
        let reason = json!({ "code": code, "message": message }).to_string();
        let _ = std::thread::Builder::new()
            .name("ppvpn-tunnel-exit".into())
            .spawn(move || {
                let reason = CString::new(reason).unwrap_or_default();
                (self.function)(self.context as *mut c_void, reason.as_ptr());
            });
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
}

static HOST: Mutex<Option<Host>> = Mutex::new(None);
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
    if slot.is_some() {
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
    runtime.spawn(crate::health::run(shared, signals_rx, exit));
    log.line(&format!(
        "started: ppvpn-core {}, {platform:?}, socket {}",
        Engine::version().core_version,
        socket.display()
    ));
    *slot = Some(Host {
        runtime,
        engine,
        shutdown,
        signals,
        socket: socket.to_path_buf(),
        log,
    });
    Ok(())
}

pub(crate) fn stop() {
    // Taken out first: the engine's shutdown is waited for without the lock.
    let Some(host) = lock(&HOST).take() else {
        return;
    };
    let _ = host.shutdown.send(true);
    if let Err(error) = host.runtime.block_on(host.engine.shutdown()) {
        host.log.line(&format!(
            "engine shutdown: {}: {}",
            error.code, error.message
        ));
    }
    host.runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    let _ = std::fs::remove_file(&host.socket);
    host.log.line("stopped");
}

pub(crate) fn network_changed(state: NetworkState) -> Result<(), ApiError> {
    *lock(&NETWORK) = Some(state.clone());
    let slot = lock(&HOST);
    if let Some(host) = slot.as_ref() {
        host.engine
            .set_network_state(state)
            .map_err(ApiError::from)?;
        let _ = host.signals.send(Signal::Network);
    }
    Ok(())
}

/// The running host's engine and a handle to its runtime, if any.
fn engine() -> Option<(tokio::runtime::Handle, Engine)> {
    lock(&HOST)
        .as_ref()
        .map(|host| (host.runtime.handle().clone(), host.engine.clone()))
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
