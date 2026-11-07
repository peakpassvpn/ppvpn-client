//! Explicit user-runtime and broker boundary for the macOS migration.
//! New process roles are opt-in; installed/default launch configuration is unchanged.
pub mod peer_identity;
pub mod protocol;
pub mod single_runtime;

#[cfg(target_os = "macos")]
pub mod mac_identity;
