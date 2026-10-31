//! What a mobile host needs besides the common API (docs/host-integration.md,
//! section 11): the tunnel it builds, the network it pushes, and sail's
//! platform hooks over the TUN it opened and its socket protection.

use serde::{Deserialize, Serialize};

use crate::config::{EngineConfig, Platform, SocketProtect};
use crate::translate;

/// The tunnel a mobile host builds before `Engine::new` (Android's
/// `VpnService.Builder`, iOS's `NEPacketTunnelNetworkSettings`): the values
/// the engine's TUN configuration assumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TunSettings {
    pub mtu: u16,
    /// The tunnel's address with its prefix length.
    pub ipv4_address: String,
    /// What goes into the tunnel: everything. Private destinations go
    /// direct by the routing rules, out of the tunnel through the protected
    /// (Android) or the extension's own (iOS) sockets.
    pub ipv4_routes: Vec<String>,
    /// The tunnel's IPv6 address (a ULA) with its prefix length (#511).
    pub ipv6_address: String,
    /// Everything IPv6 into the tunnel too, so that no application's IPv6
    /// goes around it. On a network without IPv6 the engine dials direct
    /// IPv6 by its name over IPv4 (the hand-off, as on desktop).
    pub ipv6_routes: Vec<String>,
    /// The resolver the system is given: inside the tunnel, where the
    /// engine answers every query (hijack-dns).
    pub dns_servers: Vec<String>,
}

/// The tunnel of a TUN instance on `platform`; None on desktop, where the
/// engine builds it (auto_route).
pub fn tun_settings(platform: Platform) -> Option<TunSettings> {
    mobile(platform).then(|| TunSettings {
        mtu: translate::TUN_MTU,
        ipv4_address: translate::TUN_INET4_ADDRESS.into(),
        ipv4_routes: vec!["0.0.0.0/0".into()],
        ipv6_address: translate::TUN_INET6_ADDRESS.into(),
        ipv6_routes: vec!["::/0".into()],
        dns_servers: vec![translate::TUN_DNS_ADDRESS.into()],
    })
}

pub(crate) fn mobile(platform: Platform) -> bool {
    matches!(platform, Platform::Ios | Platform::Android)
}

/// What kind of network the host is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NetworkKind {
    Wifi,
    Cellular,
    Ethernet,
    Other,
}

/// The network a mobile host is on (`Engine::set_network_state`), from
/// `ConnectivityManager` or `NWPathMonitor`. Serialises as sail's network
/// state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NetworkState {
    /// The default interface's name; None: offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<NetworkKind>,
    /// Its addresses with their prefix lengths (`192.168.1.2/24`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// Metered, as the system says.
    #[serde(default)]
    pub expensive: bool,
    /// In a low data mode, as the system says.
    #[serde(default)]
    pub constrained: bool,
}

impl NetworkState {
    /// No default interface.
    pub fn offline() -> Self {
        Self::default()
    }
    /// On `interface`, of `kind`.
    pub fn new(interface: impl Into<String>, kind: NetworkKind) -> Self {
        Self {
            interface: Some(interface.into()),
            kind: Some(kind),
            ..Self::default()
        }
    }
    pub fn with_addresses(mut self, addresses: Vec<String>) -> Self {
        self.addresses = addresses;
        self
    }
    pub fn with_gateway(mut self, gateway: impl Into<String>) -> Self {
        self.gateway = Some(gateway.into());
        self
    }
    pub fn with_expensive(mut self, expensive: bool) -> Self {
        self.expensive = expensive;
        self
    }
    pub fn with_constrained(mut self, constrained: bool) -> Self {
        self.constrained = constrained;
        self
    }

    /// Whether the network reaches the IPv6 internet: it has a global
    /// unicast IPv6 address (the host gives no IPv6 default route, and
    /// an address of that scope comes from a router that offers one).
    /// None offline, where nothing reaches anything.
    pub(crate) fn ipv6_route(&self) -> Option<bool> {
        self.interface.as_ref()?;
        Some(self.addresses.iter().any(|address| {
            let ip = address
                .split_once('/')
                .map_or(address.as_str(), |(ip, _)| ip);
            ip.parse()
                .is_ok_and(|ip| crate::hostipv6::global_unicast(&ip))
        }))
    }
}

/// sail's platform over what the host gave: the TUN it opened and its
/// socket protection. None when it gave neither.
pub(crate) fn platform(config: &EngineConfig) -> Option<HostPlatform> {
    let tun_fd = config.tun.as_ref().and_then(|tun| tun.fd);
    let protect = config.socket_protect.clone();
    (tun_fd.is_some() || protect.is_some()).then_some(HostPlatform { tun_fd, protect })
}

pub(crate) struct HostPlatform {
    tun_fd: Option<i32>,
    protect: Option<SocketProtect>,
}

impl sail::embed::Platform for HostPlatform {
    /// Only with sail's `log_to_system`, which the engine never sets: its
    /// lines come through the instance's log.
    fn log(&self, line: &str) {
        tracing::info!(target: "sail", "{line}");
    }

    fn protects_sockets(&self) -> bool {
        self.protect.is_some()
    }

    fn protect_socket(&self, fd: i32) -> std::io::Result<()> {
        match &self.protect {
            Some(protect) => protect.protect(fd),
            None => Ok(()),
        }
    }

    fn opens_tun(&self) -> bool {
        self.tun_fd.is_some()
    }

    /// The host's TUN, duplicated: sail closes what it gets at its stop,
    /// and the next start (a restart, a kernel switch) gets another
    /// duplicate of the same device. The host built it from
    /// [`tun_settings`], which the configuration the request comes from
    /// follows.
    fn open_tun(&self, _request: &sail::embed::TunRequest) -> std::io::Result<i32> {
        match self.tun_fd {
            Some(fd) => duplicate(fd),
            None => Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        }
    }
}

