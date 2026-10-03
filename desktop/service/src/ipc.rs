//! Privileged IPC server. Windows rejects anonymous and guest access at the
//! pipe boundary; request authentication and session authorization are
//! enforced before an operation reaches the core manager.

use crate::core::CORE;
#[cfg(windows)]
use crate::protocol::IPC_PIPE_NAME;
use crate::protocol::{
    Command, ConnectPayload, CoreApiPayload, HandshakePayload, HandshakeResponse, Request,
    Response, SessionRef, UpdateProfilePayload, VersionInfo,
};
use anyhow::Result;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

static RECENT_REQUESTS: Lazy<Mutex<ReplayWindow>> =
    Lazy::new(|| Mutex::new(ReplayWindow::default()));
static AUTH_SESSIONS: Lazy<Mutex<HashMap<String, AuthSession>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const AUTH_SESSION_SECONDS: u64 = 120;

/// Set once the service is shutting down (SIGTERM / SIGINT, or the Windows
/// service Stop / Shutdown control). From then on nothing may start or feed
/// a core: see [`refused_while_stopping`].
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Error of every call refused while the service shuts down. Retryable: the
/// client backs off and reconnects once a service answers again.
pub const SERVICE_STOPPING: &str = "SERVICE_STOPPING";

/// Enters the stopping state (idempotent). Called before the data plane is
/// stopped, so a client reconnecting right away (its core just went away)
/// cannot get a new core started by a service that is about to exit.
pub fn begin_stopping() {
    if !STOPPING.swap(true, Ordering::SeqCst) {
        log::info!("service stopping: refusing Connect / UpdateProfile / RenewLease / CoreApi");
    }
}

/// Commands refused while stopping: Connect (including a takeover),
/// UpdateProfile, and the calls that keep a session alive, use its core or
/// watch it.
/// Handshake, GetVersion, GetStatus and Disconnect still work.
fn refused_while_stopping(command: &Command) -> bool {
    matches!(
        command,
        Command::Connect
            | Command::UpdateProfile
            | Command::RenewLease
            | Command::CoreApi
            | Command::Watch
    )
}

#[cfg(any(windows, test))]
fn decode_publisher_pins(
    values: &[Option<&str>],
) -> std::result::Result<Vec<Vec<u8>>, &'static str> {
    values
        .iter()
        .flatten()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("CLIENT_PUBLISHER_PIN_INVALID");
            }
            hex::decode(value).map_err(|_| "CLIENT_PUBLISHER_PIN_INVALID")
        })
        .collect()
}

