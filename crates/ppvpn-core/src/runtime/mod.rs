//! What the Engine needs from a running sail, and nothing more (crate
//! private). The sail implementation is over `sail::embed`; the Engine's own
//! tests use `fake::FakeRuntime`. Shapes follow sail's docs/embed.md:
//! subscribe, then read; a failed reload changes nothing; a dial goes
//! through one named outbound, whatever the rules say.
//!
//! A Runtime passes sail's error codes through as they are; turning them into
//! the Engine's codes is [`RuntimeError::to_error`], here, not in the
//! implementation.

#![allow(dead_code)] // the Engine is wired to it with the sail implementation

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, watch};

use crate::error::{codes, Error};

/// For tests only: this crate's own, and (with the `testing` feature, which
/// the crate's dev-dependency on itself turns on) the golden contract tests
/// in tests/, through `internal`. Never in a host's build.
#[cfg(any(test, feature = "testing"))]
pub(crate) mod fake;
pub(crate) mod sail;

/// Where a runtime is in its life (`sail::embed::State`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeState {
    Idle,
    Starting,
    Running,
    Stopping,
    Stopped,
    /// It did not start, or ended on its own. `code` is sail's
    /// `ErrorKind::code()`; `panicked` makes the Engine Fatal{Panic}.
    Failed {
        code: String,
        message: String,
    },
}

/// sail's error: its stable code (`ErrorKind::code()`) and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeError {
    pub code: String,
    pub message: String,
}

impl RuntimeError {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// The Engine's error for a lifecycle or query call. A `config` error is
    /// the translation's fault, not the host's: CORE_OPERATION_FAILED, with
    /// sail's reason in the message (and the log).
    pub(crate) fn to_error(&self) -> Error {
        let (code, retryable) = match self.code.as_str() {
            "panicked" => (codes::CORE_PANICKED, false),
            "not_running" => (codes::CORE_NOT_RUNNING, false),
            "timeout" | "io" => (codes::CORE_OPERATION_FAILED, true),
            _ => (codes::CORE_OPERATION_FAILED, false),
        };
        Error::new(
            code,
            retryable,
            format!("sail {}: {}", self.code, self.message),
        )
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// Where a dial goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Domain(String, u16),
    Addr(SocketAddr),
}

/// A stream dialled through one outbound.
pub(crate) trait AsyncReadWrite: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> AsyncReadWrite for T {}

/// A UDP association through one outbound, to the dialled target.
#[async_trait]
pub(crate) trait Datagram: Send + Sync {
    async fn send(&self, data: &[u8]) -> std::io::Result<()>;
    async fn recv(&self, buf: &mut [u8]) -> std::io::Result<usize>;
}

/// A selector or fallback group: failover status, the active ingress, pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupInfo {
    pub tag: String,
    /// The member in use now.
    pub now: String,
    /// In the group's order.
    pub members: Vec<MemberInfo>,
    /// Pinned by `select`; `unfix` returns it to automatic.
    pub fixed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberInfo {
    pub tag: String,
    /// None: never checked.
    pub alive: Option<bool>,
    pub last_check: Option<SystemTime>,
    pub consecutive_failures: u32,
}

/// A group moved from one member to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupSwitch {
    pub group: String,
    pub from: String,
    pub to: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RuntimeTraffic {
    pub upload_bytes: u64,
    pub download_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeConnection {
    pub id: u64,
    pub inbound: String,
    /// Outbound tags, outermost first; the Engine maps them back to nodes
    /// with the translation's tag map.
    pub chain: Vec<String>,
    /// `tcp` or `udp`.
    pub network: String,
    pub destination: String,
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub started: SystemTime,
}

/// The network as sail sees it: the default route's interface and what
/// identifies the network on it. `offline` when sail knew a network and now
/// sees none (no default interface, address or type).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NetworkSnapshot {
    pub interface: Option<String>,
    pub index: Option<u32>,
    pub gateway: Option<IpAddr>,
    /// With their prefixes (`192.168.1.2/24`).
    pub addresses: Vec<String>,
    pub offline: bool,
}

/// A change of network that sail acts on (its DNS cache cleared, the
/// connections of the old network reset): the same publication sail's own
/// reaction reads, so the Engine's NetworkChanged and sail's reset come
/// from one decision (one monitor, #45).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkChange {
    /// From 1 for each start of the runtime; a reader that sees a gap
    /// missed changes in between (the channel keeps the latest only).
    pub generation: u64,
    /// `default_interface`, `state`, `host` (pushed) or `wake`.
    pub reason: String,
    pub old: NetworkSnapshot,
    pub new: NetworkSnapshot,
}

