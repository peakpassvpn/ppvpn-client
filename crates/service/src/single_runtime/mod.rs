//! Capture resource ownership is independent of forwarding runtime lifetime.
pub mod auth;
pub mod capture;
#[cfg(unix)]
pub mod fdpass;
