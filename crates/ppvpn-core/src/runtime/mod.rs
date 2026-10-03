//! What the Engine needs from a running sail, and nothing more (crate
//! private). The sail implementation is over `sail::embed`; the Engine's own
//! tests use [`fake::FakeRuntime`]. Shapes follow sail's docs/embed.md:
//! subscribe, then read; a failed reload changes nothing; a dial goes
//! through one named outbound, whatever the rules say.
//!
//! A Runtime passes sail's error codes through as they are; turning them into
//! the Engine's codes is [`RuntimeError::to_error`], here, not in the
//! implementation.

#![allow(dead_code)] // the Engine is wired to it with the sail implementation

use std::fmt;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, watch};

use crate::error::{codes, Error};

/// Built outside tests too: the golden contract tests (tests/) drive an
/// Engine on it through `internal`.
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
