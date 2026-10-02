//! Finding, launching and talking to the daemon.

use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::control::{Call, Request, Response, MAX_MESSAGE};
use crate::error::{CliError, Exit, Result};
use crate::identity::ProcessRecord;
use crate::paths::Paths;

/// How long a single control call may take; apply can wait up to 10 s for
/// rule sets inside core.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a new daemon gets to answer its first ping.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `stop` waits for the daemon to exit (core shutdown takes at most 10 s).
pub const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

fn core_unavailable(message: impl Into<String>) -> CliError {
    CliError::new(Exit::Core, "DAEMON_UNAVAILABLE", message).retryable()
}

/// The running daemon's process record, if the record still describes a
/// live daemon (PID, executable and start time match).
pub fn running_daemon(paths: &Paths) -> Option<ProcessRecord> {
    let data = fs::read(&paths.process_record).ok()?;
    let record: ProcessRecord = serde_json::from_slice(&data).ok()?;
    record.is_alive().then_some(record)
}

pub struct Client {
    paths: Paths,
    secret: String,
}

impl Client {
    pub fn new(paths: &Paths) -> Result<Client> {
        let secret = fs::read_to_string(&paths.secret)
            .map_err(|e| core_unavailable(format!("cannot read the session secret: {e}")))?;
        Ok(Client {
            paths: paths.clone(),
            secret: secret.trim().to_string(),
        })
    }

    pub async fn call(&self, call: Call) -> Result<Value> {
        tokio::time::timeout(CALL_TIMEOUT, self.call_once(call))
            .await
            .map_err(|_| core_unavailable("the daemon did not answer in time"))?
    }

    async fn call_once(&self, call: Call) -> Result<Value> {
        let mut stream = UnixStream::connect(&self.paths.socket)
            .await
            .map_err(|e| core_unavailable(format!("cannot connect to the daemon: {e}")))?;
        let mut data = serde_json::to_vec(&Request {
            secret: self.secret.clone(),
            call,
        })
        .map_err(|e| CliError::new(Exit::Other, "INTERNAL_ERROR", e.to_string()))?;
        data.push(b'\n');
        let io = |e: io::Error| core_unavailable(format!("control channel failed: {e}"));
        stream.write_all(&data).await.map_err(io)?;
        let mut reader = BufReader::new(stream.take(MAX_MESSAGE as u64 + 1));
        let mut line = String::new();
        reader.read_line(&mut line).await.map_err(io)?;
        match serde_json::from_str::<Response>(&line) {
            Ok(Response::Ok { data, .. }) => Ok(data),
            Ok(Response::Err { error, .. }) => Err(error.into_cli()),
            Err(e) => Err(core_unavailable(format!("malformed daemon response: {e}"))),
        }
    }
}

/// Connects to the running daemon, or starts one in the background and
/// waits for it to answer. Returns the client and whether it launched one.
pub async fn connect_or_launch(paths: &Paths) -> Result<(Client, bool)> {
    if running_daemon(paths).is_some() {
        let client = Client::new(paths)?;
        client.call(Call::Ping).await?;
        return Ok((client, false));
    }
    paths.ensure()?;
    let mut child =
        launch(paths).map_err(|e| core_unavailable(format!("cannot start the daemon: {e}")))?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(client) = Client::new(paths) {
            if client.call(Call::Ping).await.is_ok() {
                return Ok((client, true));
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(core_unavailable(format!(
                "the daemon exited during startup ({status}); see {}",
                paths.daemon_log.display()
            )));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err(core_unavailable("the daemon did not become ready in time"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Starts `<this executable> daemon` in its own session, detached from the
/// terminal, with output appended to the daemon log.
fn launch(paths: &Paths) -> io::Result<std::process::Child> {
    use std::os::unix::fs::OpenOptionsExt;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&paths.daemon_log)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe and touches no Rust state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

/// Waits until the recorded daemon has exited.
pub async fn wait_for_exit(record: &ProcessRecord, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while record.is_alive() {
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    true
}