#[cfg(any(windows, test))]
fn parse_allow_unsigned_client(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

#[cfg(windows)]
fn allow_unsigned_windows_client() -> bool {
    parse_allow_unsigned_client(option_env!("PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT"))
}

#[derive(Clone)]
struct AuthSession {
    key: [u8; 32],
    client_pid: u32,
    expires_at: u64,
}

#[derive(Default)]
struct ReplayWindow {
    request_timestamps: HashMap<String, u64>,
}

impl ReplayWindow {
    fn record_once(&mut self, request_id: &str, timestamp: u64, now: u64) -> bool {
        self.request_timestamps.retain(|_, seen_at| {
            now.saturating_sub(*seen_at) <= crate::protocol::MESSAGE_EXPIRY_SECONDS
        });
        if self.request_timestamps.contains_key(request_id) {
            return false;
        }
        self.request_timestamps
            .insert(request_id.to_string(), timestamp);
        true
    }
}

fn handle(req: Request, client_pid: u32) -> Result<Response> {
    let command = req.command.clone();
    // Connection changes are logged when they arrive and when answered;
    // the frequent calls only when slow (a stalled request shows either way).
    let changes = matches!(
        command,
        Command::Connect | Command::Disconnect | Command::UpdateProfile
    );
    if changes {
        log::info!("{command:?} from pid {client_pid}");
    }
    let started = std::time::Instant::now();
    let handled = handle_with(req, client_pid, STOPPING.load(Ordering::SeqCst));
    let took = started.elapsed();
    let outcome = match &handled {
        Ok(response) if response.success => "ok".to_string(),
        Ok(response) => response.error.clone().unwrap_or_else(|| "error".into()),
        Err(error) => format!("{error:#}"),
    };
    if took >= SLOW_REQUEST {
        log::warn!(
            "{command:?} from pid {client_pid} answered in {} ms ({outcome})",
            took.as_millis()
        );
    } else if changes {
        log::info!(
            "{command:?} from pid {client_pid} answered in {} ms ({outcome})",
            took.as_millis()
        );
    }
    handled
}

/// A request slower than this is logged whatever its kind.
const SLOW_REQUEST: std::time::Duration = std::time::Duration::from_secs(2);

/// Where a request stands after the envelope checks.
enum Admission {
    /// A fresh, never-seen handshake.
    Handshake,
    /// Authenticated and signed by `client_pid`'s session key.
    Authorized(AuthSession),
    /// Refused with this response.
    Refused(Response),
}

/// Freshness, replay, the PID-bound auth session, the signature, and the
/// stopping state (see [`refused_while_stopping`]).
fn admit(req: &Request, client_pid: u32, stopping: bool) -> Result<Admission> {
    let refuse = |error: &str| {
        Admission::Refused(Response::unsigned(
            req.id.clone(),
            false,
            None,
            Some(error.to_string()),
        ))
    };
    if !req.is_fresh() {
        return Ok(refuse("request expired"));
    }
    if !RECENT_REQUESTS
        .lock()
        .record_once(&req.id, req.timestamp, crate::protocol::now_epoch())
    {
        return Ok(refuse("request replayed"));
    }

    if req.command == Command::Handshake {
        return Ok(Admission::Handshake);
    }

    let token = req.auth_token.as_deref().unwrap_or_default();
    let now = crate::protocol::now_epoch();
    let auth = {
        let mut sessions = AUTH_SESSIONS.lock();
        sessions.retain(|_, session| session.expires_at > now);
        sessions.get(token).cloned()
    };
    let Some(auth) = auth.filter(|session| session.client_pid == client_pid) else {
        return Ok(refuse("AUTH_SESSION_INVALID"));
    };
    if !req.verify_with(&auth.key).unwrap_or(false) {
        return Ok(Admission::Refused(signed_error(
            req.id.clone(),
            "signature verification failed",
            &auth.key,
        )?));
    }
    if stopping && refused_while_stopping(&req.command) {
        return Ok(Admission::Refused(signed_error(
            req.id.clone(),
            SERVICE_STOPPING,
            &auth.key,
        )?));
    }
    Ok(Admission::Authorized(auth))
}

/// [`handle`] with the stopping state given (tests).
fn handle_with(req: Request, client_pid: u32, stopping: bool) -> Result<Response> {
    let auth = match admit(&req, client_pid, stopping)? {
        Admission::Handshake => return handshake(req, client_pid),
        Admission::Refused(response) => return Ok(response),
        Admission::Authorized(auth) => auth,
    };

    match req.command {
        Command::Handshake => unreachable!(),
        Command::GetVersion => {
            let info = VersionInfo {
                service: "PPVPN Service".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                build_id: crate::protocol::SERVICE_BUILD_ID.to_string(),
            };
            signed_ok(req.id, serde_json::to_value(info)?, &auth.key)
        }
        Command::GetStatus => {
            let status = CORE.lock().status(client_pid);
            signed_ok(req.id, serde_json::to_value(status)?, &auth.key)
        }
        Command::Connect => {
            let payload: ConnectPayload = match serde_json::from_value(req.payload) {
                Ok(p) => p,
                Err(e) => return signed_error(req.id, format!("bad payload: {e}"), &auth.key),
            };
            match CORE.lock().connect(payload, client_pid) {
                Ok(pid) => signed_ok(req.id, serde_json::json!({ "pid": pid }), &auth.key),
                Err(e) => signed_error(req.id, e.to_string(), &auth.key),
            }
        }
        Command::UpdateProfile => {
            let payload: UpdateProfilePayload = match serde_json::from_value(req.payload) {
                Ok(payload) => payload,
                Err(error) => {
                    return signed_error(req.id, format!("bad payload: {error}"), &auth.key)
                }
            };
            match crate::core::update_profile(&CORE, payload, client_pid) {
                Ok(status) => signed_ok(req.id, serde_json::to_value(status)?, &auth.key),
                Err(error) => signed_error(req.id, error.to_string(), &auth.key),
            }
        }
        Command::RenewLease => {
            let session: SessionRef = match serde_json::from_value(req.payload) {
                Ok(payload) => payload,
                Err(error) => {
                    return signed_error(req.id, format!("bad payload: {error}"), &auth.key)
                }
            };
            match CORE.lock().renew_lease(&session, client_pid) {
                Ok(status) => signed_ok(req.id, serde_json::to_value(status)?, &auth.key),
                Err(error) => signed_error(req.id, error.to_string(), &auth.key),
            }
        }
        Command::Disconnect => {
            let session: SessionRef = match serde_json::from_value(req.payload) {
                Ok(payload) => payload,
                Err(error) => {
                    return signed_error(req.id, format!("bad payload: {error}"), &auth.key)
                }
            };
            match CORE.lock().disconnect(&session, client_pid) {
                Ok(()) => signed_ok(req.id, serde_json::json!({}), &auth.key),
                Err(error) => signed_error(req.id, error.to_string(), &auth.key),
            }
        }
        Command::CoreApi => {
            let payload: CoreApiPayload = match serde_json::from_value(req.payload) {
                Ok(p) => p,
                Err(e) => return signed_error(req.id, format!("bad payload: {e}"), &auth.key),
            };
            // Not under the CORE lock: a slow call (a probe) must not hold
            // up Disconnect, lease renewals or status queries.
            match crate::core::call_api(
                &CORE,
                &payload.session,
                &payload.path,
                payload.body,
                client_pid,
            ) {
                Ok(data) => signed_ok(req.id, data, &auth.key),
                Err(e) => signed_error(req.id, e.to_string(), &auth.key),
            }
        }
        // Served by [`serve_watch`]: it needs the connection.
        Command::Watch => signed_error(req.id, "WATCH_NEEDS_A_CONNECTION", &auth.key),
    }
}

/// Largest request frame accepted.
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// A client connection, as far as a watch needs it (see [`crate::watch`]).
pub(crate) trait Connection: std::io::Read + std::io::Write {
    /// Prepares the connection for a watch and returns what forces it
    /// closed even while a write to it is blocked.
    fn watch_closer(&self) -> Option<crate::watch::Closer> {
        None
    }
}

#[cfg(unix)]
impl Connection for std::os::unix::net::UnixStream {
    fn watch_closer(&self) -> Option<crate::watch::Closer> {
        // A client that stopped reading cannot hold the writer forever.
        self.set_write_timeout(Some(WATCH_WRITE_TIMEOUT)).ok();
        let duplicate = self.try_clone().ok()?;
        Some(Box::new(move || {
            let _ = duplicate.shutdown(std::net::Shutdown::Both);
        }))
    }
}

#[cfg(windows)]
impl Connection for std::fs::File {
    fn watch_closer(&self) -> Option<crate::watch::Closer> {
        use std::os::windows::io::AsRawHandle;
        // A duplicate of the pipe handle: cancels a write blocked on a
        // client that stopped reading, then disconnects the instance.
        let duplicate = self.try_clone().ok()?;
        Some(Box::new(move || unsafe {
            let handle = duplicate.as_raw_handle() as winapi::shared::ntdef::HANDLE;
            winapi::um::ioapiset::CancelIoEx(handle, std::ptr::null_mut());
            winapi::um::namedpipeapi::DisconnectNamedPipe(handle);
        }))
    }
}

/// Longest a single write to a watch connection may block (Unix).
#[cfg(unix)]
const WATCH_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// One request per connection: `[u32 BE length][JSON]` in, the same out —
/// except `Watch`, which keeps the connection open (see [`serve_watch`]).
/// Uses only `read_exact`, so it works however the client split its writes.
fn serve_connection<S: Connection>(stream: &mut S, client_pid: u32) -> Result<()> {
    use anyhow::Context;
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .context("read request length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        anyhow::bail!("request too large: {len}");
    }
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .with_context(|| format!("read request body ({len} bytes)"))?;
    let req: Request = serde_json::from_slice(&buf).context("parse request")?;
    if req.command == Command::Watch {
        return serve_watch(
            stream,
            req,
            client_pid,
            &CORE,
            STOPPING.load(Ordering::SeqCst),
            crate::watch::KEEPALIVE,
        );
    }
    let resp = handle(req, client_pid)?;
    write_frame(stream, &resp)
}

fn write_frame<S: std::io::Write>(stream: &mut S, response: &Response) -> Result<()> {
    use anyhow::Context;
    let body = serde_json::to_vec(response)?;
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .context("write response length")?;
    stream.write_all(&body).context("write response body")?;
    stream.flush().ok();
    Ok(())
}

/// `Watch`: after the same checks as every request, and the owner's session
/// check under `core`, answers `watching` and then writes events until the
/// session's core stops, the service stops, or the client goes away. Holds
/// neither `core` nor the hub while it waits or writes. Every frame is a
/// [`Response`] with the request's id, signed with the session key.
fn serve_watch<S: Connection>(
    stream: &mut S,
    req: Request,
    client_pid: u32,
    core: &Mutex<crate::core::CoreManager>,
    stopping: bool,
    keepalive: std::time::Duration,
) -> Result<()> {
    use crate::watch::Event;
    use std::sync::mpsc::RecvTimeoutError;

    let auth = match admit(&req, client_pid, stopping)? {
        Admission::Authorized(auth) => auth,
        Admission::Refused(response) => return write_frame(stream, &response),
        Admission::Handshake => unreachable!("a Watch request"),
    };
    let session: SessionRef = match serde_json::from_value(req.payload.clone()) {
        Ok(session) => session,
        Err(error) => {
            let response = signed_error(req.id, format!("bad payload: {error}"), &auth.key)?;
            return write_frame(stream, &response);
        }
    };
    let closer = stream.watch_closer();
    let registration = core.lock().watch(&session, client_pid, closer);
    let registration = match registration {
        Ok(registration) => registration,
        Err(error) => {
            let response = signed_error(req.id, error.to_string(), &auth.key)?;
            return write_frame(stream, &response);
        }
    };
    let watching = serde_json::json!({
        "event": "watching",
        "keepalive_ms": keepalive.as_millis() as u64,
    });
    write_frame(stream, &signed_ok(req.id.clone(), watching, &auth.key)?)?;
    log::debug!(
        "watch opened for session {} (generation {}) by pid {client_pid}",
        session.session_id,
        session.generation
    );
    let mut seq = 0u64;
    loop {
        let (mut event, last) = match registration.events.recv_timeout(keepalive) {
            Ok(Event::Stopping) => (serde_json::json!({ "event": "stopping" }), true),
            Ok(Event::CoreStopped(reason)) => (
                serde_json::json!({ "event": "core_stopped", "reason": reason }),
                true,
            ),
            Err(RecvTimeoutError::Timeout) => (serde_json::json!({ "event": "keepalive" }), false),
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        seq += 1;
        event["seq"] = seq.into();
        let frame = signed_ok(req.id.clone(), event, &auth.key)?;
        if let Err(error) = write_frame(stream, &frame) {
            // The client went away (or stopped reading): not an error.
            log::debug!("watch of pid {client_pid} ended: {error:#}");
            return Ok(());
        }
        if last {
            return Ok(());
        }
    }
}

fn handshake(req: Request, client_pid: u32) -> Result<Response> {
    if req.auth_token.is_some() || !req.signature.is_empty() {
        return Ok(Response::unsigned(
            req.id,
            false,
            None,
            Some("HANDSHAKE_ENVELOPE_INVALID".to_string()),
        ));
    }
    let payload: HandshakePayload = match serde_json::from_value(req.payload) {
        Ok(payload) => payload,
        Err(error) => {
            return Ok(Response::unsigned(
                req.id,
                false,
                None,
                Some(format!("bad handshake payload: {error}")),
            ))
        }
    };
    if payload.client_nonce.len() < 32 || payload.client_nonce.len() > 128 {
        return Ok(Response::unsigned(
            req.id,
            false,
            None,
            Some("HANDSHAKE_NONCE_INVALID".to_string()),
        ));
    }
    let token = uuid::Uuid::new_v4().to_string();
    let mut hasher = Sha256::new();
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    hasher.update(payload.client_nonce.as_bytes());
    hasher.update(client_pid.to_le_bytes());
    hasher.update(crate::protocol::now_epoch().to_le_bytes());
    let key: [u8; 32] = hasher.finalize().into();
    let expires_at = crate::protocol::now_epoch().saturating_add(AUTH_SESSION_SECONDS);
    AUTH_SESSIONS.lock().insert(
        token.clone(),
        AuthSession {
            key,
            client_pid,
            expires_at,
        },
    );
    Ok(Response::unsigned(
        req.id,
        true,
        Some(serde_json::to_value(HandshakeResponse {
            auth_token: token,
            auth_key: hex::encode(key),
            expires_at,
        })?),
        None,
    ))
}

fn signed_ok(id: String, data: serde_json::Value, key: &[u8]) -> Result<Response> {
    Response::signed_with(id, true, Some(data), None, key)
}

fn signed_error(id: String, error: impl Into<String>, key: &[u8]) -> Result<Response> {
    Response::signed_with(id, false, None, Some(error.into()), key)
}

#[cfg(windows)]
pub async fn run_ipc_server() -> Result<()> {
    use log::{error, info};
    use std::ffi::OsStr;
    use std::fs::File;
    use std::mem;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::ptr;
    use tokio::task::spawn_blocking;
    use winapi::shared::winerror::ERROR_PIPE_CONNECTED;
    use winapi::um::accctrl::{
        EXPLICIT_ACCESS_W, SET_ACCESS, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
    };
    use winapi::um::aclapi::SetEntriesInAclW;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
    use winapi::um::minwinbase::SECURITY_ATTRIBUTES;
    use winapi::um::namedpipeapi::{ConnectNamedPipe, CreateNamedPipeW};
    use winapi::um::securitybaseapi::{
        AllocateAndInitializeSid, FreeSid, InitializeSecurityDescriptor, SetSecurityDescriptorDacl,
    };
    use winapi::um::winbase::{
        LocalFree, PIPE_ACCESS_DUPLEX, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use winapi::um::winnt::{
        GENERIC_ALL, PSID, SECURITY_AUTHENTICATED_USER_RID, SECURITY_DESCRIPTOR,
        SECURITY_DESCRIPTOR_REVISION, SECURITY_NT_AUTHORITY, SID_IDENTIFIER_AUTHORITY,
    };

    info!("IPC server listening on {IPC_PIPE_NAME}");

    loop {
        let pipe_handle = unsafe {
            let mut sd: SECURITY_DESCRIPTOR = mem::zeroed();
            let mut authenticated_users_sid: PSID = ptr::null_mut();
            let mut acl = ptr::null_mut();

            if InitializeSecurityDescriptor(
                &mut sd as *mut SECURITY_DESCRIPTOR as *mut _,
                SECURITY_DESCRIPTOR_REVISION,
            ) == 0
            {
                error!("InitializeSecurityDescriptor failed: {}", GetLastError());
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }

            let mut sia = SID_IDENTIFIER_AUTHORITY {
                Value: SECURITY_NT_AUTHORITY,
            };

            if AllocateAndInitializeSid(
                &mut sia as *mut SID_IDENTIFIER_AUTHORITY,
                1,
                SECURITY_AUTHENTICATED_USER_RID,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                &mut authenticated_users_sid,
            ) == 0
            {
                error!("AllocateAndInitializeSid failed: {}", GetLastError());
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }

            let mut ea = EXPLICIT_ACCESS_W {
                grfAccessPermissions: GENERIC_ALL,
                grfAccessMode: SET_ACCESS,
                grfInheritance: 0,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                    ptstrName: authenticated_users_sid as *mut _,
                },
            };

            if SetEntriesInAclW(
                1,
                &mut ea as *mut EXPLICIT_ACCESS_W,
                ptr::null_mut(),
                &mut acl,
            ) != 0
            {
                error!("SetEntriesInAclW failed");
                FreeSid(authenticated_users_sid);
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }

            if SetSecurityDescriptorDacl(&mut sd as *mut SECURITY_DESCRIPTOR as *mut _, 1, acl, 0)
                == 0
            {
                error!("SetSecurityDescriptorDacl failed: {}", GetLastError());
                LocalFree(acl as *mut _);
                FreeSid(authenticated_users_sid);
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }

            let mut sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: &mut sd as *mut SECURITY_DESCRIPTOR as *mut _,
                bInheritHandle: 0,
            };

            let wide: Vec<u16> = OsStr::new(IPC_PIPE_NAME)
                .encode_wide()
                .chain(Some(0))
                .collect();

            let handle = CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                // Byte mode: a frame's length and body may arrive in one
                // write or two, and reads may return partial data; the
                // framing below does not depend on message boundaries.
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                &mut sa,
            );

            if !acl.is_null() {
                LocalFree(acl as *mut _);
            }
            if !authenticated_users_sid.is_null() {
                FreeSid(authenticated_users_sid);
            }
            handle
        };

        if pipe_handle == INVALID_HANDLE_VALUE {
            let err = unsafe { GetLastError() };
            error!("CreateNamedPipeW failed: {err}");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            continue;
        }

        let connect = unsafe { ConnectNamedPipe(pipe_handle, ptr::null_mut()) };
        let last_err = unsafe { GetLastError() };
        if connect == 0 && last_err != ERROR_PIPE_CONNECTED {
            error!("ConnectNamedPipe failed: {last_err}");
            unsafe { CloseHandle(pipe_handle) };
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            continue;
        }

        let client_pid = match validate_windows_client(pipe_handle) {
            Ok(client_pid) => client_pid,
            Err(error) => {
                error!("reject named-pipe client: {error}");
                unsafe { CloseHandle(pipe_handle) };
                continue;
            }
        };

        let mut pipe = unsafe { File::from_raw_handle(pipe_handle as _) };

        spawn_blocking(move || {
            if let Err(error) = serve_connection(&mut pipe, client_pid) {
                error!("IPC request from pid {client_pid} failed (protocol): {error:#}");
            }
        });
    }
}