#[cfg(unix)]
fn duplicate(fd: i32) -> std::io::Result<i32> {
    // SAFETY: F_DUPFD_CLOEXEC only reads `fd`; a bad one is EBADF.
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(copy)
    }
}

#[cfg(not(unix))]
fn duplicate(_fd: i32) -> std::io::Result<i32> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Role, TunConfig};
    use sail::embed::Platform as _;

    #[test]
    fn mobile_tun_settings_match_the_translation() {
        for platform in [Platform::Linux, Platform::Macos, Platform::Windows] {
            assert_eq!(tun_settings(platform), None);
        }
        let settings = tun_settings(Platform::Android).unwrap();
        assert_eq!(Some(settings.clone()), tun_settings(Platform::Ios));
        assert_eq!(settings.mtu, 9000);
        assert_eq!(settings.ipv4_address, "10.60.159.89/30");
        assert_eq!(settings.ipv4_routes, ["0.0.0.0/0"]);
        assert_eq!(settings.ipv6_address, "fde2:ec40:9312:c7fd::1/126");
        assert_eq!(settings.ipv6_routes, ["::/0"]);
        // The other host address of the tunnel's /30.
        assert_eq!(settings.dns_servers, ["10.60.159.90"]);
        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            serde_json::json!({
                "mtu": 9000,
                "ipv4_address": "10.60.159.89/30",
                "ipv4_routes": ["0.0.0.0/0"],
                "ipv6_address": "fde2:ec40:9312:c7fd::1/126",
                "ipv6_routes": ["::/0"],
                "dns_servers": ["10.60.159.90"],
            })
        );
    }

    #[test]
    fn an_ipv6_route_needs_a_global_address() {
        let wifi = |addresses: &[&str]| {
            NetworkState::new("wlan0", NetworkKind::Wifi)
                .with_addresses(addresses.iter().map(|a| a.to_string()).collect())
        };
        assert_eq!(NetworkState::offline().ipv6_route(), None);
        assert_eq!(wifi(&["192.168.1.2/24"]).ipv6_route(), Some(false));
        assert_eq!(
            wifi(&["192.168.1.2/24", "fe80::1/64", "fd00::2/64"]).ipv6_route(),
            Some(false)
        );
        assert_eq!(
            wifi(&["192.168.1.2/24", "2001:db8::2/64"]).ipv6_route(),
            Some(true)
        );
        assert_eq!(wifi(&["2001:db8::2"]).ipv6_route(), Some(true));
        assert_eq!(wifi(&["not an address"]).ipv6_route(), Some(false));
    }

    #[test]
    fn network_state_is_sails_json() {
        let wifi = NetworkState::new("wlan0", NetworkKind::Wifi)
            .with_addresses(vec!["192.168.1.2/24".into()])
            .with_gateway("192.168.1.1")
            .with_expensive(true);
        assert_eq!(
            serde_json::to_value(&wifi).unwrap(),
            serde_json::json!({
                "interface": "wlan0",
                "type": "wifi",
                "addresses": ["192.168.1.2/24"],
                "gateway": "192.168.1.1",
                "expensive": true,
                "constrained": false,
            })
        );
        // Offline names no interface, which is what sail reads as offline.
        assert_eq!(
            serde_json::to_value(NetworkState::offline()).unwrap(),
            serde_json::json!({"expensive": false, "constrained": false})
        );
        let back: NetworkState =
            serde_json::from_str(r#"{"interface": "rmnet0", "type": "cellular"}"#).unwrap();
        assert_eq!(back, NetworkState::new("rmnet0", NetworkKind::Cellular));
    }

    #[test]
    fn no_platform_without_a_tun_or_protection() {
        let config = EngineConfig::new(Role::Tun, Platform::Android, "/x");
        assert!(platform(&config).is_none());
        let config = config.with_tun(TunConfig::new().with_fd(7));
        let host = platform(&config).unwrap();
        assert!(host.opens_tun());
        assert!(!host.protects_sockets());
    }

    #[test]
    fn protection_goes_to_the_host() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        let config = EngineConfig::new(Role::Tun, Platform::Android, "/x").with_socket_protect(
            SocketProtect::new(move |fd| {
                record.lock().unwrap().push(fd);
                if fd < 0 {
                    Err(std::io::Error::other("refused"))
                } else {
                    Ok(())
                }
            }),
        );
        let host = platform(&config).unwrap();
        assert!(host.protects_sockets());
        assert!(!host.opens_tun());
        host.protect_socket(12).unwrap();
        assert!(host.protect_socket(-1).is_err());
        assert_eq!(*seen.lock().unwrap(), [12, -1]);
    }

    #[cfg(unix)]
    #[test]
    fn each_start_gets_its_own_duplicate() {
        let mut fds = [0; 2];
        // SAFETY: a fresh pair, closed below.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let config = EngineConfig::new(Role::Tun, Platform::Ios, "/x")
            .with_tun(TunConfig::new().with_fd(fds[1]));
        let host = platform(&config).unwrap();
        let request = sail::embed::TunRequest {
            name: String::new(),
            mtu: 9000,
            ipv4: None,
            ipv6: None,
            auto_route: false,
            include_package: Vec::new(),
            exclude_package: Vec::new(),
        };
        let first = host.open_tun(&request).unwrap();
        let second = host.open_tun(&request).unwrap();
        assert!(first != fds[1] && second != fds[1] && first != second);
        // sail closes its copy at stop; the host's stays usable.
        // SAFETY: closing descriptors this test owns.
        unsafe {
            libc::close(first);
            assert_eq!(libc::write(fds[1], b"x".as_ptr().cast(), 1), 1);
            libc::close(second);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }
}
