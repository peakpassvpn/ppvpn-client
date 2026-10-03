//! The daemon: one process per user that hosts ppvpn-core's standard
//! instance (local proxy only: no TUN, no system proxy) and serves the
//! control channel. `ppvpn start` runs it in the background;
//! `ppvpn start --foreground` runs it in the CLI's own process.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

use crate::control::{
    secrets_match, Call, Request, Response, WireError, MAX_MESSAGE, UNAUTHENTICATED,
};
use crate::env::Os;
use crate::error::{CliError, Exit, Result};
use crate::identity::ProcessRecord;
use crate::paths::{write_private_file, Paths};

/// Holds `daemon.lock` (flock) for the daemon's lifetime, so a second daemon
/// for the same directories refuses to start.
pub struct DaemonLock {
    // Closing the file releases the flock.
    _file: fs::File,
}

impl DaemonLock {
    pub fn acquire(paths: &Paths) -> Result<DaemonLock> {
        use std::os::unix::fs::OpenOptionsExt;
        let env_err = |err: io::Error| {
            CliError::environment(
                "RUNTIME_DIRECTORY_UNAVAILABLE",
                format!("cannot open the daemon lock: {err}"),
            )
        };
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&paths.lock)
            .map_err(env_err)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Err(CliError::new(
                    Exit::Core,
                    "DAEMON_ALREADY_RUNNING",
                    "another ppvpn daemon is running for this user",
                )
                .retryable());
            }
            return Err(env_err(err));
        }
        Ok(DaemonLock { _file: file })
    }
}

