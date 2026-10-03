//! ppvpn-core-lab: the Rust ppvpn-core behind Core API v1, for the lab and
//! the netns CI. Not a product: hosts link the library. It starts and logs
//! as `ppvpn-core serve` does (same flags, same logfmt lines), so the
//! scripts under test/ run against it unchanged.
//!
//!     ppvpn-core-lab serve --socket <path> --session-secret-file <path> --state-dir <dir> [...]
//!     ppvpn-core-lab version

mod api;
mod corelog;
mod flags;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use corelog::Logger;
use ppvpn_core::{
    Engine, EngineConfig, LocalProxyConfig, LogConfig, LogLevel, LogSink, Platform, Role, TunConfig,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    };
    std::process::exit(code);
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first() else {
        return Err("usage: ppvpn-core-lab <version|serve>".into());
    };
    match command.as_str() {
        "version" => {
            println!(
                "{}",
                serde_json::to_string(&Engine::version()).expect("version")
            );
            Ok(())
        }
        "serve" => serve(&args[1..]),
        other => Err(format!("unknown command {other:?}")),
    }
}

/// `ppvpn-core serve`'s flags, defaults and usage text.
fn serve_flags() -> flags::FlagSet {
    let mut f = flags::FlagSet::new("serve");
    f.string("socket", "", "Unix socket or Windows named pipe path");
    f.string(
        "session-secret-file",
        "",
        "private file used to exchange the random session secret",
    );
    f.string("state-dir", "", "private directory for device-local state");
    f.string("platform", "desktop", "platform capability name");
    f.bool("local-proxy", true, "enable the shared authenticated local HTTP/SOCKS5 proxy (one port, node chosen by username)");
    f.bool(
        "tun",
        false,
        "enable sing-box TUN inbound (requires host-provided privileges)",
    );
    f.string(
        "tun-stack",
        "mixed",
        "sing-box TUN stack: mixed, system, or gvisor",
    );
    f.string("local-dns-servers", "", "with --tun: comma-separated physical-network resolvers (IP, IP:port or [IPv6]:port) read before system DNS was pointed at the tunnel; the first outside the tunnel answers direct-routed names over UDP");
    f.bool(
        "exit-on-stdin-close",
        false,
        "exit when the parent-owned stdin pipe closes",
    );
    f.string(
        "log-file",
        "",
        "append the core diagnostic log to this file (default: stderr)",
    );
    f.string("log-level", corelog::LEVEL_INFO, "diagnostic log level: info, or debug (adds one line per routed connection, including the domains visited; enable only while diagnosing)");
    f
}

