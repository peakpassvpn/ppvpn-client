//! Native capture resource boundary. Authentication, descriptor validation,
//! OS route/DNS ownership and bounded broker transactions belong to the host.
//! Tokens on the wire resolve here; no descriptor is serialized.
use crate::capture::{CaptureLeftover, CaptureSpec, ForwarderIdentity};
use async_trait::async_trait;
use std::sync::Arc;

/// Serialize macOS non-atomic descriptor creation/SCM_RIGHTS receive with all
/// child-process creation in this process. Hold only through FD_CLOEXEC or spawn,
/// never while waiting for child exit. Hosts must use this same gate for their
/// own descriptor creation/receive and child spawns. Darwin has no
/// MSG_CMSG_CLOEXEC; a host-local lock cannot protect core DNS child creation.
#[cfg(target_os = "macos")]
pub fn fd_spawn_guard() -> std::sync::MutexGuard<'static, ()> {
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GATE.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Actual live Sail registry in this process, without constructing an Engine.
/// Diagnostics only: this does not prove uniqueness in another OS process.
pub fn forwarding_runtime_ids() -> Vec<u64> {
    let mut ids: Vec<_> = sail::runtime_managers()
        .keys()
        .copied()
        .map(u64::from)
        .collect();
    ids.sort_unstable();
    ids
}

pub use sail::embed::TunPacketFlow as CapturePacketFlow;
pub type CaptureHostResult<T> = Result<T, Vec<CaptureLeftover>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureNetworkSettings {
    pub mtu: u16,
    pub ipv4_address: String,
    pub ipv6_address: Option<String>,
    pub dns_servers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTransport {
    /// An authenticated, independently validated host-owned device. The
    /// callback supplies a duplicate only; the host retains its original.
    Device,
    /// Public packet-flow APIs or isolated packet tests; bounded both ways.
    Packets { capacity: usize },
}

/// A staged, process-local resource and its exact broker ledger. Every async
/// method must have its own bounded settlement. Dropping its await must not
/// permit untracked late OS mutations. Errors report opaque leftover tokens,
/// never routes, command text, credentials or raw descriptors.
#[async_trait]
pub trait CaptureAttachment: Send + Sync + 'static {
    fn transport(&self) -> CaptureTransport;
    /// Exact actual interface verified by the host. Packet transport may use
    /// an empty name. Device transport must not rely on automatic discovery.
    fn interface_name(&self) -> &str;
    #[cfg(unix)]
    fn duplicate_device(&self) -> std::io::Result<std::os::fd::OwnedFd> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
    /// Takes ownership of the packet bridge after native commit. On failure
    /// native capture will be detached before rollback_attach.
    fn publish_packets(&self, _flow: CapturePacketFlow) -> CaptureHostResult<()> {
        Err(Vec::new())
    }
    /// Publish only this resource's OS routes/DNS; forwarder policy is unchanged.
    async fn commit_attach(&self) -> CaptureHostResult<()>;
    async fn rollback_attach(&self) -> CaptureHostResult<()>;
    /// Remove owned OS admission/settings, retaining enough state and the
    /// original handle to restore the same attachment if native detach fails.
    async fn commit_detach(&self) -> CaptureHostResult<()>;
    async fn rollback_detach(&self) -> CaptureHostResult<()>;
}

#[async_trait]
pub trait CaptureHostApi: Send + Sync + 'static {
    /// Validate peer/lease/expiry and reserve the exact resource. Failure must
    /// already have rolled back preparation; nonempty leftovers quarantine it.
    async fn prepare(
        &self,
        owner: &ForwarderIdentity,
        attachment_epoch: u64,
        spec: &CaptureSpec,
        settings: &CaptureNetworkSettings,
    ) -> CaptureHostResult<Arc<dyn CaptureAttachment>>;
}