#[cfg(windows)]
fn validate_windows_client(pipe_handle: winapi::shared::ntdef::HANDLE) -> Result<u32> {
    use anyhow::{anyhow, Context};
    use std::path::PathBuf;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::{OpenProcess, ProcessIdToSessionId};
    use winapi::um::winbase::{GetNamedPipeClientProcessId, QueryFullProcessImageNameW};
    use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;

    let mut process_id = 0u32;
    if unsafe { GetNamedPipeClientProcessId(pipe_handle, &mut process_id) } == 0 || process_id == 0
    {
        return Err(anyhow!("CLIENT_PID_UNAVAILABLE"));
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return Err(anyhow!("CLIENT_PROCESS_UNAVAILABLE"));
    }
    let mut session_id = 0u32;
    let session_ok = unsafe { ProcessIdToSessionId(process_id, &mut session_id) } != 0;
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    let query_ok =
        unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) } != 0;
    unsafe { CloseHandle(process) };
    if !session_ok || !query_ok || length == 0 {
        return Err(anyhow!("CLIENT_IDENTITY_UNAVAILABLE"));
    }

    let image = PathBuf::from(String::from_utf16(&buffer[..length as usize])?);
    let image_name = image
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(image_name.as_str(), "ppvpn.exe" | "ppvpn-desktop.exe") {
        return Err(anyhow!("CLIENT_IMAGE_NOT_ALLOWED"));
    }
    let image_parent = image
        .parent()
        .ok_or_else(|| anyhow!("CLIENT_IMAGE_DIRECTORY_INVALID"))?
        .canonicalize()
        .context("resolve client image directory")?;
    let service_parent = std::env::current_exe()
        .context("resolve service image")?
        .parent()
        .ok_or_else(|| anyhow!("SERVICE_IMAGE_DIRECTORY_INVALID"))?
        .canonicalize()
        .context("resolve service image directory")?;
    if image_parent != service_parent {
        return Err(anyhow!("CLIENT_IMAGE_OUTSIDE_INSTALL_DIRECTORY"));
    }
    if allow_unsigned_windows_client() {
        // Temporary web-distribution mode: Program Files remains admin-write
        // protected, and the client must still have the exact executable name,
        // share the service install directory, connect locally, and complete
        // the PID-bound short-lived HMAC session handshake.
        static UNSIGNED_MODE_WARNING: std::sync::Once = std::sync::Once::new();
        UNSIGNED_MODE_WARNING.call_once(|| {
            log::warn!(
                "Windows Authenticode enforcement is disabled at build time; \
                 accepting the expected client only from the protected install directory"
            );
        });
    } else {
        verify_authenticode(&image)?;
    }
    log::debug!("accepted named-pipe client pid={process_id} session={session_id}");
    Ok(process_id)
}

