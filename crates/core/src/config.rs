//! How a host creates an instance (docs/host-integration.md, sections 2 and
//! 3). The structs are `non_exhaustive`: build them with `new` and the
//! `with_*` setters, or deserialise them (FFI).

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Which instance this is (section 2). Desktop runs one of each; the CLI
/// runs a standard instance only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Role {
    /// Unprivileged: the shared local proxy, the system proxy listener,
    /// probes, traffic and connections.
    Standard,
    /// Privileged: the TUN, its routing guard (Linux), DNS inside the TUN
    /// (dns-local) and the switch in place. At most one per process
    /// (`TUN_INSTANCE_EXISTS`).
    Tun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Platform {
    Linux,
    Macos,
    Windows,
    Ios,
    Android,
}

/// The configuration of [`crate::Engine::new`] (section 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EngineConfig {
    pub role: Role,
    pub platform: Platform,
    /// Private directory: rule set cache and local proxy state. Locked by
    /// the instance (`STATE_DIR_IN_USE`). No profile is ever written here.
    pub state_dir: PathBuf,
    /// Standard only: the shared local proxy.
    #[serde(default)]
    pub local_proxy: Option<LocalProxyConfig>,
    /// Standard only: whether `set_system_proxy_listener` is allowed (the
    /// CLI leaves it off).
    #[serde(default)]
    pub system_proxy: bool,
    /// Tun only.
    #[serde(default)]
    pub tun: Option<TunConfig>,
    #[serde(default)]
    pub log: LogConfig,
    /// sail's cap on live proxied sessions (`inbound.max_connections`):
    /// connections, UDP sessions and multiplexed streams; 0 = no cap. None:
    /// sail's profile's (none on desktop, 512 on mobile). An iOS network
    /// extension sets 128 (#388: the system ends it at 50 MiB).
    #[serde(default)]
    pub max_connections: Option<u32>,
    /// Android: keeps each outbound socket out of the VPN before it
    /// connects (`VpnService.protect`). Not serialised: an FFI layer sets it.
    #[serde(skip)]
    pub socket_protect: Option<SocketProtect>,
}

impl EngineConfig {
    pub fn new(role: Role, platform: Platform, state_dir: impl Into<PathBuf>) -> Self {
        Self {
            role,
            platform,
            state_dir: state_dir.into(),
            local_proxy: None,
            system_proxy: false,
            tun: None,
            log: LogConfig::default(),
            max_connections: None,
            socket_protect: None,
        }
    }
    pub fn with_local_proxy(mut self, local_proxy: LocalProxyConfig) -> Self {
        self.local_proxy = Some(local_proxy);
        self
    }
    pub fn with_system_proxy(mut self, enabled: bool) -> Self {
        self.system_proxy = enabled;
        self
    }
    pub fn with_tun(mut self, tun: TunConfig) -> Self {
        self.tun = Some(tun);
        self
    }
    pub fn with_log(mut self, log: LogConfig) -> Self {
        self.log = log;
        self
    }
    pub fn with_max_connections(mut self, max: u32) -> Self {
        self.max_connections = Some(max);
        self
    }
    pub fn with_socket_protect(mut self, protect: SocketProtect) -> Self {
        self.socket_protect = Some(protect);
        self
    }
}

/// The host's socket protection (mobile hosts, section 11): called with
/// each outbound socket's descriptor before it connects, on sail's threads;
/// an error fails that dial. It must not call the instance. Clones share
/// the callback; two are equal when they are the same callback.
#[derive(Clone)]
pub struct SocketProtect(Arc<dyn Fn(i32) -> std::io::Result<()> + Send + Sync>);

impl SocketProtect {
    pub fn new(protect: impl Fn(i32) -> std::io::Result<()> + Send + Sync + 'static) -> Self {
        Self(Arc::new(protect))
    }
    pub(crate) fn protect(&self, fd: i32) -> std::io::Result<()> {
        (self.0)(fd)
    }
}

impl std::fmt::Debug for SocketProtect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SocketProtect(..)")
    }
}

impl PartialEq for SocketProtect {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The shared local proxy (section 4.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LocalProxyConfig {
    /// Default `127.0.0.1`.
    pub listen: String,
    /// Tried after the persisted port: default 7890; 0 = any free port (tests).
    pub preferred_port: u16,
}

impl Default for LocalProxyConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1".into(),
            preferred_port: 7890,
        }
    }
}

impl LocalProxyConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_listen(mut self, listen: impl Into<String>) -> Self {
        self.listen = listen.into();
        self
    }
    pub fn with_preferred_port(mut self, port: u16) -> Self {
        self.preferred_port = port;
        self
    }
}

/// The TUN (sections 2, 3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TunConfig {
    /// A static override of dns-local's servers (IP, IP:port or
    /// `[IPv6%zone]:port`), as `--local-dns-servers`; empty = follow the
    /// default interface.
    #[serde(default)]
    pub local_dns_servers: Vec<String>,
    /// Windows: the wintun.dll the host ships (same version as sail's
    /// `WINTUN_VERSION`); the engine never downloads or embeds it.
    #[serde(default)]
    pub wintun_dll: Option<PathBuf>,
    /// iOS and Android: the TUN the host opened with
    /// [`crate::tun_settings`]. The engine hands sail a duplicate at each
    /// start and never closes this one: the host closes it after
    /// `shutdown`. Without it sail opens the device itself, which only a
    /// lab host may (Linux, privileged).
    #[serde(default)]
    pub fd: Option<i32>,
}

impl TunConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_local_dns_servers(mut self, servers: Vec<String>) -> Self {
        self.local_dns_servers = servers;
        self
    }
    pub fn with_wintun_dll(mut self, path: impl Into<PathBuf>) -> Self {
        self.wintun_dll = Some(path.into());
        self
    }
    pub fn with_fd(mut self, fd: i32) -> Self {
        self.fd = Some(fd);
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LogLevel {
    #[default]
    Info,
    /// One line per connection and DNS query, domains included: diagnosis only.
    Debug,
}

/// Where the log lines go (section 10): logfmt lines, never blocking the
/// data plane (dropped lines are counted in `Status::dropped_log_lines`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LogSink {
    /// No log.
    #[default]
    None,
    /// Appended to this file; the host rotates it.
    File { path: PathBuf },
    /// Read with [`crate::Engine::logs`].
    Channel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LogConfig {
    #[serde(default)]
    pub level: LogLevel,
    #[serde(default)]
    pub sink: LogSink,
}

impl LogConfig {
    pub fn new(level: LogLevel, sink: LogSink) -> Self {
        Self { level, sink }
    }
}
