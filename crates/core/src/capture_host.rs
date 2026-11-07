//! Native capture resource boundary. Authentication, descriptor validation,
//! OS route/DNS ownership and bounded broker transactions belong to the host.
//! Tokens on the wire resolve here; no descriptor is serialized.
use crate::capture::{CaptureLeftover, CaptureSpec, ForwarderIdentity};
use async_trait::async_trait;
use std::sync::Arc;

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
