//! The Flutter desktop prototype's bridge (#387): flutter_rust_bridge
//! generates the Dart side from [`api`]. ci.yml runs the codegen, which adds
//! `mod frb_generated;` here; nothing generated is committed.

pub mod api;
mod autostart;
mod bridge;
mod dirs;
mod hooks;
mod messages;
mod push_agent;
mod settings;
