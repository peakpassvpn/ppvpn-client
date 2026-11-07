//! Capture resource ownership is independent of forwarding runtime lifetime.
pub mod auth;
pub mod capture;
#[cfg(unix)]
pub mod fdpass;

pub mod gateway;

#[cfg(unix)]
pub mod endpoint;
#[cfg(target_os = "macos")]
pub mod mac_peer;

#[cfg(unix)]
pub mod server;

#[cfg(target_os = "macos")]
pub mod agent;

#[cfg(target_os = "macos")]
pub mod broker;
#[cfg(target_os = "macos")]
pub mod host;
#[cfg(target_os = "macos")]
pub mod native;

#[cfg(target_os = "macos")]
pub mod login;

#[cfg(all(target_os = "macos", feature = "macos-capture-acceptance"))]
pub mod acceptance_guard;
