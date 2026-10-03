//! The Rust ppvpn-core (#45): a library hosts link and run in process. Its
//! public API and semantics are the contract in docs/host-integration.md;
//! every public change goes with that document in the same PR.
//!
//! Entry point: [`Engine`]. Values are serialisable (serde) for the future
//! FFI; enums and structs are `non_exhaustive` and only grow.

pub mod config;
mod engine;
pub mod error;
pub mod event;
mod hostipv6;
// dns-local is wired up by the runtime; until then only its tests use it.
#[allow(dead_code, unused_imports)]
pub(crate) mod localdns;
mod localproxy;
mod logfmt;
#[allow(dead_code, unused_imports)] // until the Engine calls it (engine.rs)
mod probe;
pub(crate) mod profile;
pub mod request;
// Rule sets are wired up by the engine; until then only their tests use them.
#[allow(dead_code, unused_imports)]
mod rulesets;
mod runtime;
mod state_dir;
pub mod status;
mod translate;
// The Engine starts it with the TUN; until then only its tests use it.
#[allow(dead_code, unused_imports)]
mod tunrules;
pub mod types;

pub use config::{
    EngineConfig, LocalProxyConfig, LogConfig, LogLevel, LogSink, Platform, Role, TunConfig,
};
pub use engine::{tracing_layer, Engine, LOCAL_PROXY_CONTRACT_VERSION};
pub use error::{codes, Error};
pub use event::{Event, EventItem, EventKind, EventReceiver, LogReceiver};
pub use request::{
    ApplyRequest, ApplyResult, ClearedPin, Pin, PinClearReason, RoutingMode, SwitchKind,
};
pub use status::{
    CredentialsResetReason, DegradedReason, EngineState, FatalReason, IngressHealth, IngressStatus,
    LocalProxyStatus, NodeStatus, RuleSetStatus, Status, SystemProxyStatus, TunRouting,
};
pub use types::{
    AvailabilityResult, Connection, EntranceResult, IngressInfo, IngressProbeResult,
    LocalProxyCredential, LocalProxyKind, LocalProxyMetadata, NodeInfo, ProbeAvailabilityRequest,
    ProbeEntrancesRequest, ProbeMethod, ShutdownReport, Traffic, VersionInfo,
};

/// Unstable: for this crate's own tests and `ppvpn-core-lab` only. Hosts
/// must not use it; it changes in any release without notice. The public
/// API is [`Engine`] and the value types above (docs/host-integration.md).
#[doc(hidden)]
pub mod internal {
    use chrono::{DateTime, Utc};

    pub use crate::profile::Profile;
    use crate::{ApplyRequest, Error};

    /// What `apply` checks before it changes anything (the contract golden).
    pub fn validate_request(request: &ApplyRequest, now: DateTime<Utc>) -> Result<Profile, Error> {
        crate::request::validate_request(request, now)
    }

    /// An Engine on an in-memory runtime that runs nothing: the contract
    /// golden drives lifecycle, selection and pins on it without sail. Only
    /// with the `testing` feature (tests), never in a host's build.
    #[cfg(feature = "testing")]
    pub fn engine_on_fake_runtime(config: crate::EngineConfig) -> crate::Engine {
        crate::Engine::with_runtime(
            config,
            std::sync::Arc::new(crate::runtime::fake::FakeRuntime::default()),
        )
    }
}

/// Whether a sail runtime with this id runs in this process. Here so that
/// the shell links sail (and CI builds and caches it, BoringSSL included).
pub fn sail_runtime_running(id: sail::RuntimeId) -> bool {
    sail::is_running(id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_sail_runtime_runs_by_itself() {
        assert!(!super::sail_runtime_running(0));
    }
}
