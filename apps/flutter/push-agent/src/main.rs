//! ppvpn-push-agent: shows backend pushes as OS notifications while the
//! Flutter desktop app is closed (#401), on crates/desktop's
//! `PushAgent` (pull, ack, pending, cursor). It replaces the
//! .NET agents (apps/windows/PPVPN.PushAgent, apps/linux/PPVPN.PushAgent)
//! and behaves as they do: `win` and `linux` say how on each system.
//!
//! It reads the data directory the app's client writes `push-agent.json`
//! to, so it takes the app's own directories (apps/flutter/rust/src/dirs.rs,
//! compiled in here). Its log is the client's `ppvpn-push-agent.*` in the
//! app's log directory. macOS keeps its Swift agent: there this is a stub.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

#[cfg(any(windows, target_os = "linux"))]
#[path = "../../rust/src/dirs.rs"]
mod dirs;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod win;

/// This build's version: a release's `PPVPN_VERSION`, else the workspace's
/// (as the app's, apps/flutter/rust/src/update.rs).
#[cfg(any(windows, target_os = "linux"))]
fn version() -> &'static str {
    option_env!("PPVPN_VERSION")
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[cfg(windows)]
fn main() -> ExitCode {
    win::main()
}

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    linux::main()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn main() -> ExitCode {
    eprintln!("ppvpn-push-agent: not used on this system");
    ExitCode::FAILURE
}
