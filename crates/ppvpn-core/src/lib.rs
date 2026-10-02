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
pub mod localdns;
pub mod profile;
pub mod request;
pub mod status;
pub mod types;

pub use config::{
    EngineConfig, LocalProxyConfig, LogConfig, LogLevel, LogSink, Platform, Role, TunConfig,
};
pub use engine::{Engine, LOCAL_PROXY_CONTRACT_VERSION};
pub use error::{codes, Error};
pub use event::{Event, EventItem, EventKind, EventReceiver, LogReceiver};
pub use request::{
    validate_request, ApplyRequest, ApplyResult, ClearedPin, Pin, PinClearReason, RoutingMode,
    SwitchKind,
};
pub use status::{
    DegradedReason, EngineState, FatalReason, IngressHealth, IngressStatus, LocalProxyStatus,
    NodeStatus, RuleSetStatus, Status, SystemProxyStatus, TunRouting,
};
pub use types::{
    AvailabilityResult, Connection, EntranceResult, IngressInfo, IngressProbeResult,
    LocalProxyCredential, LocalProxyKind, LocalProxyMetadata, NodeInfo, ProbeAvailabilityRequest,
    ProbeEntrancesRequest, ProbeMethod, ShutdownReport, Traffic, VersionInfo,
};

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