fn serve(args: &[String]) -> Result<(), String> {
    let mut f = serve_flags();
    if let Err(err) = f.parse(args) {
        eprint!("{}", f.usage());
        return Err(err);
    }
    let log = match f.get("log-file").as_str() {
        "" => Logger::stderr(),
        path => Logger::open_file(Path::new(path)).map_err(|e| format!("open log file: {e}"))?,
    };
    log.set_level(&f.get("log-level"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let result = runtime.block_on(serve_with_log(&log, &f));
    if let Err(err) = &result {
        log.error("serve failed", &[("error", err), ("chain", err)]);
    }
    result
}

struct Settings {
    socket: String,
    secret_file: String,
    state_dir: String,
    platform: String,
    local_proxy: bool,
    tun: bool,
    tun_stack: String,
    local_dns: Vec<String>,
    exit_on_stdin_close: bool,
}

async fn serve_with_log(log: &Logger, f: &flags::FlagSet) -> Result<(), String> {
    let s = Settings {
        socket: f.get("socket"),
        secret_file: f.get("session-secret-file"),
        state_dir: f.get("state-dir"),
        platform: f.get("platform"),
        local_proxy: f.get_bool("local-proxy"),
        tun: f.get_bool("tun"),
        tun_stack: f.get("tun-stack"),
        local_dns: split_list(&f.get("local-dns-servers")),
        exit_on_stdin_close: f.get_bool("exit-on-stdin-close"),
    };
    log.info(
        "serve starting",
        &[
            ("core_version", &Engine::version().core_version),
            ("sail_commit", &Engine::version().sail_commit),
            ("os", &go_os()),
            ("arch", &go_arch()),
            ("log_level", &log.level()),
            ("platform", &s.platform),
            ("tun", &s.tun),
            ("tun_stack", &s.tun_stack),
            ("local_proxy", &s.local_proxy),
            ("state_dir", &s.state_dir),
            ("socket", &s.socket),
        ],
    );
    if !s.local_dns.is_empty() && !s.tun {
        return Err("--local-dns-servers requires --tun".into());
    }
    if s.socket.is_empty() || s.secret_file.is_empty() || s.state_dir.is_empty() {
        return Err("serve requires --socket, --session-secret-file and --state-dir".into());
    }
    if !["mixed", "system", "gvisor"].contains(&s.tun_stack.as_str()) {
        return Err(format!(
            "unknown TUN stack {:?} (want mixed, system or gvisor)",
            s.tun_stack
        ));
    }
    create_private_dir(Path::new(&s.state_dir)).map_err(|e| format!("create state dir: {e}"))?;
    let secret = rotate_session_secret(Path::new(&s.secret_file))
        .map_err(|e| format!("write session secret: {e}"))?;
    let _remove_secret = RemoveOnDrop(PathBuf::from(&s.secret_file));

    let role = if s.tun { Role::Tun } else { Role::Standard };
    let level = if log.debug_enabled() {
        LogLevel::Debug
    } else {
        LogLevel::Info
    };
    let mut config = EngineConfig::new(role, platform(), &s.state_dir)
        .with_system_proxy(!s.tun)
        .with_log(LogConfig::new(level, LogSink::Channel));
    if s.local_proxy {
        config = config.with_local_proxy(LocalProxyConfig::new());
    }
    if s.tun {
        config = config.with_tun(TunConfig::new().with_local_dns_servers(s.local_dns.clone()));
    }
    let engine = Engine::new(config).await.map_err(|e| e.to_string())?;
    // The engine's lines, logfmt already, into the same log as serve's own.
    let mut lines = engine.logs();
    let engine_log = log.clone();
    tokio::spawn(async move {
        while let Some(line) = lines.recv().await {
            engine_log.raw(&line);
        }
    });
    let api = Arc::new(api::Api {
        engine: engine.clone(),
        secret,
        log: log.clone(),
    });
    let listener = listen(&s.socket)?;
    log.info("serve ready", &[("socket", &s.socket)]);

    let (stdin_closed_tx, stdin_closed) = tokio::sync::oneshot::channel::<()>();
    if s.exit_on_stdin_close {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            let _ = stdin_closed_tx.send(());
        });
    } else {
        std::mem::forget(stdin_closed_tx);
    }
    let reason = tokio::select! {
        err = serve_socket(listener, api) => return Err(format!("ipc serve: {err}")),
        reason = signal() => reason,
        _ = stdin_closed => "stdin closed",
    };
    log.info("serve stopping", &[("reason", &reason)]);
    let _ = tokio::time::timeout(Duration::from_secs(10), engine.shutdown()).await;
    let _ = std::fs::remove_file(&s.socket);
    Ok(())
}

#[cfg(unix)]
fn listen(socket: &str) -> Result<tokio::net::UnixListener, String> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = Path::new(socket).parent() {
        if !dir.as_os_str().is_empty() {
            create_private_dir(dir).map_err(|e| format!("listen on {socket}: {e}"))?;
        }
    }
    // A socket left by a core that was killed: nobody listens on it.
    if std::os::unix::net::UnixStream::connect(socket).is_err() {
        let _ = std::fs::remove_file(socket);
    }
    let listener =
        tokio::net::UnixListener::bind(socket).map_err(|e| format!("listen on {socket}: {e}"))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("listen on {socket}: {e}"))?;
    Ok(listener)
}

#[cfg(not(unix))]
fn listen(socket: &str) -> Result<std::convert::Infallible, String> {
    Err(format!(
        "listen on {socket}: named pipes come with the Windows build"
    ))
}

#[cfg(unix)]
async fn serve_socket(listener: tokio::net::UnixListener, api: Arc<api::Api>) -> String {
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => return e.to_string(),
        };
        let api = api.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| api.clone().handle(req));
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

#[cfg(not(unix))]
async fn serve_socket(listener: std::convert::Infallible, _api: Arc<api::Api>) -> String {
    match listener {}
}

#[cfg(unix)]
async fn signal() -> &'static str {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => "signal",
        _ = term.recv() => "signal",
    }
}

#[cfg(not(unix))]
async fn signal() -> &'static str {
    let _ = tokio::signal::ctrl_c().await;
    "signal"
}

/// A comma-separated flag value, empty items dropped.
fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// A new random secret (32 bytes, base64url without padding), written to a
/// private temporary file and renamed into place, as the Go core does.
fn rotate_session_secret(path: &Path) -> std::io::Result<String> {
    use base64::Engine as _;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(std::io::Error::other)?;
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let tmp = dir.join(format!(".session-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let _ = std::fs::remove_file(&tmp);
    let written = options.open(&tmp).and_then(|mut file| {
        use std::io::Write;
        file.write_all(secret.as_bytes())?;
        file.sync_all()
    });
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(secret)
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

/// GOOS: what the Go core logs as `os`.
fn go_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    }
}

/// GOARCH: what the Go core logs as `arch`.
fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        arch => arch,
    }
}

#[cfg(test)]
mod tests;
