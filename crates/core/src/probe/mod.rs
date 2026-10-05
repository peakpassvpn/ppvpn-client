//! Probes (docs/host-integration.md, section 4.5; Go's `probe` package and
//! the probe calls of its `internal/runtime`).
//!
//! - [`probe_entrances`] measures every ingress directly, by a TCP handshake
//!   or one ICMP echo, never through the tunnel's rules.
//! - [`probe_availability`] fetches a URL through a node's outbound
//!   ([`crate::runtime::Runtime::dial_tcp`]).
//!
//! Both return the events the Engine emits with their results. Neither
//! watches the network: the Engine passes what sail's monitor says of the
//! default interface, and with none both fail at once with
//! NO_DEFAULT_INTERFACE (retryable) and probe nothing (#69).

mod availability;
mod entrance;
mod icmp;

use std::time::Duration;

use tokio::sync::watch;

use crate::error::{codes, Error};

pub(crate) use availability::{probe_availability, AVAILABILITY_TIMEOUT};
pub(crate) use entrance::{
    probe_entrances, Net, SystemNet, ENTRANCE_CONCURRENCY, ENTRANCE_TIMEOUT,
};

/// The error codes of a probe result (Core API v1's), as opposed to the
/// error of the call.
pub(crate) mod result_codes {
    pub const CANCELED: &str = "CANCELED";
    pub const TIMEOUT: &str = "TIMEOUT";
    pub const CONNECT_FAILED: &str = "CONNECT_FAILED";
    pub const DNS_FAILED: &str = "DNS_FAILED";
    pub const ICMP_TIMEOUT: &str = "ICMP_TIMEOUT";
    pub const ICMP_UNREACHABLE: &str = "ICMP_UNREACHABLE";
    pub const ICMP_UNSUPPORTED: &str = "ICMP_UNSUPPORTED";
    pub const ICMP_FAILED: &str = "ICMP_FAILED";
    pub const TARGET_INVALID: &str = "TARGET_INVALID";
    pub const HTTP_STATUS: &str = "HTTP_STATUS";
    pub const PROXY_REQUEST_FAILED: &str = "PROXY_REQUEST_FAILED";
}

/// What the Engine knows of the default interface, from sail's monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefaultInterface {
    /// The engine cannot tell (not running, no monitor): probe.
    Unknown,
    Present,
    /// Offline: fail at once, probe nothing.
    Absent,
}

fn require_online(network: DefaultInterface) -> Result<(), Error> {
    match network {
        DefaultInterface::Absent => Err(Error::new(
            codes::NO_DEFAULT_INTERFACE,
            true,
            "the host has no default network interface; probe again once the network is back",
        )),
        DefaultInterface::Unknown | DefaultInterface::Present => Ok(()),
    }
}

/// A request's timeout: `fallback` for 0, at most two minutes (Core API v1).
fn timeout_of(ms: u64, fallback: Duration) -> Duration {
    if ms == 0 {
        fallback
    } else {
        Duration::from_millis(ms.min(120_000))
    }
}

/// Ends probes in flight: what has not finished reports CANCELED.
pub(crate) struct Canceller(watch::Sender<bool>);

impl Canceller {
    pub(crate) fn cancel(&self) {
        self.0.send_replace(true);
    }
}

/// The probes' side of a [`Canceller`].
#[derive(Clone)]
pub(crate) struct Cancel(watch::Receiver<bool>);

impl Cancel {
    pub(crate) fn new() -> (Canceller, Cancel) {
        let (tx, rx) = watch::channel(false);
        (Canceller(tx), Cancel(rx))
    }

    /// Never cancelled.
    pub(crate) fn never() -> Cancel {
        Cancel::new().1
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once cancelled; never when the canceller is gone first.
    pub(crate) async fn cancelled(&self) {
        let mut rx = self.0.clone();
        if rx.wait_for(|cancelled| *cancelled).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go: api TestNoDefaultInterfaceIsRetryable.
    #[test]
    fn no_default_interface_is_retryable() {
        let error = require_online(DefaultInterface::Absent).unwrap_err();
        assert_eq!(
            (error.code, error.retryable),
            (codes::NO_DEFAULT_INTERFACE, true)
        );
        assert!(require_online(DefaultInterface::Present).is_ok());
        assert!(require_online(DefaultInterface::Unknown).is_ok());
    }

    #[test]
    fn timeouts_default_and_are_capped() {
        let fallback = Duration::from_secs(5);
        assert_eq!(timeout_of(0, fallback), fallback);
        assert_eq!(timeout_of(50, fallback), Duration::from_millis(50));
        assert_eq!(timeout_of(600_000, fallback), Duration::from_secs(120));
    }

    #[tokio::test]
    async fn cancel_wakes_and_never_does_not() {
        let (canceller, cancel) = Cancel::new();
        assert!(!cancel.is_cancelled());
        canceller.cancel();
        assert!(cancel.is_cancelled());
        cancel.cancelled().await;
        let never = Cancel::never();
        let waited = tokio::time::timeout(Duration::from_millis(20), never.cancelled()).await;
        assert!(waited.is_err());
    }
}
