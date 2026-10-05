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

use crate::config::Platform;
use crate::error::{codes, Error};
use crate::translate;

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
            // A name we configured: another program holds it.
            "tun_name_taken" => (codes::TUN_NAME_TAKEN, false),
            _ => (codes::CORE_OPERATION_FAILED, false),
        };
        Error::new(
            code,
            retryable,
            format!("sail {}: {}", self.code, self.message),
        )
    }
}

impl RuntimeError {
    /// [`to_error`](Self::to_error) on `platform`: a TUN name taken is
    /// retryable where sail chose it (no `interface_name`; another free one
    /// may be had), not where we configured it.
    pub(crate) fn to_error_on(&self, platform: Platform) -> Error {
        let mut error = self.to_error();
        if error.code == codes::TUN_NAME_TAKEN {
            error.retryable = translate::interface_name(platform).is_empty();
        }
        error
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

/// Connections through one chain of outbounds that failed (#214
/// DialFailed): `count` since the one before for that chain, `last` the
/// latest of them.
/// A connection once the rules decided of it and, where they sent it to an
/// outbound, once its dial ended (sail's `Routed`): a TCP connection, a UDP
/// session or a stream of a multiplexed one. Addresses and domains whole:
/// the Engine redacts what it logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Routed {
    /// As `connections()` lists it; None where it never opened.
    pub id: Option<u64>,
    /// `tcp` or `udp`.
    pub network: String,
    /// The inbound's tag.
    pub inbound: String,
    /// `ip:port`.
    pub source: String,
    /// Where it was to go, as the rules saw it (`host:port`).
    pub destination: String,
    pub domain: Option<String>,
    /// `request`, `fake_ip`, `sniffed` or `reverse_mapping`.
    pub domain_source: Option<String>,
    /// The protocol sniffing recognized.
    pub protocol: Option<String>,
    /// The deciding rule's index in `route.rules` (a logical rule is one);
    /// None for `route.final`, or a dial of the host's own.
    pub rule: Option<usize>,
    /// `outbound`, `reject`, `drop` or `hijack_dns`.
    pub action: String,
    /// Outermost first, as `DialFailed::chain`: the outbound the rules named,
    /// then the member each group on the way took; the last carried it.
    /// Empty where none was asked. (sail's code at 2eb3fe47; its doc says
    /// the reverse order, asked of Sail.)
    pub chain: Vec<String>,
    /// What the outbound was asked to reach (`host:port`).
    pub request_destination: Option<String>,
    /// The address its TCP connection out was made to (`ip:port`).
    pub target: Option<String>,
    /// How the dial failed (the I/O error's kind); None when it connected
    /// or none was made.
    pub error: Option<String>,
    pub connect_ms: Option<u64>,
}

/// A DNS query answered or failed (sail's `DnsExchange`): a client's, or
/// the instance's own; a sequential server's members each once asked. The
/// name and records whole: the Engine redacts what it logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DnsExchange {
    /// As sail has it: without its final dot.
    pub name: String,
    /// `A`, `AAAA`, `HTTPS`, ...
    pub qtype: String,
    pub qtype_code: u16,
    /// The server's tag; None where a rule answered.
    pub server: Option<String>,
    /// `exchanged`, `cached`, `optimistic` or `rule`.
    pub source: String,
    /// A sequential server's attempt, from 1: each failed member, and the
    /// one that answered.
    pub attempt: Option<u32>,
    /// The answer's response code, as its number and as DNS names it
    /// (`NOERROR`, `NXDOMAIN`, ...); None when it failed.
    pub rcode: Option<u16>,
    pub rcode_name: Option<String>,
    /// Why there is no answer.
    pub error: Option<String>,
    /// The answer section's records as DNS writes their data, the first 16.
    pub answers: Vec<String>,
    pub answers_total: u32,
    /// The least TTL of the records, as the client is given it.
    pub ttl: Option<u32>,
    /// How long the server took; None from the cache or a rule.
    pub duration_ms: Option<u64>,
    /// Asked by the instance itself (to dial a domain).
    pub for_instance: bool,
}

