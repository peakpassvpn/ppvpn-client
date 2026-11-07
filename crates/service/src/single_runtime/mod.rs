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
