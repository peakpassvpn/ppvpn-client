//! dns-local: the physical network's own resolvers, read from the default
//! interface and followed across network changes (#61; Go:
//! internal/localdns). Queries go to them from sockets bound to that
//! interface (the runtime's [`Dial`]), never through the TUN and never to
//! 127.0.0.1. With none to ask, a query gets SERVFAIL at once.
//!
//! #45 dns-local cases: A (reading and filtering) in `servers`, `scutil` and
//! `adapters`, B (the cache) in `cache`, C (queries) here. D and E need the
//! runtime and run in the lab and netns CI.

pub mod adapters;
pub mod cache;
pub mod exchange;
pub mod hosts;
pub(crate) mod listener;
pub mod scutil;
pub mod servers;
pub(crate) mod source;

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;

use hickory_proto::op::{Message, MessageType, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{RData, Record, RecordType};

pub use cache::{Cache, Change};
pub(crate) use exchange::{Dial, PlainDial, RuntimeDial};
pub use hosts::Hosts;
pub use servers::{Interface, Prefix, Server};

/// The TTL of answers from the hosts file.
pub const HOSTS_TTL: u32 = 10;

/// What one read of the system's resolvers found. `source` names where they
/// came from (adapter, scutil-global, scutil-scoped, override) for the log.
#[derive(Clone, Debug, PartialEq)]
pub struct Discovered {
    pub servers: Vec<Server>,
    pub source: String,
    pub error: Option<String>,
}

impl Discovered {
    pub fn read(source: &str, servers: Vec<Server>) -> Self {
        Discovered {
            servers,
            source: source.into(),
            error: None,
        }
    }
    pub fn failed(source: &str, error: String) -> Self {
        Discovered {
            servers: Vec::new(),
            source: source.into(),
            error: Some(error),
        }
    }
}

/// Why dns-local has no resolver to ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalDnsError {
    /// No default interface (offline).
    NoInterface,
    /// The interface has no usable resolver (DHCP has not handed one out,
    /// or only loopback and tunnel addresses).
    NoServers { interface: String, source: String },
    /// Reading the interface's resolvers failed.
    Read {
        interface: String,
        source: String,
        cause: String,
    },
}

impl fmt::Display for LocalDnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocalDnsError::NoInterface => write!(f, "no default interface"),
            LocalDnsError::NoServers { interface, source } if source.is_empty() => {
                write!(f, "no DNS servers on {interface}")
            }
            LocalDnsError::NoServers { interface, source } => {
                write!(f, "no DNS servers on {interface} ({source})")
            }
            LocalDnsError::Read {
                interface,
                source,
                cause,
            } => {
                write!(f, "read the DNS servers of {interface} ({source}): {cause}")
            }
        }
    }
}

impl std::error::Error for LocalDnsError {}

/// The dns-local transport: answers a query from the hosts file, else from
/// the default interface's resolvers, else with SERVFAIL.
pub(crate) struct LocalDns {
    cache: Arc<Cache>,
    hosts: Arc<Hosts>,
    dial: Arc<dyn Dial>,
}

impl LocalDns {
    pub(crate) fn new(cache: Arc<Cache>, hosts: Arc<Hosts>, dial: Arc<dyn Dial>) -> Self {
        LocalDns { cache, hosts, dial }
    }