/// Identity-compared host hook, skipped by EngineConfig serialization.
#[derive(Clone)]
pub struct CaptureHost(pub(crate) Arc<dyn CaptureHostApi>);
impl CaptureHost {
    pub fn new(api: Arc<dyn CaptureHostApi>) -> Self {
        Self(api)
    }
}
impl std::fmt::Debug for CaptureHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CaptureHost(..)")
    }
}
impl PartialEq for CaptureHost {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub(crate) type DeviceSlot = Arc<std::sync::Mutex<Option<Arc<dyn CaptureAttachment>>>>;
pub(crate) struct DevicePlatform {
    #[cfg(unix)]
    pub slot: DeviceSlot,
    pub protect: Option<crate::SocketProtect>,
}
impl sail::embed::Platform for DevicePlatform {
    fn log(&self, line: &str) {
        tracing::info!(target:"sail", "{line}");
    }
    fn protects_sockets(&self) -> bool {
        self.protect.is_some()
    }
    fn protect_socket(&self, fd: i32) -> std::io::Result<()> {
        match &self.protect {
            Some(p) => p.protect(fd),
            None => Ok(()),
        }
    }
    fn opens_tun(&self) -> bool {
        true
    }
    fn open_tun(&self, request: &sail::embed::TunRequest) -> std::io::Result<i32> {
        #[cfg(unix)]
        {
            use std::os::fd::IntoRawFd;
            let resource = self
                .slot
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                .ok_or(std::io::ErrorKind::NotFound)?;
            let name = resource.interface_name().to_owned();
            if request.auto_route || request.name != name {
                return Err(std::io::ErrorKind::InvalidInput.into());
            }
            let fd = resource.duplicate_device()?;
            validate_duplicate(&fd, &name)?;
            Ok(fd.into_raw_fd())
        }
        #[cfg(not(unix))]
        {
            let _ = request;
            Err(std::io::ErrorKind::Unsupported.into())
        }
    }
}

#[cfg(unix)]
fn validate_duplicate(fd: &std::os::fd::OwnedFd, name: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: fcntl only reads/updates this owned duplicate's descriptor flags.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    #[cfg(target_os = "macos")]
    {
        // Public SDK net/if_utun.h and sys/sys_domain.h: a successful actual
        // UTUN_OPT_IFNAME lookup also rejects ordinary TCP sockets/files.
        const SYSPROTO_CONTROL: libc::c_int = 2;
        const UTUN_OPT_IFNAME: libc::c_int = 2;
        let mut actual = [0_u8; 64];
        let mut length = actual.len() as libc::socklen_t;
        // SAFETY: the buffer and its length are valid for getsockopt.
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                SYSPROTO_CONTROL,
                UTUN_OPT_IFNAME,
                actual.as_mut_ptr().cast(),
                &mut length,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let length = (length as usize).min(actual.len());
        let end = actual[..length]
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(length);
        if &actual[..end] != name.as_bytes() {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = name;
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, OwnedFd};
    // Only the explicitly selected child invocation reads the probe variable.
    #[test]
    fn descriptor_exec_probe_child() {
        if let Ok(value) = std::env::var("PPVPN_CAPTURE_FD_EXEC_PROBE") {
            let (fd, expected) = value.split_once(':').unwrap();
            let open = unsafe { libc::fcntl(fd.parse().unwrap(), libc::F_GETFD) } >= 0;
            std::process::exit(if open == (expected == "open") { 0 } else { 9 });
        }
    }
    #[test]
    fn shared_fd_gate_prevents_child_inheritance_during_cloexec_setup() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::sync::mpsc;
        use std::time::Duration;
        let file = tempfile::tempfile().unwrap();
        let gate = fd_spawn_guard();
        // Model Darwin's received descriptor before fcntl: deliberately no
        // CLOEXEC, with a high number that child stdio cannot reuse.
        let raw = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 256) };
        assert!(raw >= 0);
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
        let child = move |expected: &str| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "capture_host::tests::descriptor_exec_probe_child",
                ])
                .env("PPVPN_CAPTURE_FD_EXEC_PROBE", format!("{raw}:{expected}"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        // Positive control: an uncoordinated actual exec inherits this fd.
        assert!(child("open"));
        let (ready, started) = mpsc::channel();
        let (finished, result) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            ready.send(()).unwrap();
            let child = {
                let _gate = fd_spawn_guard();
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "capture_host::tests::descriptor_exec_probe_child",
                    ])
                    .env("PPVPN_CAPTURE_FD_EXEC_PROBE", format!("{raw}:closed"))
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            };
            // The gate is released before waiting for the child.
            finished
                .send(child.wait_with_output().unwrap().status.success())
                .unwrap();
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(result.recv_timeout(Duration::from_millis(30)).is_err());
        assert_eq!(
            unsafe { libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
        drop(gate);
        assert!(result.recv_timeout(Duration::from_secs(30)).unwrap());
        thread.join().unwrap();
        drop(descriptor);
    }
    #[test]
    fn a_non_utun_descriptor_is_refused_and_the_original_remains_owned() {
        let file = tempfile::tempfile().unwrap();
        let duplicate: OwnedFd = file.try_clone().unwrap().into();
        let raw = duplicate.as_raw_fd();
        assert!(validate_duplicate(&duplicate, "utun231").is_err());
        // SAFETY: both owned descriptors remain alive during these reads.
        assert_ne!(
            unsafe { libc::fcntl(raw, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert!(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) } >= 0);
    }
}