#[cfg(windows)]
fn verify_authenticode(path: &std::path::Path) -> Result<()> {
    use anyhow::anyhow;
    use std::ffi::OsStr;
    use std::mem;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use winapi::shared::guiddef::GUID;
    use winapi::um::wintrust::{
        WinVerifyTrust, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL,
        WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    let wide: Vec<u16> = OsStr::new(path.as_os_str())
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut file_info: WINTRUST_FILE_INFO = unsafe { mem::zeroed() };
    file_info.cbStruct = mem::size_of::<WINTRUST_FILE_INFO>() as u32;
    file_info.pcwszFilePath = wide.as_ptr();
    file_info.hFile = ptr::null_mut();
    file_info.pgKnownSubject = ptr::null();

    let mut data: WINTRUST_DATA = unsafe { mem::zeroed() };
    data.cbStruct = mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    // Runtime IPC must remain available offline. Installation/update performs
    // the online revocation gate; here we validate the embedded signature and
    // trusted chain without allowing a 15-second lease renewal to fetch URLs.
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    unsafe {
        *data.u.pFile_mut() = &mut file_info;
    }
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    data.dwProvFlags = WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL;

    // WINTRUST_ACTION_GENERIC_VERIFY_V2. Keep a local GUID so no mutable
    // process-global from the Windows headers is borrowed.
    let mut action = GUID {
        Data1: 0x00aac56b,
        Data2: 0xcd44,
        Data3: 0x11d0,
        Data4: [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee],
    };
    let status = unsafe {
        WinVerifyTrust(
            ptr::null_mut(),
            &mut action,
            &mut data as *mut WINTRUST_DATA as *mut _,
        )
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(
            ptr::null_mut(),
            &mut action,
            &mut data as *mut WINTRUST_DATA as *mut _,
        );
    }
    if status != 0 {
        return Err(anyhow!("CLIENT_AUTHENTICODE_INVALID:{status:#x}"));
    }
    verify_publisher_certificate(path)
}

#[cfg(windows)]
fn verify_publisher_certificate(path: &std::path::Path) -> Result<()> {
    use anyhow::anyhow;
    use std::ffi::OsStr;
    use std::mem;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use winapi::ctypes::c_void;
    use winapi::um::wincrypt::{
        CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext,
        CertGetCertificateContextProperty, CryptMsgClose, CryptMsgGetParam, CryptQueryObject,
        CERT_FIND_SUBJECT_CERT, CERT_INFO, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
        CERT_QUERY_FORMAT_FLAG_BINARY, CERT_QUERY_OBJECT_FILE, CERT_SHA256_HASH_PROP_ID,
        CMSG_SIGNER_INFO, CMSG_SIGNER_INFO_PARAM, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    };

    let expected = decode_publisher_pins(&[
        option_env!("PPVPN_WINDOWS_PUBLISHER_SHA256"),
        option_env!("PPVPN_WINDOWS_NEXT_PUBLISHER_SHA256"),
    ])
    .map_err(|code| anyhow!(code))?;
    if expected.is_empty() {
        if cfg!(debug_assertions) {
            log::warn!(
                "publisher pin is not configured; trusted-signature-only mode is for debug builds"
            );
            return Ok(());
        }
        return Err(anyhow!("CLIENT_PUBLISHER_PIN_NOT_CONFIGURED"));
    }

    let wide: Vec<u16> = OsStr::new(path.as_os_str())
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut store = ptr::null_mut();
    let mut message = ptr::null_mut();
    let queried = unsafe {
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            wide.as_ptr().cast(),
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut store,
            &mut message,
            ptr::null_mut(),
        )
    };
    if queried == 0 || store.is_null() || message.is_null() {
        if !message.is_null() {
            unsafe { CryptMsgClose(message) };
        }
        if !store.is_null() {
            unsafe { CertCloseStore(store, 0) };
        }
        return Err(anyhow!("CLIENT_PUBLISHER_CERTIFICATE_UNAVAILABLE"));
    }

    let result = (|| -> Result<()> {
        let mut signer_bytes = 0u32;
        if unsafe {
            CryptMsgGetParam(
                message,
                CMSG_SIGNER_INFO_PARAM,
                0,
                ptr::null_mut(),
                &mut signer_bytes,
            )
        } == 0
            || signer_bytes < mem::size_of::<CMSG_SIGNER_INFO>() as u32
        {
            return Err(anyhow!("CLIENT_PUBLISHER_SIGNER_INFO_UNAVAILABLE"));
        }

        // Use usize storage so the CMSG_SIGNER_INFO pointer is suitably
        // aligned while CryptoAPI fills the byte-sized output buffer.
        let words = (signer_bytes as usize).div_ceil(mem::size_of::<usize>());
        let mut signer_storage = vec![0usize; words];
        if unsafe {
            CryptMsgGetParam(
                message,
                CMSG_SIGNER_INFO_PARAM,
                0,
                signer_storage.as_mut_ptr().cast(),
                &mut signer_bytes,
            )
        } == 0
        {
            return Err(anyhow!("CLIENT_PUBLISHER_SIGNER_INFO_UNAVAILABLE"));
        }
        let signer = unsafe { &*(signer_storage.as_ptr().cast::<CMSG_SIGNER_INFO>()) };
        let mut certificate_info: CERT_INFO = unsafe { mem::zeroed() };
        certificate_info.Issuer = signer.Issuer;
        certificate_info.SerialNumber = signer.SerialNumber;
        let certificate = unsafe {
            CertFindCertificateInStore(
                store,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                0,
                CERT_FIND_SUBJECT_CERT,
                (&certificate_info as *const CERT_INFO).cast::<c_void>(),
                ptr::null(),
            )
        };
        if certificate.is_null() {
            return Err(anyhow!("CLIENT_PUBLISHER_CERTIFICATE_UNAVAILABLE"));
        }

        let hash_result = (|| -> Result<Vec<u8>> {
            let mut hash_bytes = 0u32;
            if unsafe {
                CertGetCertificateContextProperty(
                    certificate,
                    CERT_SHA256_HASH_PROP_ID,
                    ptr::null_mut(),
                    &mut hash_bytes,
                )
            } == 0
                || hash_bytes != 32
            {
                return Err(anyhow!("CLIENT_PUBLISHER_HASH_UNAVAILABLE"));
            }
            let mut hash = vec![0u8; hash_bytes as usize];
            if unsafe {
                CertGetCertificateContextProperty(
                    certificate,
                    CERT_SHA256_HASH_PROP_ID,
                    hash.as_mut_ptr().cast(),
                    &mut hash_bytes,
                )
            } == 0
            {
                return Err(anyhow!("CLIENT_PUBLISHER_HASH_UNAVAILABLE"));
            }
            Ok(hash)
        })();
        unsafe { CertFreeCertificateContext(certificate) };
        let actual = hash_result?;
        if !expected
            .iter()
            .any(|allowed| actual.as_slice() == allowed.as_slice())
        {
            return Err(anyhow!("CLIENT_PUBLISHER_MISMATCH"));
        }
        Ok(())
    })();

    unsafe {
        CryptMsgClose(message);
        CertCloseStore(store, 0);
    }
    result
}