/// What a reload did with each inbound: those of the configuration in its
/// order, then those it no longer has.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ReloadReport {
    /// How much was built again: `full` (outbounds, groups, DNS, routing
    /// and rule-sets anew), or `inbounds_only` when the configuration
    /// differed from the running one in its inbounds (and `user_limits`)
    /// alone, all else kept as it ran.
    pub path: String,
    /// By tag: `untouched`, `reloaded` (new users, certificate or key; the
    /// listener kept), `added`, `removed` or `replaced`.
    pub inbounds: Vec<(String, String)>,
    /// What the reload took and did not reach everywhere (an endpoint that
    /// runs keeps the dial defaults it was built with), as sail says it.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DialFailed {
    /// The outbounds it went through, outermost first, joined by `>` as the
    /// log's `out=`: the route's outbound, then the member each group on
    /// the way took, down to the member tried (`pick>direct`; `F>G>m` for a
    /// group G in a group F).
    pub chain: String,
    /// Where it went, redacted as the log says it.
    pub destination: String,
    /// `dial`, `handshake` or `transfer`.
    pub stage: String,
    /// The I/O error's kind (`ConnectionRefused`, `TimedOut`, …).
    pub error: String,
    pub count: u64,
    /// Whether the group goes on to try another member: false for the
    /// failure that ends the connection (each member's failure is told).
    pub more_to_try: bool,
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
    /// Outbound tags, outermost first (as `Routed::chain`; sail lists them
    /// the Clash way, members first, and the runtime turns them): the
    /// outbound the rules named, then each group's member, the last the one
    /// that carries it. The Engine maps them back to nodes with the
    /// translation's tag map.
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
/// from one decision (one monitor, #214).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkChange {
    /// From 1 for each start of the runtime; a reader that sees a gap
    /// missed changes in between (the channel keeps the latest only).
    pub generation: u64,
    /// `interface_changed` (another default interface), `moved` (the same
    /// interface on another network, a wake included), `offline` (no
    /// default interface) or `restored` (one again after none).
    pub change: String,
    /// `default_interface`, `state`, `host` (pushed), `wake`, or `lagged`
    /// (made up from the snapshot after changes were missed).
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
    /// In place: group selections and pins kept for groups of the same tag;
    /// the DNS cache emptied. The inbounds `config` has are those that run
    /// after it, compared by tag (one added at run time and not in `config`
    /// is removed); what became of each is the report, and only those
    /// removed or replaced lose their connections. A failed reload changes
    /// nothing; its own errors are `needs_restart` (a TUN added, removed or
    /// changed: only a start sets one up) and `inbound_lost` (a replaced
    /// inbound's new listener did not bind and the old one could not listen
    /// again: that inbound listens no more).
    async fn reload(&self, config: &str) -> Result<ReloadReport, RuntimeError>;
    /// Returns once listeners are closed and connections dropped.
    async fn stop(&self) -> Result<(), RuntimeError>;
    /// What the last end of a run (a stop, a failure, a start that failed
    /// partway) left: sail's tasks still running, and what it changed in the
    /// system and could not undo, each with its kind and, in its detail, the
    /// command that clears it by hand where there is one. Empty when it all
    /// ended; the same report until the next start.
    fn stop_leftovers(&self) -> Vec<crate::types::Leftover>;

    /// Subscribe first, then read.
    fn states(&self) -> watch::Receiver<RuntimeState>;
    fn state(&self) -> RuntimeState;

    async fn groups(&self) -> Result<Vec<GroupInfo>, RuntimeError>;
    /// Selects `member` (selector) or fixes it (fallback).
    async fn select(&self, group: &str, member: &str) -> Result<(), RuntimeError>;
    /// Back to automatic.
    async fn unfix(&self, group: &str) -> Result<(), RuntimeError>;
    /// Has the group test its members now, with its own URLs and timeouts,
    /// within `within`; the group chooses from the results as from its
    /// scheduled tests.
    async fn check_group(&self, group: &str, within: Duration) -> Result<(), RuntimeError>;
    /// Group switches as they happen. Taken once (by the Engine, at new).
    fn group_switches(&self) -> mpsc::Receiver<GroupSwitch>;
    /// Failed connections as they happen, through starts and stops; those
    /// that do not fit while the reader is behind are dropped. Taken once.
    fn dial_failures(&self) -> mpsc::Receiver<DialFailed>;
    /// Each connection routed, through starts and stops; those that do not
    /// fit while the reader is behind are dropped. Taken once, and only when
    /// wanted (the Engine's log at debug): sail builds them only for a
    /// subscriber, which taking this makes.
    fn routes(&self) -> mpsc::Receiver<Routed>;
    /// Each DNS query answered or failed, through starts and stops; as
    /// `routes`, taken once and only when wanted (sail builds them only for
    /// a subscriber), and dropped while the reader is behind.
    fn dns_exchanges(&self) -> mpsc::Receiver<DnsExchange>;

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

/// An inbound's tag in sing-box JSON: `tag`, else its `type`.
pub(crate) fn inbound_tag(inbound: &serde_json::Value) -> Option<String> {
    inbound["tag"]
        .as_str()
        .filter(|t| !t.is_empty())
        .or_else(|| inbound["type"].as_str())
        .map(str::to_owned)
}

/// The kind a change from `old` to `new` would have had.
pub(crate) fn made_up_kind(old: &NetworkSnapshot, new: &NetworkSnapshot) -> &'static str {
    match (old.offline, new.offline) {
        (_, true) => "offline",
        (true, false) => "restored",
        _ if (&old.interface, old.index) != (&new.interface, new.index) => "interface_changed",
        _ => "moved",
    }
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

#[cfg(all(test, target_os = "linux"))]
mod netns_helper;
#[cfg(all(test, target_os = "linux"))]
mod netns_tests;
#[cfg(all(test, windows, feature = "fault-injection"))]
mod windows_helper;
#[cfg(all(test, windows, feature = "fault-injection"))]
mod windows_tests;

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
            ("tun_name_taken", codes::TUN_NAME_TAKEN, false),
        ];
        for (sail, code, retryable) in cases {
            let error = RuntimeError::new(sail, "why").to_error();
            assert_eq!((error.code, error.retryable), (code, retryable), "{sail}");
            assert_eq!(error.message, format!("sail {sail}: why"));
        }
    }

    #[test]
    fn a_tun_name_taken_is_retryable_only_where_sail_chose_it() {
        let taken = RuntimeError::new(
            "tun_name_taken",
            "[tun] inbound: no free utun after 3 attempts, the last utun9",
        );
        for (platform, retryable) in [
            (Platform::Macos, true),
            (Platform::Linux, false),
            (Platform::Windows, false),
        ] {
            let error = taken.to_error_on(platform);
            assert_eq!(
                (error.code, error.retryable),
                (codes::TUN_NAME_TAKEN, retryable),
                "{platform:?}"
            );
            assert!(error.message.contains("utun9"), "the name is told");
        }
        let other = RuntimeError::new("io", "why").to_error_on(Platform::Macos);
        assert_eq!(other, RuntimeError::new("io", "why").to_error());
    }
}