/// A running sail. Every method may be called from any task; the
/// implementation serialises what sail needs serialised.
#[async_trait]
pub(crate) trait Runtime: Send + Sync + 'static {
    /// Runs `config` (sing-box JSON, the translation's output). An error
    /// leaves it not running.
    async fn start(&self, config: &str) -> Result<(), RuntimeError>;
    /// In place: listeners and open connections kept; group selections and
    /// pins kept for groups of the same tag; the DNS cache emptied. A failed
    /// reload changes nothing.
    async fn reload(&self, config: &str) -> Result<(), RuntimeError>;
    /// Returns once listeners are closed and connections dropped.
    async fn stop(&self) -> Result<(), RuntimeError>;

    /// Subscribe first, then read.
    fn states(&self) -> watch::Receiver<RuntimeState>;
    fn state(&self) -> RuntimeState;

    async fn groups(&self) -> Result<Vec<GroupInfo>, RuntimeError>;
    /// Selects `member` (selector) or fixes it (fallback).
    async fn select(&self, group: &str, member: &str) -> Result<(), RuntimeError>;
    /// Back to automatic.
    async fn unfix(&self, group: &str) -> Result<(), RuntimeError>;
    /// Group switches as they happen. Taken once (by the Engine, at new).
    fn group_switches(&self) -> mpsc::Receiver<GroupSwitch>;

    async fn traffic(&self) -> Result<RuntimeTraffic, RuntimeError>;
    async fn connections(&self) -> Result<Vec<RuntimeConnection>, RuntimeError>;
    /// Closes one connection; false if it was already gone.
    async fn close_connection(&self, id: u64) -> Result<bool, RuntimeError>;

    /// Through `outbound` alone, whatever the rules say (probes).
    async fn dial_tcp(
        &self,
        outbound: &str,
        to: Target,
        timeout: Duration,
    ) -> Result<Box<dyn AsyncReadWrite>, RuntimeError>;
    async fn dial_udp(
        &self,
        outbound: &str,
        to: Target,
        timeout: Duration,
    ) -> Result<Box<dyn Datagram>, RuntimeError>;

    /// The users of `inbound` become `users` (name, password) without
    /// rebinding its listener. Not atomic: a step that fails leaves the
    /// steps before it done.
    async fn replace_inbound_users(
        &self,
        inbound: &str,
        users: Vec<(String, String)>,
    ) -> Result<(), RuntimeError>;

    /// The default interface changed; called by the Engine's one monitor.
    async fn network_changed(&self) -> Result<(), RuntimeError>;

    /// sail's log lines for this instance. Bounded: lines that do not fit
    /// are dropped and counted, never waited for. Taken once (by the Engine,
    /// at new).
    fn logs(&self) -> mpsc::Receiver<String>;
    /// Log lines dropped so far (`Status::dropped_log_lines`).
    fn dropped_log_lines(&self) -> u64;

    /// The TUN device's actual name while a configuration with a TUN runs:
    /// for the Engine's log and status, and tunrules' `iif` rules. None
    /// without a TUN, and where the name is not known yet: on macOS the
    /// kernel picks the utun and sail does not report it (until sail::embed
    /// does, the configured name is all there is, and macOS sets none).
    fn tun_name(&self) -> Option<String>;

    /// The network now; None while the runtime does not run.
    fn network(&self) -> Option<NetworkSnapshot>;
    /// Each change of network, latest only (see `NetworkChange::generation`);
    /// None until the first since the runtime started. The receiver lives
    /// across starts and stops.
    fn network_changes(&self) -> watch::Receiver<Option<NetworkChange>>;
}

/// The `interface_name` of the configuration's tun inbound, if it has one.
pub(crate) fn configured_tun_name(config: &str) -> Option<String> {
    let config: serde_json::Value = serde_json::from_str(config).ok()?;
    config["inbounds"]
        .as_array()?
        .iter()
        .find(|inbound| inbound["type"] == "tun")?["interface_name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tun_name_is_the_tun_inbounds() {
        let with = r#"{"inbounds":[{"type":"mixed","tag":"local"},{"type":"tun","tag":"tun","interface_name":"ppvpn0"}]}"#;
        assert_eq!(configured_tun_name(with).as_deref(), Some("ppvpn0"));
        let unnamed = r#"{"inbounds":[{"type":"tun","tag":"tun"}]}"#;
        assert_eq!(
            configured_tun_name(unnamed),
            None,
            "macOS: the kernel names it"
        );
        assert_eq!(
            configured_tun_name(r#"{"inbounds":[{"type":"mixed"}]}"#),
            None
        );
        assert_eq!(configured_tun_name("not json"), None);
    }

    #[test]
    fn sail_codes_map_to_engine_codes() {
        let cases = [
            ("config", codes::CORE_OPERATION_FAILED, false),
            ("panicked", codes::CORE_PANICKED, false),
            ("not_running", codes::CORE_NOT_RUNNING, false),
            ("timeout", codes::CORE_OPERATION_FAILED, true),
            ("internal", codes::CORE_OPERATION_FAILED, false),
        ];
        for (sail, code, retryable) in cases {
            let error = RuntimeError::new(sail, "why").to_error();
            assert_eq!((error.code, error.retryable), (code, retryable), "{sail}");
            assert_eq!(error.message, format!("sail {sail}: why"));
        }
    }
}