    /// The cache, for the runtime's interface monitor
    /// ([`Cache::invalidate`] on every change of the default interface).
    pub fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }

    /// The answer to `query`. Never an error: a query with no server to ask
    /// gets SERVFAIL (C1), and so does one that every server failed (B4
    /// then has the next query read the servers again). Logged at debug
    /// level as Go's `dns` line (server `dns-local`, the resolver that
    /// answered as `upstream`).
    pub async fn exchange(&self, query: &Message) -> Message {
        let started = std::time::Instant::now();
        let (answer, outcome) = self.answer(query).await;
        log_exchange(query, &answer, &outcome, started.elapsed());
        answer
    }

    async fn answer(&self, query: &Message) -> (Message, Outcome) {
        if let Some(answer) = self.hosts_answer(query) {
            return (answer, Outcome::Hosts);
        }
        // Reading may run scutil: off the async threads.
        let cache = self.cache.clone();
        let (iface, servers) = match tokio::task::spawn_blocking(move || cache.resolve()).await {
            Ok(Ok(found)) => found,
            Ok(Err(e)) => {
                return (
                    exchange::server_failure(query),
                    Outcome::Failed(e.to_string()),
                );
            }
            Err(e) => {
                return (
                    exchange::server_failure(query),
                    Outcome::Failed(e.to_string()),
                );
            }
        };
        let addrs: Vec<_> = servers.iter().map(|s| s.socket_addr(&iface)).collect();
        match exchange::exchange(&self.dial, &addrs, query).await {
            Ok((answer, upstream)) => (answer, Outcome::Answered(upstream.to_string())),
            Err(errors) => {
                self.cache.failed();
                (
                    exchange::server_failure(query),
                    Outcome::Failed(format!("every server failed: {}", errors.join("; "))),
                )
            }
        }
    }

    /// An answer from the hosts file for a single A or AAAA question whose
    /// name it lists (C6): its addresses of that family, none being an
    /// empty answer.
    fn hosts_answer(&self, query: &Message) -> Option<Message> {
        let [question] = query.queries.as_slice() else {
            return None;
        };
        let want = question.query_type;
        if want != RecordType::A && want != RecordType::AAAA {
            return None;
        }
        let ips = self.hosts.lookup(&question.name.to_ascii());
        if ips.is_empty() {
            return None;
        }
        let mut answer = Message::new(query.id, MessageType::Response, query.op_code);
        answer.metadata.response_code = ResponseCode::NoError;
        answer.metadata.recursion_desired = query.recursion_desired;
        answer.metadata.recursion_available = true;
        answer.add_query(question.clone());
        for ip in ips {
            let rdata = match (ip, want) {
                (IpAddr::V4(v4), RecordType::A) => RData::A(A(*v4)),
                (IpAddr::V6(v6), RecordType::AAAA) => RData::AAAA(AAAA(*v6)),
                _ => continue,
            };
            answer.add_answer(Record::from_rdata(question.name.clone(), HOSTS_TTL, rdata));
        }
        Some(answer)
    }
}

/// Where an answer came from, for the `dns` line.
enum Outcome {
    Hosts,
    /// The resolver that answered.
    Answered(String),
    /// Why it is SERVFAIL.
    Failed(String),
}

/// Go's dnstransport `dns` line, at debug level: name, type, server, then
/// `upstream` and `rcode`/`answers`, or `error`; and `ms`. lab and the
/// performance checks parse it.
fn log_exchange(
    query: &Message,
    answer: &Message,
    outcome: &Outcome,
    elapsed: std::time::Duration,
) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    let (name, qtype) = query
        .queries
        .first()
        .map(|q| (q.name.to_ascii(), q.query_type.to_string()))
        .unwrap_or_default();
    let server = crate::translate::DNS_LOCAL_TAG;
    let ms = elapsed.as_millis() as u64;
    match outcome {
        Outcome::Failed(error) => {
            tracing::debug!(name = %name, "type" = %qtype, server, error = %error, ms, "dns");
        }
        // As Go: no upstream when no resolver was asked.
        Outcome::Hosts => {
            tracing::debug!(
                name = %name,
                "type" = %qtype,
                server,
                rcode = %rcode(answer.metadata.response_code),
                answers = answer.answers.len(),
                ms,
                "dns"
            );
        }
        Outcome::Answered(upstream) => {
            tracing::debug!(
                name = %name,
                "type" = %qtype,
                server,
                upstream = %upstream,
                rcode = %rcode(answer.metadata.response_code),
                answers = answer.answers.len(),
                ms,
                "dns"
            );
        }
    }
}

/// A response code in Go's (miekg/dns) words.
fn rcode(code: ResponseCode) -> String {
    match code {
        ResponseCode::NoError => "NOERROR".into(),
        ResponseCode::FormErr => "FORMERR".into(),
        ResponseCode::ServFail => "SERVFAIL".into(),
        ResponseCode::NXDomain => "NXDOMAIN".into(),
        ResponseCode::NotImp => "NOTIMP".into(),
        ResponseCode::Refused => "REFUSED".into(),
        other => u16::from(other).to_string(),
    }
}

#[cfg(test)]
mod tests;