#[cfg(unix)]
pub async fn run_ipc_server() -> Result<()> {
    use crate::protocol::IPC_SOCKET_PATH;
    use anyhow::Context;
    use log::{error, info};
    use std::os::unix::net::UnixListener;
    use tokio::task::spawn_blocking;

    #[cfg(target_os = "macos")]
    prepare_macos_socket_directory()?;
    #[cfg(target_os = "linux")]
    prepare_linux_socket_directory()?;
    // Remove stale socket if previous service crashed without cleanup.
    let _ = std::fs::remove_file(IPC_SOCKET_PATH);

    let listener =
        UnixListener::bind(IPC_SOCKET_PATH).with_context(|| format!("bind {IPC_SOCKET_PATH}"))?;

    // The socket is connectable by local users, but every accepted connection
    // is bound to the peer PID and must originate from the fixed PPVPN app
    // executable path before it can obtain a short-lived HMAC session.
    set_socket_permissions(IPC_SOCKET_PATH);

    info!("IPC server listening on {IPC_SOCKET_PATH}");

    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                #[cfg(target_os = "macos")]
                let client_pid = match validate_macos_client(&stream) {
                    Ok(pid) => pid,
                    Err(error) => {
                        error!("reject unix-socket client: {error}");
                        continue;
                    }
                };
                #[cfg(target_os = "linux")]
                let client_pid = match validate_linux_client(&stream) {
                    Ok(pid) => pid,
                    Err(error) => {
                        error!("reject unix-socket client: {error}");
                        continue;
                    }
                };
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                let client_pid = 0;
                spawn_blocking(move || {
                    if let Err(error) = serve_connection(&mut stream, client_pid) {
                        error!("IPC request from pid {client_pid} failed (protocol): {error:#}");
                    }
                });
            }
            Err(e) => {
                error!("accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn prepare_macos_socket_directory() -> Result<()> {
    use anyhow::{anyhow, Context};
    use std::path::Path;
    use std::process::Command;

    let directory = Path::new(crate::protocol::IPC_SOCKET_PATH)
        .parent()
        .ok_or_else(|| anyhow!("SERVICE_SOCKET_DIRECTORY_INVALID"))?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("create {}", directory.display()))?;
    let base = directory
        .parent()
        .ok_or_else(|| anyhow!("SERVICE_SOCKET_BASE_DIRECTORY_INVALID"))?;
    for path in [base, directory] {
        let path_text = path.display().to_string();
        for (command, mode) in [("chown", "root:wheel"), ("chmod", "755")] {
            let status = Command::new(command)
                .args([mode, path_text.as_str()])
                .status()
                .with_context(|| format!("run {command}"))?;
            if !status.success() {
                return Err(anyhow!("SERVICE_SOCKET_DIRECTORY_PERMISSIONS_FAILED"));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_macos_client(stream: &std::os::unix::net::UnixStream) -> Result<u32> {
    use anyhow::{anyhow, Context};
    use std::ffi::CStr;
    use std::os::fd::AsRawFd;
    use std::path::Path;

    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if result != 0 || pid <= 0 {
        return Err(anyhow!("CLIENT_PID_UNAVAILABLE"));
    }
    let mut buffer = vec![0i8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let written = unsafe {
        libc::proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast(),
            libc::PROC_PIDPATHINFO_MAXSIZE as u32,
        )
    };
    if written <= 0 {
        return Err(anyhow!("CLIENT_PROCESS_PATH_UNAVAILABLE"));
    }
    let path = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_str()
        .context("decode client process path")?;
    if !macos_client_path_allowed(Path::new(path), cfg!(debug_assertions)) {
        return Err(anyhow!("CLIENT_IMAGE_NOT_ALLOWED:{path}"));
    }
    Ok(pid as u32)
}

#[cfg(any(target_os = "macos", test))]
fn macos_client_path_allowed(path: &std::path::Path, debug_build: bool) -> bool {
    let normalized = path.to_string_lossy();
    if matches!(
        normalized.as_ref(),
        "/Applications/PPVPN.app/Contents/MacOS/PPVPN"
            | "/Applications/PPVPN（开发版）.app/Contents/MacOS/PPVPN"
    ) {
        return true;
    }
    debug_build
        && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("ppvpn-desktop" | "PPVPN")
        )
        && normalized.contains("/ppvpn-desktop/src-tauri/target/")
}

#[cfg(target_os = "macos")]
fn set_socket_permissions(path: &str) {
    use std::process::Command;

    let _ = Command::new("chown").args(["root:wheel", path]).status();
    let _ = Command::new("chmod").args(["666", path]).status();
}

/// systemd creates /run/ppvpn (RuntimeDirectory=ppvpn, mode 0755); this only
/// covers a service started by hand.
#[cfg(target_os = "linux")]
fn prepare_linux_socket_directory() -> Result<()> {
    use anyhow::{anyhow, Context};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    let directory = Path::new(crate::protocol::IPC_SOCKET_PATH)
        .parent()
        .ok_or_else(|| anyhow!("SERVICE_SOCKET_DIRECTORY_INVALID"))?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("create {}", directory.display()))?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("chmod {}", directory.display()))?;
    Ok(())
}

/// Same policy as macOS: the peer PID must be running the installed PPVPN
/// executable, which only root can replace.
#[cfg(target_os = "linux")]
fn validate_linux_client(stream: &std::os::unix::net::UnixStream) -> Result<u32> {
    use anyhow::anyhow;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 || credentials.pid <= 0 {
        return Err(anyhow!("CLIENT_PID_UNAVAILABLE"));
    }
    let pid = credentials.pid;
    let exe = format!("/proc/{pid}/exe");
    let link = std::fs::read_link(&exe).map_err(|_| anyhow!("CLIENT_PROCESS_PATH_UNAVAILABLE"))?;
    // After a package upgrade the running app still executes the replaced
    // binary, which reads back as "<path> (deleted)": judge the path it was
    // started from, and ownership by the inode it actually runs.
    let path = linux_client_image(&link);
    if !linux_client_path_allowed(&path, cfg!(debug_assertions)) {
        return Err(anyhow!("CLIENT_IMAGE_NOT_ALLOWED:{}", link.display()));
    }
    if !cfg!(debug_assertions) {
        // /proc/<pid>/exe resolves to the executing inode even when deleted.
        let metadata = std::fs::metadata(&exe)
            .map_err(|_| anyhow!("CLIENT_IMAGE_NOT_ALLOWED:{}", path.display()))?;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(anyhow!("CLIENT_IMAGE_NOT_ROOT_OWNED:{}", path.display()));
        }
    }
    Ok(pid as u32)
}

