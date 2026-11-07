//! macOS capture-only OS operations. No Engine, profile, proxy or dispatcher.
//! Commands have fixed executable paths, argument arrays and bounded settlement.
use anyhow::{bail, Result};
use core_foundation::{
    array::CFArray,
    base::{CFType, CFTypeRef, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use ppvpn_core::{capture_host::CaptureNetworkSettings, NetworkKind, NetworkState};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{c_void, CStr, CString},
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCDynamicStoreCreate(
        a: CFTypeRef,
        name: CFTypeRef,
        callout: *const c_void,
        context: *mut c_void,
    ) -> CFTypeRef;
    fn SCDynamicStoreCopyValue(store: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn SCDynamicStoreAddTemporaryValue(store: CFTypeRef, key: CFTypeRef, value: CFTypeRef) -> u8;
    fn SCDynamicStoreRemoveValue(store: CFTypeRef, key: CFTypeRef) -> u8;
}
pub struct Store(CFType);
// Only used under its owner's mutex, with no callbacks; CFType owns the session.
unsafe impl Send for Store {}
impl Store {
    pub fn new() -> Result<Self> {
        let name = CFString::new("com.peakpassvpn.capture");
        let raw = unsafe {
            SCDynamicStoreCreate(
                std::ptr::null(),
                name.as_CFTypeRef(),
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if raw.is_null() {
            bail!("DYNAMIC_STORE_UNAVAILABLE");
        }
        Ok(Self(unsafe { CFType::wrap_under_create_rule(raw) }))
    }
    fn copy(&self, key: &str) -> Option<CFType> {
        let key = CFString::new(key);
        let raw = unsafe { SCDynamicStoreCopyValue(self.0.as_CFTypeRef(), key.as_CFTypeRef()) };
        (!raw.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(raw) })
    }
    pub fn dictionary(&self, key: &str) -> Option<CFDictionary<CFString, CFType>> {
        // Check the untyped dictionary first; typed values are CFType, keys are
        // the OS schema's CFStrings. Missing/wrong schema is treated as offline.
        self.copy(key)?
            .downcast::<CFDictionary>()
            .map(|d| unsafe { CFDictionary::wrap_under_get_rule(d.as_concrete_TypeRef()) })
    }
    pub fn remove(&self, key: &str) -> Result<()> {
        let k = CFString::new(key);
        if unsafe { SCDynamicStoreRemoveValue(self.0.as_CFTypeRef(), k.as_CFTypeRef()) } == 0
            && self.copy(key).is_some()
        {
            bail!("DNS_REMOVE_FAILED");
        }
        Ok(())
    }
    pub fn add_dns(&self, key: &str, servers: &[String]) -> Result<()> {
        let servers: Vec<_> = servers.iter().map(|s| CFString::new(s)).collect();
        let values = CFDictionary::from_CFType_pairs(&[
            (
                CFString::new("ServerAddresses"),
                CFArray::from_CFTypes(&servers).as_CFType(),
            ),
            (
                CFString::new("SupplementalMatchDomains"),
                CFArray::from_CFTypes(&[CFString::new("")]).as_CFType(),
            ),
            (
                CFString::new("SearchOrder"),
                CFNumber::from(100_000i32).as_CFType(),
            ),
        ]);
        let key = CFString::new(key);
        if unsafe {
            SCDynamicStoreAddTemporaryValue(
                self.0.as_CFTypeRef(),
                key.as_CFTypeRef(),
                values.as_CFTypeRef(),
            )
        } == 0
        {
            bail!("DNS_ADD_FAILED");
        }
        Ok(())
    }
}
fn field(d: &CFDictionary<CFString, CFType>, name: &str) -> Option<String> {
    d.find(CFString::new(name))?
        .downcast::<CFString>()
        .map(|s| s.to_string())
}
/// Physical global primary service only. Capture publishes a separate DNS key
/// and split routes; it never changes Global/IPv4 or a real service's DNS.
/// Refuse other tunnels instead of treating a competitor as an uplink.
pub fn physical() -> Result<NetworkState> {
    let store = Store::new()?;
    let Some(global) = store
        .dictionary("State:/Network/Global/IPv4")
        .or_else(|| store.dictionary("State:/Network/Global/IPv6"))
    else {
        return Ok(NetworkState::offline());
    };
    let Some(name) = field(&global, "PrimaryInterface") else {
        return Ok(NetworkState::offline());
    };
    if name.starts_with("utun") || name.starts_with("tun") || name.starts_with("ppp") {
        bail!("PHYSICAL_NETWORK_CONTENDED");
    }
    let mut head = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    struct Addresses(*mut libc::ifaddrs);
    impl Drop for Addresses {
        fn drop(&mut self) {
            unsafe { libc::freeifaddrs(self.0) }
        }
    }
    let _addresses = Addresses(head);
    let mut node = head;
    let mut addresses = vec![];
    while !node.is_null() {
        let a = unsafe { &*node };
        if !a.ifa_addr.is_null()
            && !a.ifa_netmask.is_null()
            && unsafe { CStr::from_ptr(a.ifa_name) }.to_bytes() == name.as_bytes()
            && a.ifa_flags & libc::IFF_UP as u32 != 0
        {
            let family = unsafe { (*a.ifa_addr).sa_family as i32 };
            if family == libc::AF_INET {
                let ip = unsafe { &*(a.ifa_addr.cast::<libc::sockaddr_in>()) };
                let mask = unsafe { &*(a.ifa_netmask.cast::<libc::sockaddr_in>()) };
                addresses.push(format!(
                    "{}/{}",
                    std::net::Ipv4Addr::from(ip.sin_addr.s_addr.to_ne_bytes()),
                    mask.sin_addr.s_addr.count_ones()
                ));
            } else if family == libc::AF_INET6 {
                let ip = unsafe { &*(a.ifa_addr.cast::<libc::sockaddr_in6>()) };
                let mask = unsafe { &*(a.ifa_netmask.cast::<libc::sockaddr_in6>()) };
                addresses.push(format!(
                    "{}/{}",
                    std::net::Ipv6Addr::from(ip.sin6_addr.s6_addr),
                    mask.sin6_addr
                        .s6_addr
                        .iter()
                        .map(|b| b.count_ones())
                        .sum::<u32>()
                ));
            }
        }
        node = a.ifa_next;
    }
    addresses.sort();
    addresses.dedup();
    let mut state = NetworkState::new(name, NetworkKind::Other).with_addresses(addresses);
    state.gateway = field(&global, "Router");
    Ok(state)
}
/// No shell, inherited input, PATH lookup, payload logging or unbounded wait.
/// Child is reaped before returning even after timeout; OS operation settlement
/// remains serialized by the broker worker, never abandoned on async timeout.
// The same process-wide gate is used by core DNS child creation and by every
// host raw-socket/SCM_RIGHTS receive window; a root-local mutex is insufficient.
pub fn spawn_guard() -> std::sync::MutexGuard<'static, ()> {
    ppvpn_core::capture_host::fd_spawn_guard()
}
/// Control-plane diagnostics contain only a fixed phase, static error code,
/// ErrorKind/errno and elapsed time. Never payloads, paths, tokens or settings.
pub fn diagnostic(stage: &'static str, error: &anyhow::Error) {
    let os = error.chain().find_map(|e| e.downcast_ref::<io::Error>());
    let text = error.to_string();
    let code = text
        .split(':')
        .next()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 80
                && s.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        })
        .unwrap_or("CAPTURE_CONTROL_FAILED");
    eprintln!(
        "capture-control stage={stage} code={code} kind={:?} errno={:?}",
        os.map(io::Error::kind),
        os.and_then(io::Error::raw_os_error)
    );
}
pub fn trace_phase(stage: &'static str) {
    #[cfg(feature = "macos-capture-acceptance")]
    eprintln!("capture-control stage={stage}");
    #[cfg(not(feature = "macos-capture-acceptance"))]
    let _ = stage;
}
pub fn trace_operation(stage: &'static str, elapsed: Duration, ok: bool) {
    #[cfg(feature = "macos-capture-acceptance")]
    eprintln!(
        "capture-control stage={stage} elapsed_ms={} ok={ok} at_ms={}",
        elapsed.as_millis(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    #[cfg(not(feature = "macos-capture-acceptance"))]
    let _ = (stage, elapsed, ok);
}
pub fn timed<T>(stage: &'static str, work: impl FnOnce() -> Result<T>) -> Result<T> {
    let started = Instant::now();
    let result = work();
    trace_operation(stage, started.elapsed(), result.is_ok());
    result
}
fn checked<T>(stage: &'static str, work: impl FnOnce() -> Result<T>) -> Result<T> {
    let result = timed(stage, work);
    if let Err(error) = &result {
        diagnostic(stage, error);
    }
    result
}
fn capture_route_stage(family: &str, prefix: &str, deleting: bool) -> &'static str {
    match (family, prefix, deleting) {
        ("-inet", "0.0.0.0/1", false) => "route-check-v4-low",
        ("-inet", "128.0.0.0/1", false) => "route-check-v4-high",
        ("-inet6", "::/1", false) => "route-check-v6-low",
        ("-inet6", "8000::/1", false) => "route-check-v6-high",
        ("-inet", "0.0.0.0/1", true) => "route-delete-v4-low",
        ("-inet", "128.0.0.0/1", true) => "route-delete-v4-high",
        ("-inet6", "::/1", true) => "route-delete-v6-low",
        ("-inet6", "8000::/1", true) => "route-delete-v6-high",
        _ => "route-control-other",
    }
}
pub fn command(program: &str, args: &[&str]) -> Result<()> {
    let mut child = {
        let _guard = spawn_guard();
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?
    };
    let end = Instant::now() + Duration::from_millis(300);
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            #[cfg(feature = "macos-capture-acceptance")]
            eprintln!(
                "capture-control stage=os-command code=OS_COMMAND_FAILED exit={:?}",
                status.code()
            );
            bail!("OS_COMMAND_FAILED");
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            bail!("OS_COMMAND_TIMEOUT");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub mtu: u16,
    pub ipv4_address: String,
    pub ipv6_address: Option<String>,
    pub dns_servers: Vec<String>,
}
impl From<&CaptureNetworkSettings> for Settings {
    fn from(s: &CaptureNetworkSettings) -> Self {
        Self {
            mtu: s.mtu,
            ipv4_address: s.ipv4_address.clone(),
            ipv6_address: s.ipv6_address.clone(),
            dns_servers: s.dns_servers.clone(),
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        // Broker accepts exactly the shipped core contract, never arbitrary
        // addresses, executable arguments, route prefixes or DNS keys.
        if self.mtu != 9000
            || self.ipv4_address != "10.60.159.89/30"
            || self
                .ipv6_address
                .as_deref()
                .is_some_and(|a| a != "fde2:ec40:9312:c7fd::1/126")
            || self.dns_servers != ["10.60.159.90"]
        {
            bail!("CAPTURE_SETTINGS_INVALID");
        }
        Ok(())
    }
}
#[repr(C)]
struct ControlInfo {
    id: u32,
    name: [u8; 96],
}
#[repr(C)]
struct ControlAddress {
    len: u8,
    family: u8,
    sysaddr: u16,
    id: u32,
    unit: u32,
    reserved: [u32; 5],
}
pub fn interface(fd: &OwnedFd) -> Result<String> {
    let mut bytes = [0u8; 64];
    let mut length = bytes.len() as libc::socklen_t;
    if unsafe { libc::getsockopt(fd.as_raw_fd(), 2, 2, bytes.as_mut_ptr().cast(), &mut length) } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let size = (length as usize).min(bytes.len());
    let end = bytes[..size].iter().position(|b| *b == 0).unwrap_or(size);
    let name = std::str::from_utf8(&bytes[..end])?;
    if !name.starts_with("utun")
        || name.len() > 15
        || !name[4..].bytes().all(|b| b.is_ascii_digit())
    {
        bail!("CAPTURE_DEVICE_INVALID");
    }
    Ok(name.into())
}
pub fn device() -> Result<OwnedFd> {
    let _guard = spawn_guard();
    let raw = unsafe { libc::socket(libc::PF_SYSTEM, libc::SOCK_DGRAM, 2) };
    if raw < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut info = ControlInfo {
        id: 0,
        name: [0; 96],
    };
    let name = b"com.apple.net.utun_control";
    info.name[..name.len()].copy_from_slice(name);
    if unsafe { libc::ioctl(fd.as_raw_fd(), 0xc0644e03, &mut info) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let addr = ControlAddress {
        len: 32,
        family: libc::AF_SYSTEM as u8,
        sysaddr: 2,
        id: info.id,
        unit: 0,
        reserved: [0; 5],
    };
    if unsafe { libc::connect(fd.as_raw_fd(), (&addr as *const ControlAddress).cast(), 32) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    interface(&fd)?;
    Ok(fd)
}
/// Exact ordered resources, retained through detach rollback. Split default
/// routes preserve the physical default and bound outbound sockets bypass them.
/// A temporary DNS key is removed by configd on broker death as well as detach.
pub struct Resources {
    pub fd: OwnedFd,
    pub name: String,
    settings: Settings,
    dns: Store,
    key: String,
    routes: Vec<(&'static str, &'static str)>,
    published: Vec<bool>,
    dns_live: bool,
    admitted: bool,
    journal: Journal,
}
impl Resources {
    pub fn prepare(token: &str, settings: Settings) -> Result<Self> {
        settings.validate()?;
        trace_phase("resource-store-create");
        let dns = Store::new()?;
        trace_phase("utun-create");
        let fd = device().inspect_err(|e| diagnostic("utun-create", e))?;
        let name = interface(&fd)?;
        let mut routes = vec![("-inet", "0.0.0.0/1"), ("-inet", "128.0.0.0/1")];
        if settings.ipv6_address.is_some() {
            routes.extend([("-inet6", "::/1"), ("-inet6", "8000::/1")]);
        }
        let mut marker = *uuid::Uuid::new_v4().as_bytes();
        marker[0] = 0xfd;
        let marker = std::net::Ipv6Addr::from(marker);
        let key = format!("State:/Network/Service/com.peakpassvpn.capture.{token}/DNS");
        trace_phase("resource-journal-create");
        let journal = Journal {
            name: name.clone(),
            index: index(&name)?,
            marker,
            boot: boot()?,
            key: key.clone(),
            settings: settings.clone(),
            published: vec![false; routes.len()],
            scoped_routes: vec![],
        };
        let mut resource = Self {
            fd,
            name,
            settings,
            dns,
            key: format!("State:/Network/Service/com.peakpassvpn.capture.{token}/DNS"),
            published: vec![false; routes.len()],
            routes,
            dns_live: false,
            admitted: false,
            journal,
        };
        if let Err(e) = resource.configure() {
            diagnostic("utun-configure", &e);
            let _ = resource.cleanup();
            return Err(e);
        }
        resource
            .save()
            .inspect_err(|e| diagnostic("journal-save", e))?;
        trace_phase("resource-prepared");
        Ok(resource)
    }
    fn save(&mut self) -> Result<()> {
        self.journal.published.clone_from(&self.published);
        persist(&self.journal)
    }
    fn configure(&self) -> Result<()> {
        let address = self.settings.ipv4_address.split('/').next().unwrap();
        let mtu = self.settings.mtu.to_string();
        trace_phase("ifconfig-v4");
        command(
            "/sbin/ifconfig",
            &[
                &self.name,
                "inet",
                address,
                "10.60.159.90",
                "netmask",
                "255.255.255.252",
                "mtu",
                &mtu,
                "up",
            ],
        )?;
        if let Some(address) = &self.settings.ipv6_address {
            trace_phase("ifconfig-v6");
            command("/sbin/ifconfig", &[&self.name, "inet6", address])?;
        }
        trace_phase("ifconfig-ownership-marker");
        command(
            "/sbin/ifconfig",
            &[
                &self.name,
                "inet6",
                &format!("{}/128", self.journal.marker),
                "alias",
            ],
        )?;
        Ok(())
    }
    pub fn heal(&mut self) -> Result<()> {
        if self.admitted {
            self.attach()?;
        }
        Ok(())
    }
    pub fn attach(&mut self) -> Result<()> {
        if !marked(&self.journal)? {
            bail!("CAPTURE_DEVICE_OWNERSHIP_LOST");
        }
        self.ensure_scoped_uplinks()?;
        for i in 0..self.routes.len() {
            if self.published[i] {
                let (family, prefix) = self.routes[i];
                if !route_owned(&self.name, family, prefix)? {
                    self.published[i] = false;
                }
            }
            if !self.published[i] {
                // Persist intent before mutation, including an ambiguous child
                // timeout/crash. Rollback checks actual route ownership.
                self.published[i] = true;
                self.save()?;
                let (family, prefix) = self.routes[i];
                command(
                    "/sbin/route",
                    &[
                        "-n",
                        "add",
                        family,
                        "-net",
                        prefix,
                        "-interface",
                        &self.name,
                    ],
                )?;
            }
        }
        if self.dns.copy(&self.key).is_none() {
            self.dns_live = false;
        }
        if !self.dns_live {
            self.dns.add_dns(&self.key, &self.settings.dns_servers)?;
            self.dns_live = true;
        }
        self.admitted = true;
        Ok(())
    }
    fn ensure_scoped_uplinks(&mut self) -> Result<()> {
        let candidates =
            scoped_candidates(&self.dns, &self.name, self.settings.ipv6_address.is_some())?;
        // A transient offline state retains the last owned scopes. The next
        // renewal follows the physical primary again without restarting core.
        if candidates.is_empty() {
            return Ok(());
        }
        for old in &mut self.journal.scoped_routes {
            if !candidates.iter().any(|new| new.same_route(old)) {
                clear_scoped_route(old)?;
            }
        }
        self.journal
            .scoped_routes
            .retain(|old| candidates.iter().any(|new| new.same_route(old)));
        for candidate in candidates {
            if !self
                .journal
                .scoped_routes
                .iter()
                .any(|old| old.same_route(&candidate))
            {
                self.journal.scoped_routes.push(candidate);
            }
        }
        self.save()?;
        for i in 0..self.journal.scoped_routes.len() {
            let current = &self.journal.scoped_routes[i];
            let args = scoped_arguments(current, "get");
            if let Some(text) = route_query("/sbin/route", &args)? {
                if scoped_matches(&text, current, false) {
                    trace_phase("scoped-uplink-present");
                    continue;
                }
            }
            // Journal intent before add. Never replace or change a route that
            // another owner installed; EEXIST rejects this transaction.
            self.journal.scoped_routes[i].owned = true;
            self.save()?;
            let args = scoped_arguments(&self.journal.scoped_routes[i], "add");
            if let Err(error) = command("/sbin/route", &args) {
                // A completed failed add (including EEXIST) never transfers
                // another writer's route to us. A timed-out add is ambiguous
                // and retains its journal intent for fenced recovery.
                if error.to_string() == "OS_COMMAND_FAILED" {
                    self.journal.scoped_routes[i].owned = false;
                    self.save()?;
                }
                return Err(error);
            }
            trace_phase("scoped-uplink-added");
        }
        Ok(())
    }
    pub fn detach(&mut self) -> Result<()> {
        self.admitted = false;
        if !marked(&self.journal)? {
            bail!("CAPTURE_DEVICE_OWNERSHIP_LOST");
        }
        let dns = checked("detach-dns", || self.dns.remove(&self.key));
        if dns.is_ok() {
            self.dns_live = false;
        }
        let routes = checked("detach-routes", || {
            clear_routes(&self.name, &self.routes, &mut self.published)
        });
        let scopes = checked("detach-uplinks", || {
            clear_scoped_routes(&mut self.journal.scoped_routes)
        });
        checked("detach-journal", || self.save())?;
        dns?;
        routes?;
        scopes
    }
    pub fn cleanup(&mut self) -> Result<()> {
        // Never bring a recycled/externally changed interface down merely
        // because route/DNS cleanup failed its ownership check.
        if !marked(&self.journal)? {
            bail!("CAPTURE_DEVICE_OWNERSHIP_LOST");
        }
        let routes = self.detach();
        let down = command("/sbin/ifconfig", &[&self.name, "down"]);
        routes?;
        down?;
        forget_journal()
    }
}

impl super::capture::Resources for Resources {
    fn cleanup(&mut self) -> std::result::Result<(), String> {
        Resources::cleanup(self).map_err(|_| "CAPTURE_LEDGER_UNSETTLED".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_utun_not_arbitrary_descriptor() {
        let file = std::fs::File::open("/dev/null").unwrap();
        assert!(interface(&file.into()).is_err());
    }
    #[test]
    fn physical_snapshot_is_read_only() {
        let _ = physical();
    }
}

const JOURNAL: &str = super::broker::JOURNAL;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    name: String,
    index: u32,
    marker: std::net::Ipv6Addr,
    boot: i64,
    key: String,
    settings: Settings,
    published: Vec<bool>,
    #[serde(default)]
    scoped_routes: Vec<ScopedRoute>,
}
fn boot() -> Result<i64> {
    let mut value: libc::timeval = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&value);
    if unsafe {
        libc::sysctlbyname(
            c"kern.boottime".as_ptr(),
            (&mut value as *mut libc::timeval).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(value.tv_sec)
}
fn index(name: &str) -> Result<u32> {
    Ok(unsafe { libc::if_nametoindex(CString::new(name)?.as_ptr()) })
}
fn marked(j: &Journal) -> Result<bool> {
    if index(&j.name)? != j.index {
        return Ok(false);
    }
    let mut head = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut node = head;
    let mut matches = false;
    while !node.is_null() {
        let a = unsafe { &*node };
        if !a.ifa_addr.is_null()
            && unsafe { CStr::from_ptr(a.ifa_name) }.to_bytes() == j.name.as_bytes()
            && unsafe { (*a.ifa_addr).sa_family as i32 } == libc::AF_INET6
        {
            let address = unsafe { &*a.ifa_addr.cast::<libc::sockaddr_in6>() };
            matches |= address.sin6_addr.s6_addr == j.marker.octets();
        }
        node = a.ifa_next;
    }
    unsafe { libc::freeifaddrs(head) };
    Ok(matches)
}
fn persist(j: &Journal) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let temporary = format!("{JOURNAL}.{}.tmp", uuid::Uuid::new_v4());
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(j)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, JOURNAL)?;
        std::fs::File::open(super::broker::DIRECTORY)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
fn forget_journal() -> Result<()> {
    match std::fs::remove_file(JOURNAL) {
        Ok(()) => {
            std::fs::File::open(super::broker::DIRECTORY)?.sync_all()?;
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopedRoute {
    family: String,
    interface: String,
    index: u32,
    gateway: String,
    owned: bool,
}
impl ScopedRoute {
    fn validate(&self) -> Result<()> {
        if self.index == 0
            || self.interface.is_empty()
            || self.interface.len() >= libc::IFNAMSIZ
            || !self.interface.bytes().all(|c| c.is_ascii_alphanumeric())
            || self.interface.starts_with("utun")
            || self.interface.starts_with("tun")
            || self.interface.starts_with("ppp")
        {
            bail!("CAPTURE_UPLINK_INVALID");
        }
        let (address, zone) = self
            .gateway
            .split_once('%')
            .map_or((self.gateway.as_str(), None), |(ip, zone)| (ip, Some(zone)));
        let address: std::net::IpAddr = address
            .parse()
            .map_err(|_| anyhow::anyhow!("CAPTURE_UPLINK_INVALID"))?;
        if address.is_unspecified()
            || address.is_loopback()
            || address.is_multicast()
            || (self.family != "-inet" && self.family != "-inet6")
            || (self.family == "-inet") != address.is_ipv4()
            || zone.is_some_and(|zone| {
                self.family != "-inet6"
                    || (zone != self.interface && zone != self.index.to_string())
            })
        {
            bail!("CAPTURE_UPLINK_INVALID");
        }
        Ok(())
    }
    fn same_route(&self, other: &Self) -> bool {
        self.family == other.family
            && self.interface == other.interface
            && self.index == other.index
            && scoped_gateway_matches(&self.gateway, other)
    }
}
fn scoped_candidates(store: &Store, own_interface: &str, ipv6: bool) -> Result<Vec<ScopedRoute>> {
    let mut routes = vec![];
    for (family, key) in [
        ("-inet", "State:/Network/Global/IPv4"),
        ("-inet6", "State:/Network/Global/IPv6"),
    ] {
        if family == "-inet6" && !ipv6 {
            continue;
        }
        let Some(global) = store.dictionary(key) else {
            continue;
        };
        let (Some(interface), Some(mut gateway)) =
            (field(&global, "PrimaryInterface"), field(&global, "Router"))
        else {
            continue;
        };
        if interface == own_interface {
            continue;
        }
        if family == "-inet6"
            && gateway
                .parse::<std::net::Ipv6Addr>()
                .is_ok_and(|ip| ip.is_unicast_link_local())
        {
            gateway.push('%');
            gateway.push_str(&interface);
        }
        let route = ScopedRoute {
            family: family.into(),
            index: index(&interface)?,
            interface,
            gateway,
            owned: false,
        };
        route.validate()?;
        routes.push(route);
    }
    Ok(routes)
}
fn scoped_arguments<'a>(route: &'a ScopedRoute, operation: &'a str) -> Vec<&'a str> {
    let mut args = vec![
        "-n",
        operation,
        route.family.as_str(),
        "-ifscope",
        route.interface.as_str(),
        "-net",
        "default",
    ];
    if operation != "get" {
        args.push(&route.gateway);
    }
    args
}
fn scoped_gateway_matches(actual: &str, route: &ScopedRoute) -> bool {
    let split = |value: &str| {
        let (address, zone) = value
            .split_once('%')
            .map_or((value, None), |(ip, zone)| (ip, Some(zone)));
        (
            address.parse::<std::net::IpAddr>().ok(),
            zone.map(str::to_owned),
        )
    };
    let (actual, actual_zone) = split(actual);
    let (expected, expected_zone) = split(&route.gateway);
    actual.is_some()
        && actual == expected
        && (actual.is_some_and(|ip| ip.is_ipv6())
            || (actual_zone.is_none() && expected_zone.is_none()))
        && [actual_zone, expected_zone].iter().all(|zone| {
            zone.as_ref()
                .is_none_or(|zone| zone == &route.interface || zone == &route.index.to_string())
        })
}
fn scoped_matches(text: &str, route: &ScopedRoute, require_gateway: bool) -> bool {
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(key).map(str::trim))
    };
    let default = matches!(field("destination:"), Some("default" | "0.0.0.0" | "::"));
    let mask = field("mask:").is_none_or(|mask| matches!(mask, "default" | "0.0.0.0" | "::" | "0"));
    let scoped = field("flags:").is_some_and(|flags| {
        flags
            .trim_matches(['<', '>'])
            .split(',')
            .any(|flag| flag.trim() == "IFSCOPE")
    });
    default
        && mask
        && scoped
        && field("interface:") == Some(route.interface.as_str())
        && (!require_gateway
            || field("gateway:").is_some_and(|gateway| scoped_gateway_matches(gateway, route)))
}
fn clear_scoped_route(route: &mut ScopedRoute) -> Result<()> {
    if !route.owned {
        return Ok(());
    }
    if index(&route.interface)? != route.index {
        route.owned = false;
        return Ok(());
    }
    if route_query("/sbin/route", &scoped_arguments(route, "get"))?
        .is_some_and(|text| scoped_matches(&text, route, true))
    {
        command("/sbin/route", &scoped_arguments(route, "delete"))?;
        trace_phase("scoped-uplink-deleted");
    }
    route.owned = false;
    Ok(())
}
fn clear_scoped_routes(routes: &mut [ScopedRoute]) -> Result<()> {
    let mut failed = false;
    for route in routes.iter_mut().rev() {
        let stage = if route.family == "-inet" {
            "uplink-clear-v4"
        } else {
            "uplink-clear-v6"
        };
        if checked(stage, || clear_scoped_route(route)).is_err() {
            failed = true;
        }
    }
    if failed {
        bail!("CAPTURE_UPLINK_ROUTES_UNSETTLED");
    }
    Ok(())
}
fn route_query(_program: &str, args: &[&str]) -> Result<Option<String>> {
    super::route_snapshot::query(args)
}
#[cfg(test)]
pub(super) fn query_test(program: &str, args: &[&str]) -> Result<Option<String>> {
    route_query(program, args)
}
/// A longest-prefix result is not necessarily the requested route. Verify the
/// exact /1 mask and network as well as the unique owned interface before delete.
fn route_owned(name: &str, family: &str, prefix: &str) -> Result<bool> {
    Ok(
        route_query("/sbin/route", &["-n", "get", family, "-net", prefix])?
            .is_some_and(|text| route_matches(&text, name, family, prefix)),
    )
}
fn route_matches(text: &str, name: &str, family: &str, prefix: &str) -> bool {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key).map(str::trim))
    };
    let destination = field("destination:").unwrap_or_default();
    let mask = field("mask:").unwrap_or_default();
    let network = prefix.split('/').next().unwrap();
    let exact = if family == "-inet" {
        mask == "128.0.0.0"
            && (destination == network || (network == "0.0.0.0" && destination == "default"))
    } else {
        (mask == "8000::" || mask == "1")
            && (destination == network
                || destination == prefix
                || (network == "::" && destination == "default"))
    };
    exact && field("interface:") == Some(name)
}
fn settle_route(
    published: &mut bool,
    query: Result<bool>,
    delete: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if query? {
        delete()?;
    }
    *published = false;
    Ok(())
}
fn clear_routes(name: &str, routes: &[(&str, &str)], published: &mut [bool]) -> Result<()> {
    let mut failed = false;
    for i in (0..routes.len()).rev() {
        if published[i] {
            let (family, prefix) = routes[i];
            let query = checked(capture_route_stage(family, prefix, false), || {
                route_owned(name, family, prefix)
            });
            if settle_route(&mut published[i], query, || {
                checked(capture_route_stage(family, prefix, true), || {
                    command(
                        "/sbin/route",
                        &["-n", "delete", family, "-net", prefix, "-interface", name],
                    )
                })
            })
            .is_err()
            {
                failed = true;
            }
        }
    }
    if failed {
        bail!("CAPTURE_ROUTES_UNSETTLED");
    }
    Ok(())
}
#[cfg(test)]
mod settlement_tests {
    use super::*;
    #[test]
    fn unknown_query_or_delete_failure_preserves_owned_intent() {
        for code in [
            "ROUTE_QUERY_TIMEOUT",
            "ROUTE_SNAPSHOT_MALFORMED",
            "ROUTE_QUERY_FAILED",
        ] {
            let mut published = true;
            assert!(
                settle_route(&mut published, Err(anyhow::anyhow!(code)), || panic!(
                    "unknown route must not be deleted"
                ))
                .is_err()
            );
            assert!(published);
        }
        let mut published = true;
        assert!(settle_route(&mut published, Ok(true), || bail!("OS_COMMAND_TIMEOUT")).is_err());
        assert!(published);
        settle_route(&mut published, Ok(false), || {
            panic!("proven absent/foreign route must not be deleted")
        })
        .unwrap();
        assert!(!published);
    }
}
/// Only an idle, root-locked broker invokes recovery. An unpredictable /128
/// interface marker plus boot identity and interface index fences off recycled
/// utun names. No blanket route/DNS cleanup is allowed.
pub fn recover() -> Result<()> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(JOURNAL)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != 0
        || m.mode() & 0o077 != 0
        || m.nlink() != 1
        || m.len() > 16 * 1024
    {
        bail!("CAPTURE_JOURNAL_UNSAFE");
    }
    let mut bytes = vec![];
    file.take(16 * 1024).read_to_end(&mut bytes)?;
    let mut j: Journal = serde_json::from_slice(&bytes)?;
    j.settings.validate()?;
    if j.scoped_routes.len() > 2 {
        bail!("CAPTURE_JOURNAL_INVALID");
    }
    for route in &j.scoped_routes {
        route.validate()?;
    }
    if !j.name.starts_with("utun")
        || !j.name[4..].bytes().all(|b| b.is_ascii_digit())
        || j.name.len() > 15
        || !j
            .key
            .starts_with("State:/Network/Service/com.peakpassvpn.capture.")
        || !j.key.ends_with("/DNS")
    {
        bail!("CAPTURE_JOURNAL_INVALID");
    }
    if j.boot != boot()? {
        return forget_journal();
    }
    if index(&j.name)? == 0 {
        // Interface routes die with utun, physical scopes do not. They are
        // independently fenced by the recorded physical index and gateway.
        clear_scoped_routes(&mut j.scoped_routes)?;
        Store::new()?.remove(&j.key)?;
        return forget_journal();
    }
    if !marked(&j)? {
        bail!("CAPTURE_RECOVERY_OWNERSHIP_UNVERIFIED");
    }
    let mut routes = vec![("-inet", "0.0.0.0/1"), ("-inet", "128.0.0.0/1")];
    if j.settings.ipv6_address.is_some() {
        routes.extend([("-inet6", "::/1"), ("-inet6", "8000::/1")]);
    }
    if j.published.len() != routes.len() {
        bail!("CAPTURE_JOURNAL_INVALID");
    }
    let dns = Store::new()?;
    let removed = dns.remove(&j.key);
    let routes = clear_routes(&j.name, &routes, &mut j.published);
    let scopes = clear_scoped_routes(&mut j.scoped_routes);
    let down = if marked(&j)? {
        command("/sbin/ifconfig", &[&j.name, "down"])
    } else {
        Err(anyhow::anyhow!("CAPTURE_RECOVERY_OWNERSHIP_UNVERIFIED"))
    };
    removed?;
    routes?;
    scopes?;
    down?;
    forget_journal()
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn the_broker_settings_match_the_public_core_tunnel_contract() {
        let core = ppvpn_core::tun_settings(ppvpn_core::Platform::Ios).unwrap();
        let settings = Settings {
            mtu: core.mtu,
            ipv4_address: core.ipv4_address,
            ipv6_address: Some(core.ipv6_address),
            dns_servers: core.dns_servers,
        };
        settings.validate().unwrap();
        let mut invalid = settings.clone();
        invalid.mtu = 1500;
        assert!(invalid.validate().is_err());
        let mut invalid = settings;
        invalid.dns_servers = vec!["192.0.2.1".into()];
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn scoped_uplink_ownership_never_accepts_global_or_foreign_routes() {
        let route = ScopedRoute {
            family: "-inet".into(),
            interface: "en0".into(),
            index: 7,
            gateway: "192.168.64.1".into(),
            owned: true,
        };
        route.validate().unwrap();
        let exact="destination: default\ngateway: 192.168.64.1\ninterface: en0\nflags: <UP,GATEWAY,STATIC,IFSCOPE>\n";
        assert!(scoped_matches(exact, &route, true));
        assert!(!scoped_matches(
            &exact.replace(",IFSCOPE", ""),
            &route,
            true
        ));
        assert!(!scoped_matches(&exact.replace("en0", "en1"), &route, true));
        assert!(!scoped_matches(
            &exact.replace("192.168.64.1", "192.168.64.2"),
            &route,
            true
        ));
        assert!(scoped_matches(
            &exact.replace("192.168.64.1", "192.168.64.2"),
            &route,
            false
        ));
        assert!(!scoped_matches(
            &exact.replace(
                "destination: default",
                "destination: default\nmask: 128.0.0.0"
            ),
            &route,
            true
        ));
        assert_eq!(
            scoped_arguments(&route, "add"),
            vec![
                "-n",
                "add",
                "-inet",
                "-ifscope",
                "en0",
                "-net",
                "default",
                "192.168.64.1"
            ]
        );
        assert_eq!(
            scoped_arguments(&route, "delete"),
            vec![
                "-n",
                "delete",
                "-inet",
                "-ifscope",
                "en0",
                "-net",
                "default",
                "192.168.64.1"
            ]
        );
        assert!(!scoped_gateway_matches("192.168.64.1%en0", &route));
        let v6 = ScopedRoute {
            family: "-inet6".into(),
            interface: "en0".into(),
            index: 7,
            gateway: "fe80::1%en0".into(),
            owned: true,
        };
        v6.validate().unwrap();
        assert!(scoped_gateway_matches(
            "fe80:0000:0000:0000:0000:0000:0000:0001%7",
            &v6
        ));
        assert!(!scoped_gateway_matches("fe80::1%en1", &v6));
        assert!(!scoped_gateway_matches("fe80::2%en0", &v6));
        let mut borrowed = route.clone();
        borrowed.owned = false;
        clear_scoped_route(&mut borrowed).unwrap();
        let mut invalid = route.clone();
        invalid.gateway = "-delete".into();
        assert!(invalid.validate().is_err());
        let mut invalid = route;
        invalid.interface = "utun7".into();
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn legacy_journal_has_no_owned_physical_scopes() {
        let core = ppvpn_core::tun_settings(ppvpn_core::Platform::Ios).unwrap();
        let value = serde_json::json!({"name":"utun99","index":99,"marker":"fd00::1","boot":1,"key":"fixture","settings":{"mtu":core.mtu,"ipv4_address":core.ipv4_address,"ipv6_address":core.ipv6_address,"dns_servers":core.dns_servers},"published":[true,true,true,true]});
        assert!(serde_json::from_value::<Journal>(value)
            .unwrap()
            .scoped_routes
            .is_empty());
    }
    #[test]
    fn cleanup_never_accepts_a_default_fallback_or_foreign_interface() {
        let exact = "destination: 128.0.0.0\nmask: 128.0.0.0\ninterface: utun99\n";
        assert!(route_matches(exact, "utun99", "-inet", "128.0.0.0/1"));
        assert!(!route_matches(exact, "utun98", "-inet", "128.0.0.0/1"));
        assert!(!route_matches(
            "destination: default\nmask: default\ninterface: utun99\n",
            "utun99",
            "-inet",
            "0.0.0.0/1"
        ));
        assert!(!route_matches(exact, "utun99", "-inet", "0.0.0.0/1"));
        assert!(route_matches(
            "destination: 8000::\nmask: 8000::\ninterface: utun99\n",
            "utun99",
            "-inet6",
            "8000::/1"
        ));
    }
}
