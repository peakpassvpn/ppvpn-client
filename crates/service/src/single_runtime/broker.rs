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
    io::{Read, Write},
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
pub const DIRECTORY: &str = "/private/var/run/com.peakpassvpn.capture"; // sensitive-check: allow — canonical macOS system runtime location
pub const SOCKET: &str = "/private/var/run/com.peakpassvpn.capture/broker.sock"; // sensitive-check: allow — canonical macOS system runtime location
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
        if !dir.exists() {
            std::fs::create_dir(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
        }
        for ancestor in dir.ancestors() {
            let m = std::fs::symlink_metadata(ancestor)?;
            if !m.is_dir() || m.uid() != 0 || m.mode() & 0o022 != 0 {
                bail!("CAPTURE_DIRECTORY_UNSAFE");
            }
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
                        .map_err(|_| "CAPTURE_PREPARE_FAILED".into())
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
fn transact(stream: &mut UnixStream, ledger: &mut Ledger, generation: u64) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(600)))?;
    stream.set_write_timeout(Some(Duration::from_millis(600)))?;
    let (untrusted, token) = mac_peer::audit_peer(stream)?;
    let peer = mac_peer::verify(
        stream,
        untrusted.uid,
        crate::mac_identity::SERVICE_IDENTIFIER,
        Path::new(IMAGE),
    )?;
    let hello: Request = read_frame(stream)?;
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
    write_frame(
        stream,
        &Response::unsigned(hello.id, true, Some(handshake), None),
    )?;
    let request: Request = read_frame(stream)?;
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
        let fd = ledger.operation(peer, path, g)?;
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
            if let Some(fd) = fd {
                fdpass::send(stream, &fd)?;
            }
        }
        Err(_) => {
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
struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}
static RUNNING: OnceLock<parking_lot::Mutex<Option<Running>>> = OnceLock::new();
pub fn start() -> Result<()> {
    let endpoint = Endpoint::bind()?;
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
                        if crate::mac_identity::check_process(&bytes, &requirement).is_err() {
                            let _ = ledger.capture.broker_shutdown();
                            ledger.peer = None;
                        }
                    }
                    if let Err(error) = ledger.capture.expire(Instant::now()) {
                        log::error!("capture broker cleanup quarantined: {error:?}");
                        let _ = ledger.capture.retry_cleanup();
                    }
                }
                match endpoint.listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = transact(&mut stream, &mut ledger, generation);
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