/// The path a client was started from: `/proc/<pid>/exe` of a binary that was
/// replaced since (a package upgrade) reads back with a " (deleted)" suffix.
#[cfg(any(target_os = "linux", test))]
fn linux_client_image(link: &std::path::Path) -> std::path::PathBuf {
    let text = link.to_string_lossy();
    match text.strip_suffix(" (deleted)") {
        Some(original) => std::path::PathBuf::from(original),
        None => link.to_path_buf(),
    }
}

/// Release: the deb/rpm install location. Debug: an `apps/linux` build output.
#[cfg(any(target_os = "linux", test))]
fn linux_client_path_allowed(path: &std::path::Path, debug_build: bool) -> bool {
    let normalized = path.to_string_lossy();
    if normalized == "/usr/lib/ppvpn/ppvpn" {
        return true;
    }
    debug_build
        && path.file_name().and_then(|name| name.to_str()) == Some("ppvpn")
        && normalized.contains("/apps/linux/")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn set_socket_permissions(path: &str) {
    use std::{fs, os::unix::fs::PermissionsExt};

    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o666);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_publisher_pins, handle, handle_with, linux_client_image, linux_client_path_allowed,
        macos_client_path_allowed, parse_allow_unsigned_client, ReplayWindow, SERVICE_STOPPING,
    };
    use crate::protocol::{Command, HandshakeResponse, Request, VersionInfo};

    /// In-memory connection: hands the request out `chunk` bytes per read
    /// (like a byte-mode pipe or socket returning partial data) and records
    /// what the server writes.
    struct FakeStream {
        input: Vec<u8>,
        position: usize,
        chunk: usize,
        output: Vec<u8>,
    }

    impl std::io::Read for FakeStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let end = (self.position + self.chunk.min(buf.len())).min(self.input.len());
            let read = end - self.position;
            buf[..read].copy_from_slice(&self.input[self.position..end]);
            self.position = end;
            Ok(read)
        }
    }

    impl super::Connection for FakeStream {}

    impl std::io::Write for FakeStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn handshake_frame() -> Vec<u8> {
        let request = Request {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: crate::protocol::now_epoch(),
            command: Command::Handshake,
            payload: serde_json::json!({
                "client_nonce": uuid::Uuid::new_v4().simple().to_string(),
            }),
            auth_token: None,
            signature: String::new(),
        };
        let body = serde_json::to_vec(&request).unwrap();
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        frame
    }

    #[test]
    fn framing_works_whatever_the_read_sizes() {
        // One combined write read back whole, byte by byte, or in odd chunks.
        for chunk in [usize::MAX, 1, 3, 4096] {
            let mut stream = FakeStream {
                input: handshake_frame(),
                position: 0,
                chunk,
                output: Vec::new(),
            };
            super::serve_connection(&mut stream, 4242).unwrap();
            let length = u32::from_be_bytes(stream.output[..4].try_into().unwrap()) as usize;
            assert_eq!(stream.output.len(), 4 + length, "chunk {chunk}");
            let response: crate::protocol::Response =
                serde_json::from_slice(&stream.output[4..]).unwrap();
            assert!(response.success, "chunk {chunk}");
        }
    }

    #[test]
    fn truncated_frames_are_protocol_errors() {
        let frame = handshake_frame();
        for cut in [2, 4, frame.len() - 1] {
            let mut stream = FakeStream {
                input: frame[..cut].to_vec(),
                position: 0,
                chunk: usize::MAX,
                output: Vec::new(),
            };
            let error = super::serve_connection(&mut stream, 4242).unwrap_err();
            assert!(
                format!("{error:#}").contains("read request"),
                "cut {cut}: {error:#}"
            );
            assert!(stream.output.is_empty());
        }
        let mut huge = FakeStream {
            input: (64 * 1024 * 1024u32).to_be_bytes().to_vec(),
            position: 0,
            chunk: usize::MAX,
            output: Vec::new(),
        };
        assert!(
            format!("{:#}", super::serve_connection(&mut huge, 1).unwrap_err())
                .contains("too large")
        );
    }

    #[test]
    fn replay_window_rejects_duplicate_request_ids() {
        let mut window = ReplayWindow::default();
        assert!(window.record_once("request-1", 100, 100));
        assert!(!window.record_once("request-1", 100, 101));
    }

    #[test]
    fn replay_window_expires_old_request_ids() {
        let mut window = ReplayWindow::default();
        assert!(window.record_once("request-1", 100, 100));
        assert!(window.record_once("request-1", 200, 200));
    }

    #[test]
    fn publisher_pin_decoder_accepts_rotation_pair() {
        let primary = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let next = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        let pins = decode_publisher_pins(&[Some(primary), Some(next)]).unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].len(), 32);
        assert_eq!(pins[1], vec![0xff; 32]);
    }

    #[test]
    fn publisher_pin_decoder_rejects_non_sha256_values() {
        assert_eq!(
            decode_publisher_pins(&[Some("abcd")]),
            Err("CLIENT_PUBLISHER_PIN_INVALID")
        );
        assert_eq!(
            decode_publisher_pins(&[Some(
                "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            )]),
            Err("CLIENT_PUBLISHER_PIN_INVALID")
        );
    }

    #[test]
    fn unsigned_client_mode_requires_an_explicit_truthy_build_value() {
        assert!(!parse_allow_unsigned_client(None));
        assert!(!parse_allow_unsigned_client(Some("0")));
        assert!(!parse_allow_unsigned_client(Some("false")));
        assert!(parse_allow_unsigned_client(Some("1")));
        assert!(parse_allow_unsigned_client(Some("true")));
    }

    #[test]
    fn handshake_issues_pid_bound_short_lived_key() {
        let handshake_id = uuid::Uuid::new_v4().to_string();
        let handshake = Request {
            id: handshake_id.clone(),
            timestamp: crate::protocol::now_epoch(),
            command: Command::Handshake,
            payload: serde_json::json!({
                "client_nonce": uuid::Uuid::new_v4().simple().to_string(),
            }),
            auth_token: None,
            signature: String::new(),
        };
        let response = handle(handshake, 4242).unwrap();
        assert!(response.success);
        assert!(response.signature.is_empty());
        assert_eq!(response.id, handshake_id);
        let auth: HandshakeResponse = serde_json::from_value(response.data.unwrap()).unwrap();
        assert!(auth.expires_at > crate::protocol::now_epoch());
        let key = hex::decode(auth.auth_key).unwrap();
        assert_eq!(key.len(), 32);

        let authenticated = Request::signed(
            uuid::Uuid::new_v4().to_string(),
            Command::GetVersion,
            serde_json::json!({}),
            Some(auth.auth_token.clone()),
            &key,
        )
        .unwrap();
        let accepted = handle(authenticated, 4242).unwrap();
        assert!(accepted.success);
        assert!(accepted.verify_with(&key).unwrap());
        let version: VersionInfo = serde_json::from_value(accepted.data.clone().unwrap()).unwrap();
        assert_eq!(version.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(version.build_id, crate::protocol::SERVICE_BUILD_ID);

        let wrong_process = Request::signed(
            uuid::Uuid::new_v4().to_string(),
            Command::GetVersion,
            serde_json::json!({}),
            Some(auth.auth_token),
            &key,
        )
        .unwrap();
        let rejected = handle(wrong_process, 7777).unwrap();
        assert!(!rejected.success);
        assert_eq!(rejected.error.as_deref(), Some("AUTH_SESSION_INVALID"));
        assert!(rejected.signature.is_empty());
    }

    /// Handshake for `pid`; returns (token, key).
    fn authenticate(pid: u32) -> (String, Vec<u8>) {
        let handshake = Request {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: crate::protocol::now_epoch(),
            command: Command::Handshake,
            payload: serde_json::json!({
                "client_nonce": uuid::Uuid::new_v4().simple().to_string(),
            }),
            auth_token: None,
            signature: String::new(),
        };
        let response = handle(handshake, pid).unwrap();
        let auth: HandshakeResponse = serde_json::from_value(response.data.unwrap()).unwrap();
        (auth.auth_token, hex::decode(auth.auth_key).unwrap())
    }

    #[test]
    fn a_stopping_service_refuses_connect_and_session_calls() {
        let pid = 5151;
        let (token, key) = authenticate(pid);
        let session = serde_json::json!({"session_id": "s-stopping", "generation": 1});
        let connect = serde_json::json!({
            "session_id": "s-stopping", "generation": 1,
            "profile": {"revision": "r1", "schema_version": 1}, "take_over": true,
        });
        let core_api = serde_json::json!({
            "session_id": "s-stopping", "generation": 1, "path": "/v1/get-status", "body": {},
        });
        for (command, payload) in [
            (Command::Connect, connect.clone()),
            (Command::UpdateProfile, connect),
            (Command::RenewLease, session.clone()),
            (Command::CoreApi, core_api),
        ] {
            let request = Request::signed(
                uuid::Uuid::new_v4().to_string(),
                command.clone(),
                payload,
                Some(token.clone()),
                &key,
            )
            .unwrap();
            let response = handle_with(request, pid, true).unwrap();
            assert!(!response.success, "{command:?}");
            assert_eq!(
                response.error.as_deref(),
                Some(SERVICE_STOPPING),
                "{command:?}"
            );
            assert!(response.verify_with(&key).unwrap(), "signed: {command:?}");
        }
        // Version, status and disconnect still answer.
        for (command, payload) in [
            (Command::GetVersion, serde_json::json!({})),
            (Command::GetStatus, serde_json::json!({})),
            (Command::Disconnect, session),
        ] {
            let request = Request::signed(
                uuid::Uuid::new_v4().to_string(),
                command.clone(),
                payload,
                Some(token.clone()),
                &key,
            )
            .unwrap();
            let response = handle_with(request, pid, true).unwrap();
            assert_ne!(
                response.error.as_deref(),
                Some(SERVICE_STOPPING),
                "{command:?}"
            );
        }
    }

    // --- watch connections --------------------------------------------------

    /// Reads one frame; `None` at EOF.
    #[cfg(unix)]
    fn read_frame(
        stream: &mut std::os::unix::net::UnixStream,
    ) -> Option<crate::protocol::Response> {
        use std::io::Read;
        let mut length = [0u8; 4];
        match stream.read(&mut length[..1]) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        stream.read_exact(&mut length[1..]).unwrap();
        let mut body = vec![0u8; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).unwrap();
        Some(serde_json::from_slice(&body).unwrap())
    }

    /// A signed `Watch` for `session` and the key its answers are signed with.
    #[cfg(unix)]
    fn watch_request(pid: u32, session: &crate::protocol::SessionRef) -> (Request, Vec<u8>) {
        let (token, key) = authenticate(pid);
        let request = Request::signed(
            uuid::Uuid::new_v4().to_string(),
            Command::Watch,
            serde_json::to_value(session).unwrap(),
            Some(token),
            &key,
        )
        .unwrap();
        (request, key)
    }

    /// Serves `request` on one end of a socket pair in a thread; returns the
    /// client end, with the `watching` answer already read and checked.
    #[cfg(unix)]
    fn open_watch<'scope>(
        scope: &'scope std::thread::Scope<'scope, '_>,
        core: &'scope parking_lot::Mutex<crate::core::CoreManager>,
        request: Request,
        key: &[u8],
        keepalive: std::time::Duration,
    ) -> std::os::unix::net::UnixStream {
        let (mut server, mut client) = std::os::unix::net::UnixStream::pair().unwrap();
        let id = request.id.clone();
        let pid = std::process::id();
        scope.spawn(move || {
            super::serve_watch(&mut server, request, pid, core, false, keepalive).unwrap();
        });
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let watching = read_frame(&mut client).expect("an answer");
        assert!(watching.success, "{:?}", watching.error);
        assert_eq!(watching.id, id);
        assert!(watching.verify_with(key).unwrap());
        let data = watching.data.unwrap();
        assert_eq!(data["event"], "watching");
        assert_eq!(data["keepalive_ms"], keepalive.as_millis() as u64);
        client
    }

    #[cfg(unix)]
    #[test]
    fn a_watcher_hears_stopping_then_eof_and_does_not_delay_the_stop() {
        use crate::core::tests::{connect_payload, fake_manager, new_session, process_exists};
        use std::time::{Duration, Instant};

        let (manager, _cores) = fake_manager(vec![], Duration::from_secs(5));
        let hub = manager.watchers();
        let core = parking_lot::Mutex::new(manager);
        let me = std::process::id();
        let session = new_session(1);
        let pid = core.lock().connect(connect_payload(&session), me).unwrap();
        let (request, key) = watch_request(me, &session);

        std::thread::scope(|scope| {
            let mut client = open_watch(scope, &core, request, &key, Duration::from_millis(40));
            // Idle: keepalives, numbered.
            for seq in 1..=2u64 {
                let frame = read_frame(&mut client).unwrap();
                assert!(frame.verify_with(&key).unwrap());
                let data = frame.data.unwrap();
                assert_eq!(
                    (data["event"].as_str(), data["seq"].as_u64()),
                    (Some("keepalive"), Some(seq))
                );
            }
            // A second watcher whose client never reads: its writer never
            // finishes on its own (what a blocked write looks like).
            let stuck = hub
                .register(session.clone(), Some(Box::new(|| {})))
                .unwrap();

            // SIGTERM path: stopping (Connect & co. refused), watchers told
            // and closed, then the core stopped.
            let started = Instant::now();
            crate::core::stop_sequence(&core, &hub, crate::watch::CLOSE_GRACE)
                .stop_because(crate::core::stop_reason::SERVICE_STOPPING)
                .unwrap();
            let took = started.elapsed();
            assert!(took < Duration::from_secs(2), "stop took {took:?}");
            assert!(!process_exists(pid), "core stopped");

            let mut events = Vec::new();
            while let Some(frame) = read_frame(&mut client) {
                assert!(frame.verify_with(&key).unwrap());
                events.push(frame.data.unwrap()["event"].as_str().unwrap().to_string());
            }
            // Keepalives may precede it; `stopping` is last, then EOF.
            assert_eq!(
                events.last().map(String::as_str),
                Some("stopping"),
                "{events:?}"
            );
            assert!(
                !events.iter().any(|event| event == "core_stopped"),
                "{events:?}"
            );
            drop(stuck);
        });
        // A watch opened after the stop began is refused.
        assert_eq!(
            hub.register(session, None).err(),
            Some(super::SERVICE_STOPPING)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_watcher_hears_core_stopped_when_its_core_dies() {
        use crate::core::tests::{connect_payload, fake_manager, new_session};
        use std::time::{Duration, Instant};

        let (manager, _cores) = fake_manager(vec![], Duration::from_secs(5));
        let core = parking_lot::Mutex::new(manager);
        let me = std::process::id();
        let session = new_session(1);
        let pid = core.lock().connect(connect_payload(&session), me).unwrap();
        let (request, key) = watch_request(me, &session);
        let done = std::sync::atomic::AtomicBool::new(false);

        std::thread::scope(|scope| {
            let mut client = open_watch(scope, &core, request, &key, Duration::from_secs(5));
            // The lease watchdog, at a test pace.
            let (core, done_ref) = (&core, &done);
            scope.spawn(move || {
                while !done_ref.load(std::sync::atomic::Ordering::SeqCst) {
                    core.lock().reap_exited();
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            // The core crashes.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            let started = Instant::now();
            let frame = read_frame(&mut client).unwrap();
            assert!(started.elapsed() < Duration::from_secs(1));
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(frame.verify_with(&key).unwrap());
            let data = frame.data.unwrap();
            assert_eq!(data["event"], "core_stopped");
            assert_eq!(data["reason"], crate::core::stop_reason::EXITED);
            assert!(read_frame(&mut client).is_none(), "then EOF");
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_watch_needs_the_owners_live_session() {
        use crate::core::tests::{connect_payload, fake_manager, new_session};
        use std::time::Duration;

        let (manager, _cores) = fake_manager(vec![], Duration::from_secs(5));
        let core = parking_lot::Mutex::new(manager);
        let me = std::process::id();
        let session = new_session(2);
        core.lock().connect(connect_payload(&session), me).unwrap();
        let answer = |request: Request, stopping: bool| {
            let (mut server, mut client) = std::os::unix::net::UnixStream::pair().unwrap();
            super::serve_watch(
                &mut server,
                request,
                me,
                &core,
                stopping,
                Duration::from_secs(5),
            )
            .unwrap();
            drop(server);
            let frame = read_frame(&mut client).unwrap();
            assert!(
                read_frame(&mut client).is_none(),
                "closed after the refusal"
            );
            frame
        };
        let stale = crate::protocol::SessionRef {
            generation: 1,
            ..session.clone()
        };
        let (request, key) = watch_request(me, &stale);
        let refused = answer(request, false);
        assert_eq!(refused.error.as_deref(), Some("STALE_OR_FOREIGN_SESSION"));
        assert!(refused.verify_with(&key).unwrap());
        let (request, _) = watch_request(me, &session);
        assert_eq!(
            answer(request, true).error.as_deref(),
            Some(SERVICE_STOPPING)
        );
        core.lock().stop().unwrap();
    }

    #[test]
    fn macos_client_path_requires_the_app_bundle_outside_debug_builds() {
        use std::path::Path;

        assert!(macos_client_path_allowed(
            Path::new("/Applications/PPVPN.app/Contents/MacOS/PPVPN"),
            false
        ));
        assert!(!macos_client_path_allowed(
            Path::new("/tmp/PPVPN.app/Contents/MacOS/PPVPN"),
            false
        ));
        assert!(macos_client_path_allowed(
            Path::new(
                "/Volumes/dev/src/Projects/ppvpn-desktop/src-tauri/target/debug/ppvpn-desktop"
            ),
            true
        ));
    }

    #[test]
    fn linux_client_path_requires_the_package_install_outside_debug_builds() {
        use std::path::Path;

        assert!(linux_client_path_allowed(
            Path::new("/usr/lib/ppvpn/ppvpn"),
            false
        ));
        assert!(!linux_client_path_allowed(
            Path::new("/usr/lib/ppvpn/ppvpn (deleted)"),
            false
        ));
        assert!(!linux_client_path_allowed(
            Path::new("/work/user/ppvpn-desktop/apps/linux/bin/Debug/net8.0/ppvpn"),
            false
        ));
        assert!(linux_client_path_allowed(
            Path::new("/work/user/ppvpn-desktop/apps/linux/bin/Debug/net8.0/ppvpn"),
            true
        ));
        assert!(!linux_client_path_allowed(
            Path::new("/work/user/ppvpn-desktop/apps/linux/bin/Debug/net8.0/other"),
            true
        ));
    }

    #[test]
    fn a_client_whose_binary_was_upgraded_is_judged_by_its_original_path() {
        use std::path::Path;

        let replaced = linux_client_image(Path::new("/usr/lib/ppvpn/ppvpn (deleted)"));
        assert_eq!(replaced, Path::new("/usr/lib/ppvpn/ppvpn"));
        assert!(linux_client_path_allowed(&replaced, false));
        assert_eq!(
            linux_client_image(Path::new("/usr/lib/ppvpn/ppvpn")),
            Path::new("/usr/lib/ppvpn/ppvpn")
        );
        // Only the kernel's suffix is removed: another binary stays another binary.
        assert!(!linux_client_path_allowed(
            &linux_client_image(Path::new("/tmp/ppvpn (deleted)")),
            false
        ));
    }
}

#[cfg(not(any(windows, unix)))]
pub async fn run_ipc_server() -> Result<()> {
    anyhow::bail!("ppvpn-service unsupported platform")
}