/// A fresh 256-bit secret, hex encoded.
fn new_secret() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    io::Read::read_exact(&mut fs::File::open("/dev/urandom")?, &mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Removes a leftover socket, refusing anything that is not a socket.
fn remove_stale_socket(paths: &Paths) -> io::Result<()> {
    match fs::symlink_metadata(&paths.socket) {
        Ok(meta) if meta.file_type().is_socket() => fs::remove_file(&paths.socket),
        Ok(_) => Err(io::Error::other("the control socket path is not a socket")),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn engine_config(paths: &Paths, os: Os) -> ppvpn_core::EngineConfig {
    let platform = match os {
        Os::Macos => ppvpn_core::Platform::Macos,
        Os::Linux => ppvpn_core::Platform::Linux,
    };
    ppvpn_core::EngineConfig::new(ppvpn_core::Role::Standard, platform, &paths.state_dir)
        .with_local_proxy(ppvpn_core::LocalProxyConfig::new())
        .with_system_proxy(false)
}

/// The daemon's state while it serves.
pub struct Daemon {
    paths: Paths,
    secret: String,
    engine: ppvpn_core::Engine,
    record: ProcessRecord,
    _lock: DaemonLock,
}

impl Daemon {
    /// Prepares directories, takes the lock, creates the engine, binds the
    /// socket and writes the secret and process record. Nothing is applied
    /// or started.
    pub async fn bind(paths: &Paths, os: Os) -> Result<(Daemon, UnixListener)> {
        paths.ensure()?;
        let lock = DaemonLock::acquire(paths)?;
        let env_err = |what: &str, err: io::Error| {
            CliError::environment("RUNTIME_DIRECTORY_UNAVAILABLE", format!("{what}: {err}"))
        };
        remove_stale_socket(paths)
            .map_err(|e| env_err("cannot remove the stale control socket", e))?;
        let engine = ppvpn_core::Engine::new(engine_config(paths, os))
            .await
            .map_err(|e| WireError::from(&e).into_cli())?;
        let secret = new_secret().map_err(|e| env_err("cannot create the session secret", e))?;
        write_private_file(&paths.secret, secret.as_bytes())
            .map_err(|e| env_err("cannot write the session secret", e))?;
        let listener = UnixListener::bind(&paths.socket)
            .map_err(|e| env_err("cannot bind the control socket", e))?;
        fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))
            .map_err(|e| env_err("cannot restrict the control socket", e))?;
        let record =
            ProcessRecord::current().map_err(|e| env_err("cannot read this process", e))?;
        let data = serde_json::to_vec(&record)
            .map_err(|e| env_err("cannot encode the process record", io::Error::other(e)))?;
        write_private_file(&paths.process_record, &data)
            .map_err(|e| env_err("cannot write the process record", e))?;
        Ok((
            Daemon {
                paths: paths.clone(),
                secret,
                engine,
                record,
                _lock: lock,
            },
            listener,
        ))
    }

    /// Serves until a `Shutdown` request, SIGTERM or SIGINT, then shuts the
    /// engine down and removes the socket, secret and record.
    pub async fn serve(self, listener: UnixListener) -> Result<()> {
        let daemon = Arc::new(self);
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| CliError::new(Exit::Other, "SIGNAL_SETUP_FAILED", e.to_string()))?;
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let daemon = daemon.clone();
                    let stop_tx = stop_tx.clone();
                    tokio::spawn(async move {
                        let _ = daemon.handle(stream, &stop_tx).await;
                    });
                }
                _ = stop_rx.changed() => break,
                _ = sigterm.recv() => break,
                _ = tokio::signal::ctrl_c() => break,
            }
        }
        let result = daemon.engine.shutdown().await;
        daemon.cleanup();
        result
            .map(|_| ())
            .map_err(|e| WireError::from(&e).into_cli())
    }

    fn cleanup(&self) {
        let _ = fs::remove_file(&self.paths.socket);
        let _ = fs::remove_file(&self.paths.secret);
        // Only remove the record if it is still ours.
        if fs::read(&self.paths.process_record)
            .ok()
            .and_then(|data| serde_json::from_slice::<ProcessRecord>(&data).ok())
            .is_some_and(|record| record == self.record)
        {
            let _ = fs::remove_file(&self.paths.process_record);
        }
    }

    async fn handle(&self, stream: UnixStream, stop: &watch::Sender<bool>) -> io::Result<()> {
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read.take(MAX_MESSAGE as u64 + 1));
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let response = match serde_json::from_str::<Request>(&line) {
            Err(_) if line.len() > MAX_MESSAGE => Response::err(WireError::from(
                &CliError::argument("control request too large"),
            )),
            Err(err) => Response::err(WireError::from(&CliError::argument(format!(
                "malformed control request: {err}"
            )))),
            Ok(request) if !secrets_match(&request.secret, &self.secret) => {
                Response::err(WireError {
                    code: UNAUTHENTICATED.into(),
                    field: None,
                    retryable: false,
                    message: "control request without the session secret".into(),
                })
            }
            Ok(request) => {
                let shutdown = matches!(request.call, Call::Shutdown);
                let response = self.call(request.call).await;
                if shutdown {
                    let _ = stop.send(true);
                }
                response
            }
        };
        let mut data = serde_json::to_vec(&response).map_err(io::Error::other)?;
        data.push(b'\n');
        write.write_all(&data).await?;
        write.shutdown().await
    }

    async fn call(&self, call: Call) -> Response {
        let core = |err: ppvpn_core::Error| WireError::from(&err);
        let result: std::result::Result<Value, WireError> = match call {
            Call::Ping => Ok(json!({
                "pid": self.record.pid,
                "core": ppvpn_core::Engine::version(),
            })),
            Call::Status => encode(&self.engine.status()),
            Call::Apply(request) => match self.engine.apply(request).await {
                Ok(result) => encode(&result),
                Err(err) => Err(core(err)),
            },
            Call::Start => self
                .engine
                .start()
                .await
                .map(|()| Value::Null)
                .map_err(core),
            Call::Shutdown => Ok(Value::Null),
        };
        match result {
            Ok(data) => Response::ok(data),
            Err(err) => Response::err(err),
        }
    }
}

fn encode<T: serde::Serialize>(value: &T) -> std::result::Result<Value, WireError> {
    serde_json::to_value(value).map_err(|err| WireError {
        code: "CORE_OPERATION_FAILED".into(),
        field: None,
        retryable: false,
        message: format!("cannot encode the core response: {err}"),
    })
}
