//! Capture-only root service. Every channel is audit-token/code-signature/image
//! checked before handshake; every grant is bound to that exact user process,
//! forwarding identity and core-owned attachment epoch. It never holds Engine.
use super::{
    auth::Session,
    capture::{Capture, Owner},
    fdpass, mac_peer,
    native::{self, Resources, Settings},
};
use crate::{
    peer_identity::Peer,
    protocol::{self, Command, HandshakeResponse, Request, Response},
};
use anyhow::{anyhow, bail, Result};
use ppvpn_core_wire::CaptureLease;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
pub const IMAGE:&str="/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle/Contents/MacOS/ppvpn-service";
pub const DIRECTORY: &str = "/Library/Application Support/PPVPN/run/capture";
pub const SOCKET: &str = "/Library/Application Support/PPVPN/run/capture/broker.sock";
pub const JOURNAL: &str = "/Library/Application Support/PPVPN/run/capture/grant.json";
const FRAME_LIMIT: usize = 16 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub token: String,
    pub lease: CaptureLease,
    pub settings: Option<Settings>,
}
impl Grant {
    fn owner(&self, peer: Peer) -> Owner {
        Owner {
            uid: peer.uid,
            pid: peer.pid,
            pid_version: peer.pid_version,
            audit_session: peer.audit_session,
            session_id: self.lease.owner.instance.clone(),
            generation: self.lease.owner.generation,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if uuid::Uuid::parse_str(&self.token).is_err()
            || self.lease.owner.instance.is_empty()
            || self.lease.owner.instance.len() > 128
            || self.lease.owner.generation == 0
            || self.lease.attachment_epoch == 0
        {
            bail!("CAPTURE_GRANT_INVALID");
        }
        Ok(())
    }
}
pub fn write_frame<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.is_empty() || bytes.len() > FRAME_LIMIT {
        bail!("CAPTURE_FRAME_INVALID");
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    Ok(())
}
pub fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut size = [0; 4];
    stream.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    if size == 0 || size > FRAME_LIMIT {
        bail!("CAPTURE_FRAME_INVALID");
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
struct Endpoint {
    listener: UnixListener,
    _lock: File,
    inode: u64,
}
impl Endpoint {
    fn bind() -> Result<Self> {
        if unsafe { libc::geteuid() } != 0 {
            bail!("CAPTURE_BROKER_ROOT_REQUIRED");
        }
        let dir = Path::new(DIRECTORY);
        // /private/var/run is legitimately root:daemon group-writable on // sensitive-check: allow — canonical macOS system directory
        // macOS. Use the existing protected system IPC hierarchy instead of
        // relaxing the ancestry policy or modifying a system directory.
        for directory in [
            "/Library/Application Support",
            "/Library/Application Support/PPVPN",
            "/Library/Application Support/PPVPN/run",
            DIRECTORY,
        ] {
            super::login::root_directory(Path::new(directory))?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir.join("broker.lock"))?;
        let m = lock.metadata()?;
        if !m.is_file() || m.uid() != 0 || m.nlink() != 1 || m.mode() & 0o077 != 0 {
            bail!("CAPTURE_LOCK_UNSAFE");
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("CAPTURE_BROKER_ALREADY_RUNNING");
        }
        match std::fs::symlink_metadata(SOCKET) {
            Ok(m) => {
                if !m.file_type().is_socket() || m.uid() != 0 {
                    bail!("CAPTURE_ENDPOINT_UNSAFE");
                }
                match UnixStream::connect(SOCKET) {
                    Ok(_) => bail!("CAPTURE_BROKER_ALREADY_RUNNING"),
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                        std::fs::remove_file(SOCKET)?
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let listener = UnixListener::bind(SOCKET)?;
        std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o666))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            inode: std::fs::symlink_metadata(SOCKET)?.ino(),
            _lock: lock,
        })
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(SOCKET)
            .is_ok_and(|m| m.ino() == self.inode && m.file_type().is_socket())
        {
            let _ = std::fs::remove_file(SOCKET);
        }
    }
}
struct Ledger {
    capture: Capture<(String, Resources)>,
    peer: Option<(Peer, [u32; 8])>,
}
impl super::capture::Resources for (String, Resources) {
    fn cleanup(&mut self) -> std::result::Result<(), String> {
        self.1
            .cleanup()
            .map_err(|_| "CAPTURE_LEDGER_UNSETTLED".into())
    }
}
impl Ledger {
    fn operation(&mut self, peer: Peer, path: &str, g: Grant) -> Result<Option<OwnedFd>> {
        g.validate()?;
        let owner = g.owner(peer);
        let now = Instant::now();
        // A signed readiness probe does not renew, heal, create or remove any
        // resource. Startup recovery has settled before this server accepts.
        if path == "probe" {
            return Ok(None);
        }
        if !self.capture.occupied() && matches!(path, "detach" | "release") {
            return Ok(None);
        }
        if path == "prepare" {
            let settings = g
                .settings
                .clone()
                .ok_or_else(|| anyhow!("CAPTURE_SETTINGS_REQUIRED"))?;
            settings.validate()?;
            let token = g.token.clone();
            self.capture
                .acquire(owner, g.lease.clone(), now, |_| {
                    Resources::prepare(&token, settings)
                        .map(|r| (token.clone(), r))
                        .map_err(|e| {
                            native::diagnostic("resource-prepare", &e);
                            "CAPTURE_PREPARE_FAILED".into()
                        })
                })
                .map_err(|e| anyhow!("{e:?}"))?;
            return self
                .capture
                .resources(&g.owner(peer), &g.lease, now, |(_, r)| {
                    r.fd.try_clone()
                        .map(Some)
                        .map_err(|_| "CAPTURE_FD_DUPLICATE_FAILED".into())
                })
                .map_err(|e| anyhow!("{e:?}"));
        }
        self.capture
            .resources(&owner, &g.lease, now, |(token, _)| {
                if token != &g.token {
                    return Err("CAPTURE_TOKEN_MISMATCH".into());
                }
                Ok(())
            })
            .map_err(|e| anyhow!("{e:?}"))?;
        if path == "renew" {
            self.capture
                .resources(&owner, &g.lease, now, |(_, r)| {
                    r.heal().map_err(|_| "CAPTURE_HEAL_FAILED".into())
                })
                .map_err(|e| anyhow!("{e:?}"))?;
            self.capture
                .renew(&owner, &g.lease, now)
                .map_err(|e| anyhow!("{e:?}"))?;
            return Ok(None);
        }
        if path == "release" {
            self.capture
                .detach(&owner, &g.lease)
                .map_err(|e| anyhow!("{e:?}"))?;
            self.peer = None;
            return Ok(None);
        }
        // Token names only a prepared resource; it cannot substitute any other
        // process, lease/epoch, routes, DNS key or raw descriptor.
        self.capture
            .resources(&owner, &g.lease, now, |(token, r)| {
                if token != &g.token {
                    return Err("CAPTURE_TOKEN_MISMATCH".into());
                }
                match path {
                    "attach" | "restore" => r.attach(),
                    "detach" => r.detach(),
                    "release" => r.cleanup(),
                    _ => return Err("CAPTURE_OPERATION_INVALID".into()),
                }
                .map_err(|_| "CAPTURE_LEDGER_UNSETTLED".into())
            })
            .map_err(|e| anyhow!("{e:?}"))?;
        Ok(None)
    }
}
fn configure_accepted_stream(stream: &UnixStream) -> io::Result<()> {
    // Darwin inherits O_NONBLOCK from the listener. SO_RCVTIMEO alone does not
    // change that flag: a second frame would immediately fail with EAGAIN.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(600)))?;
    stream.set_write_timeout(Some(Duration::from_millis(600)))
}
fn transact(stream: &mut UnixStream, ledger: &mut Ledger, generation: u64) -> Result<()> {
    transact_using(stream, ledger, generation, |stream, untrusted| {
        mac_peer::verify(
            stream,
            untrusted.uid,
            crate::mac_identity::SERVICE_IDENTIFIER,
            Path::new(IMAGE),
        )
    })
}
fn transact_using(
    stream: &mut UnixStream,
    ledger: &mut Ledger,
    generation: u64,
    verify: impl FnOnce(&UnixStream, Peer) -> Result<Peer>,
) -> Result<()> {
    let (untrusted, token) = mac_peer::audit_peer(stream)?;
    let peer = native::timed("broker-peer-check", || verify(stream, untrusted))?;
    let hello: Request = native::timed("broker-hello-read", || read_frame(stream))?;
    if hello.command != Command::Handshake
        || !hello.is_fresh()
        || hello.id.is_empty()
        || hello.id.len() > 128
    {
        bail!("CAPTURE_HANDSHAKE_INVALID");
    }
    if hello.payload.get("build_id").and_then(|v| v.as_str()) != Some(protocol::SERVICE_BUILD_ID)
        || hello
            .payload
            .get("protocol_version")
            .and_then(|v| v.as_u64())
            != Some(1)
    {
        bail!("CAPTURE_BROKER_VERSION_MISMATCH");
    }
    let nonce = hello
        .payload
        .get("client_nonce")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !(32..=128).contains(&nonce.len()) {
        bail!("CAPTURE_HANDSHAKE_INVALID");
    }
    let mut key = [0; 32];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| anyhow!("RANDOM_UNAVAILABLE"))?;
    let auth = uuid::Uuid::new_v4().to_string();
    let mut session = Session::new(
        peer,
        key,
        auth.clone(),
        generation,
        Instant::now(),
        Duration::from_secs(5),
    )
    .map_err(|_| anyhow!("CAPTURE_AUTH_INVALID"))?;
    let mut handshake = serde_json::to_value(HandshakeResponse {
        auth_token: auth,
        auth_key: hex::encode(key),
        expires_at: protocol::now_epoch() + 5,
    })?;
    handshake["build_id"] = json!(protocol::SERVICE_BUILD_ID);
    handshake["protocol_version"] = json!(1);
    native::timed("broker-hello-write", || {
        write_frame(
            stream,
            &Response::unsigned(hello.id, true, Some(handshake), None),
        )
    })?;
    let request: Request = native::timed("broker-request-read", || read_frame(stream))?;
    if mac_peer::audit_peer(stream)?.0 != peer {
        bail!("CAPTURE_PEER_CHANGED");
    }
    session
        .verify(
            peer,
            &request,
            generation,
            Instant::now(),
            protocol::now_epoch(),
        )
        .map_err(|_| anyhow!("CAPTURE_AUTH_REJECTED"))?;
    let result = (|| {
        if !ppvpn_core::capture_host::forwarding_runtime_ids().is_empty() {
            bail!("CAPTURE_BROKER_ROLE_VIOLATION");
        }
        if request.command != Command::CoreApi {
            bail!("CAPTURE_OPERATION_INVALID");
        }
        let path = request
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("CAPTURE_OPERATION_INVALID"))?;
        let g: Grant =
            serde_json::from_value(request.payload.get("body").cloned().unwrap_or_default())?;
        let stage = match path {
            "prepare" => "broker-prepare",
            "attach" => "broker-attach",
            "detach" => "broker-detach",
            "release" => "broker-release",
            "renew" => "broker-renew",
            "restore" => "broker-restore",
            "probe" => "broker-probe",
            _ => "broker-other",
        };
        let fd = native::timed(stage, || ledger.operation(peer, path, g))?;
        if path == "prepare" {
            ledger.peer = Some((peer, token));
        }
        let name = fd.as_ref().map(native::interface).transpose()?;
        Ok::<_, anyhow::Error>((fd, name))
    })();
    match result {
        Ok((fd, name)) => {
            let response =
                session.sign_response(request.id, true, Some(json!({"interface":name,"root_runtime_ids":ppvpn_core::capture_host::forwarding_runtime_ids()})), None)?;
            write_frame(stream, &response)?;
            native::trace_phase("broker-response-written");
            if let Some(fd) = fd {
                fdpass::send(stream, &fd)?;
                native::trace_phase("broker-device-sent");
            }
        }
        Err(error) => {
            native::diagnostic("broker-operation", &error);
            let clean = request.payload.get("path").and_then(|v| v.as_str()) == Some("prepare")
                && !ledger.capture.occupied();
            write_frame(
                stream,
                &session.sign_response(
                    request.id,
                    false,
                    Some(json!({"clean":clean})),
                    Some("CAPTURE_BROKER_OPERATION_FAILED".into()),
                )?,
            )?;
        }
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn transact_test(
    stream: &mut UnixStream,
    verify: impl FnOnce(&UnixStream, Peer) -> Result<Peer>,
) -> (Result<()>, bool) {
    configure_accepted_stream(stream).unwrap();
    let mut ledger = Ledger {
        capture: Capture::default(),
        peer: None,
    };
    let result = transact_using(stream, &mut ledger, 1, verify);
    (result, ledger.capture.occupied())
}
struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}
static RUNNING: OnceLock<parking_lot::Mutex<Option<Running>>> = OnceLock::new();
pub fn start() -> Result<()> {
    let endpoint = super::image_verification::before_exposure(Endpoint::bind)?;
    native::recover()?;
    let mut seed = [0; 8];
    SystemRandom::new()
        .fill(&mut seed)
        .map_err(|_| anyhow!("RANDOM_UNAVAILABLE"))?;
    let generation = u64::from_ne_bytes(seed).max(1);
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let thread = std::thread::Builder::new()
        .name("ppvpn-capture-broker".into())
        .spawn(move || {
            let mut ledger = Ledger {
                capture: Capture::default(),
                peer: None,
            };
            let requirement = crate::mac_identity::requirement(
                crate::mac_identity::SERVICE_IDENTIFIER,
                crate::mac_identity::team_id(),
            )?;
            let mut checked = Instant::now();
            while !stopping.load(Ordering::SeqCst) {
                if Instant::now() >= checked {
                    checked = Instant::now() + Duration::from_secs(1);
                    if let Some((_, token)) = &ledger.peer {
                        let bytes: Vec<_> = token.iter().flat_map(|w| w.to_ne_bytes()).collect();
                        if native::timed("broker-peer-watch", || {
                            crate::mac_identity::check_process(&bytes, &requirement)
                        })
                        .is_err()
                        {
                            let _ = ledger.capture.broker_shutdown();
                            ledger.peer = None;
                        }
                    }
                    let started = Instant::now();
                    let expired = ledger.capture.expire(Instant::now());
                    native::trace_operation(
                        "broker-expire-check",
                        started.elapsed(),
                        expired.is_ok(),
                    );
                    if let Err(error) = expired {
                        log::error!("capture broker cleanup quarantined: {error:?}");
                        let _ = ledger.capture.retry_cleanup();
                    }
                }
                match endpoint.listener.accept() {
                    Ok((mut stream, _)) => {
                        if let Err(error) = configure_accepted_stream(&stream) {
                            native::diagnostic("broker-accepted-stream", &error.into());
                            continue;
                        }
                        if let Err(error) = native::timed("broker-transaction", || {
                            transact(&mut stream, &mut ledger, generation)
                        }) {
                            native::diagnostic("broker-transport", &error);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    Err(e) => {
                        let _ = ledger.capture.broker_shutdown();
                        return Err(e.into());
                    }
                }
            }
            ledger
                .capture
                .broker_shutdown()
                .map_err(|_| anyhow!("CAPTURE_BROKER_SHUTDOWN_LEFTOVERS"))
        })?;
    let slot = RUNNING.get_or_init(|| parking_lot::Mutex::new(None));
    let mut slot = slot.lock();
    if slot.is_some() {
        stop.store(true, Ordering::SeqCst);
        let _ = thread.join();
        bail!("CAPTURE_BROKER_ALREADY_RUNNING");
    }
    *slot = Some(Running {
        stop,
        thread: Some(thread),
    });
    Ok(())
}
pub fn shutdown() -> Result<()> {
    let Some(slot) = RUNNING.get() else {
        return Ok(());
    };
    let Some(mut running) = slot.lock().take() else {
        return Ok(());
    };
    running.stop.store(true, Ordering::SeqCst);
    running
        .thread
        .take()
        .unwrap()
        .join()
        .map_err(|_| anyhow!("CAPTURE_BROKER_PANICKED"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn darwin_accepted_stream_blocks_for_a_delayed_signed_second_frame() {
        for raw_accept in [false, true] {
            let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "pv-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            ));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("s");
            let listener = UnixListener::bind(&path).unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut client = UnixStream::connect(&path).unwrap();
            let mut accepted = if raw_accept {
                let _gate = ppvpn_core::capture_host::fd_spawn_guard();
                use std::os::fd::FromRawFd;
                let fd = unsafe {
                    libc::accept(
                        listener.as_raw_fd(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                assert!(fd >= 0);
                assert_eq!(
                    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                    0
                );
                unsafe { UnixStream::from_raw_fd(fd) }
            } else {
                listener.accept().unwrap().0
            };
            assert_ne!(
                unsafe { libc::fcntl(accepted.as_raw_fd(), libc::F_GETFL) } & libc::O_NONBLOCK,
                0
            );
            let mut byte = [0];
            let before = Instant::now();
            assert_eq!(
                accepted.read(&mut byte).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            assert!(before.elapsed() < Duration::from_millis(100));
            configure_accepted_stream(&accepted).unwrap();
            assert_eq!(
                accepted.read_timeout().unwrap(),
                Some(Duration::from_millis(600))
            );
            assert_eq!(
                accepted.write_timeout().unwrap(),
                Some(Duration::from_millis(600))
            );
            assert_eq!(
                unsafe { libc::fcntl(accepted.as_raw_fd(), libc::F_GETFL) } & libc::O_NONBLOCK,
                0
            );
            let server = std::thread::spawn(move || {
                let (peer, _) = mac_peer::audit_peer(&accepted).unwrap();
                let hello: Request = read_frame(&mut accepted).unwrap();
                assert_eq!(hello.command, Command::Handshake);
                let key = [0x42; 32];
                let token = uuid::Uuid::new_v4().to_string();
                let mut session = Session::new(
                    peer,
                    key,
                    token.clone(),
                    1,
                    Instant::now(),
                    Duration::from_secs(5),
                )
                .unwrap();
                let started = Instant::now();
                write_frame(
                    &mut accepted,
                    &Response::unsigned(
                        hello.id,
                        true,
                        Some(
                            serde_json::to_value(HandshakeResponse {
                                auth_token: token,
                                auth_key: hex::encode(key),
                                expires_at: protocol::now_epoch() + 5,
                            })
                            .unwrap(),
                        ),
                        None,
                    ),
                )
                .unwrap();
                let request: Request = read_frame(&mut accepted).unwrap();
                assert!(started.elapsed() >= Duration::from_millis(80));
                session
                    .verify(peer, &request, 1, Instant::now(), protocol::now_epoch())
                    .unwrap();
                let response = session
                    .sign_response(request.id, true, Some(json!({"probe":true})), None)
                    .unwrap();
                write_frame(&mut accepted, &response).unwrap();
            });
            let hello = Request {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp: protocol::now_epoch(),
                command: Command::Handshake,
                payload: json!({}),
                auth_token: None,
                signature: String::new(),
            };
            write_frame(&mut client, &hello).unwrap();
            let response: Response = read_frame(&mut client).unwrap();
            let hello: HandshakeResponse = serde_json::from_value(response.data.unwrap()).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let key = hex::decode(hello.auth_key).unwrap();
            let request = Request::signed(
                uuid::Uuid::new_v4().to_string(),
                Command::CoreApi,
                json!({"path":"probe"}),
                Some(hello.auth_token),
                &key,
            )
            .unwrap();
            write_frame(&mut client, &request).unwrap();
            let response: Response = read_frame(&mut client).unwrap();
            assert!(response.success && response.verify_with(&key).unwrap());
            server.join().unwrap();
            drop((client, listener));
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    fn capture_paths_share_the_protected_system_ipc_hierarchy() {
        let base = Path::new(crate::protocol::IPC_SOCKET_PATH)
            .parent()
            .unwrap();
        assert!(Path::new(DIRECTORY).starts_with(base));
        assert_eq!(Path::new(SOCKET).parent(), Some(Path::new(DIRECTORY)));
        assert_eq!(Path::new(JOURNAL).parent(), Some(Path::new(DIRECTORY)));
        // Read only: never create a directory or modify the workstation.
        for path in Path::new(DIRECTORY).ancestors() {
            if let Ok(m) = std::fs::symlink_metadata(path) {
                assert!(m.is_dir() && !m.file_type().is_symlink());
                assert_eq!(m.uid(), 0);
                assert_eq!(m.mode() & 0o022, 0);
            }
        }
    }
    #[test]
    fn readiness_probe_cannot_allocate_an_os_grant_or_bypass_grant_validation() {
        let peer = Peer {
            uid: 501,
            pid: 1,
            pid_version: 1,
            audit_session: 1,
        };
        let mut ledger = Ledger {
            capture: Capture::default(),
            peer: None,
        };
        let grant = Grant {
            token: uuid::Uuid::new_v4().to_string(),
            lease: CaptureLease {
                owner: ppvpn_core_wire::ForwarderIdentity {
                    instance: "readiness-owner".into(),
                    generation: 1,
                },
                attachment_epoch: 1,
            },
            settings: None,
        };
        assert!(ledger
            .operation(peer, "probe", grant.clone())
            .unwrap()
            .is_none());
        assert!(!ledger.capture.occupied());
        assert!(ledger.peer.is_none());
        assert!(ledger.operation(peer, "prepare", grant.clone()).is_err());
        let mut malformed = grant;
        malformed.token.clear();
        assert!(ledger.operation(peer, "probe", malformed).is_err());
        assert!(!ledger.capture.occupied());
    }
}
