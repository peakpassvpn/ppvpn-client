//! Starts the push agent (apps/flutter/push-agent), which shows backend
//! pushes as notifications while the app is closed. macOS keeps its Swift
//! agent: nothing here.
//!
//! Windows, as the native app's PushAgentAutostart.cs: signed in and the
//! client has registered the device (it wrote `push-agent.json`), the
//! per-user Run value `PPVPNPushAgent` points at the agent next to the app's
//! executable, and the agent is started when it is not running. Signed out
//! (or signing in again), the Run value is removed and the agent is asked
//! to quit. Separate from launch at sign-in: the agent runs whether or not
//! the app starts with Windows. The agent removes the value itself when its
//! push token is revoked; the installer removes it on uninstall.
//!
//! Linux, as the native app's PPVPNApplication.cs: the agent is started
//! once when the app starts (it exits at once when it already runs);
//! autostart covers later sessions (the package's).
//!
//! A development build has no agent next to it: nothing is started then.

use std::sync::Arc;

use ppvpn_client::Client;

/// Once, when the app's client is created.
pub(crate) fn start(client: &Arc<Client>, data_dir: &str) {
    #[cfg(windows)]
    win::follow(client.clone(), data_dir);
    #[cfg(not(windows))]
    let _ = (client, data_dir);
    #[cfg(target_os = "linux")]
    start_linux();
}

#[cfg(target_os = "linux")]
fn start_linux() {
    use std::process::{Command, Stdio};

    const AGENT: &str = "/usr/lib/ppvpn/ppvpn-push-agent";
    if !std::path::Path::new(AGENT).is_file() {
        return;
    }
    match Command::new(AGENT).stdin(Stdio::null()).spawn() {
        // Reaped on a thread of its own: it runs as long as the session.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(error) => tracing::warn!("push agent not started: {error}"),
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use ppvpn_client::{AuthState, Client};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };
    use windows_sys::Win32::System::Threading::{
        OpenEventW, OpenMutexW, SetEvent, EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
    };

    const EXE_NAME: &str = "ppvpn-push-agent.exe";
    /// Also in installer/ppvpn.nsi.
    const RUN_VALUE: &str = "PPVPNPushAgent";
    /// apps/flutter/push-agent/src/win.rs; also in installer/ppvpn.nsi.
    const INSTANCE_MUTEX: &str = r"Local\PPVPN.PushAgent";
    const QUIT_EVENT: &str = r"Local\PPVPN.PushAgent.Quit";
    /// Written by the client after registering the device, deleted on
    /// sign-out.
    const AGENT_FILE: &str = "push-agent.json";
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// How often the sign-in state and the agent file are looked at (the
    /// native app watched the file; a rewrite is seen by its time).
    const POLL: Duration = Duration::from_secs(2);

    /// Follows the sign-in state and the agent file from now on, on a
    /// thread of its own.
    pub(crate) fn follow(client: Arc<Client>, data_dir: &str) {
        let file = Path::new(data_dir).join(AGENT_FILE);
        let Some(agent) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(EXE_NAME)))
        else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("ppvpn-push-agent".into())
            .spawn(move || {
                let mut seen = None;
                let mut missing_logged = false;
                loop {
                    let stage = Stage::of(&client.snapshot().auth);
                    let written = std::fs::metadata(&file)
                        .and_then(|meta| meta.modified())
                        .ok();
                    let now = (stage, written);
                    if seen != Some(now) {
                        seen = Some(now);
                        evaluate(stage, written.is_some(), &agent, &mut missing_logged);
                    }
                    std::thread::sleep(POLL);
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("push agent: cannot follow the sign-in: {error}");
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Stage {
        Restoring,
        SignedOut,
        SignedIn,
    }

    impl Stage {
        fn of(auth: &AuthState) -> Self {
            match auth {
                AuthState::Restoring => Self::Restoring,
                AuthState::SignedOut | AuthState::AwaitingBrowser { .. } => Self::SignedOut,
                AuthState::SignedIn => Self::SignedIn,
            }
        }
    }

    fn evaluate(stage: Stage, file: bool, agent: &Path, missing_logged: &mut bool) {
        match stage {
            Stage::SignedIn if file => enable(agent, missing_logged),
            Stage::SignedOut => disable(),
            Stage::SignedIn | Stage::Restoring => {}
        }
    }

    fn enable(agent: &Path, missing_logged: &mut bool) {
        if !agent.is_file() {
            // A dev build: the agent is not next to the app.
            if !*missing_logged {
                tracing::info!("push agent: {} not found; not starting it", agent.display());
            }
            *missing_logged = true;
            return;
        }
        let command = format!("\"{}\"", agent.display());
        if run_value().as_deref() != Some(command.as_str()) {
            match set_run_value(&command) {
                Ok(()) => tracing::info!("push agent: autostart on"),
                Err(error) => tracing::warn!("push agent: writing the Run value failed: {error}"),
            }
        }
        if running() {
            return;
        }
        let mut process = std::process::Command::new(agent);
        if let Some(dir) = agent.parent() {
            process.current_dir(dir);
        }
        match process.spawn() {
            Ok(mut child) => {
                tracing::info!("push agent: started (pid {})", child.id());
                std::thread::spawn(move || child.wait());
            }
            Err(error) => tracing::warn!("push agent: start failed: {error}"),
        }
    }

    fn disable() {
        if run_value().is_some() {
            match delete_run_value() {
                Ok(()) => tracing::info!("push agent: autostart off"),
                Err(error) => tracing::warn!("push agent: removing the Run value failed: {error}"),
            }
        }
        let name = wide(QUIT_EVENT);
        // SAFETY: a NUL-terminated name; the handle is closed here.
        unsafe {
            let quit = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if !quit.is_null() {
                SetEvent(quit);
                CloseHandle(quit);
                tracing::info!("push agent: asked to quit");
            }
        }
    }

    /// The agent holds its instance mutex while it runs.
    fn running() -> bool {
        let name = wide(INSTANCE_MUTEX);
        // SAFETY: a NUL-terminated name; the handle is closed here.
        unsafe {
            let mutex = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
            if mutex.is_null() {
                return false;
            }
            CloseHandle(mutex);
            true
        }
    }

    fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
        text.as_ref().encode_wide().chain(Some(0)).collect()
    }

    fn run_value() -> Option<String> {
        let (key, name) = (wide(RUN_KEY), wide(RUN_VALUE));
        let mut buffer = vec![0u16; 1024];
        let mut size = u32::try_from(buffer.len() * 2).ok()?;
        // SAFETY: NUL-terminated strings; `buffer` holds `size` bytes. A
        // longer value fails (ERROR_MORE_DATA) and reads as different.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            // Not there (or unreadable): ERROR_FILE_NOT_FOUND and the rest.
            return None;
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    fn set_run_value(command: &str) -> std::io::Result<()> {
        let (key, name, value) = (wide(RUN_KEY), wide(RUN_VALUE), wide(command));
        let size = u32::try_from(value.len() * 2).map_err(std::io::Error::other)?;
        // SAFETY: NUL-terminated strings; `value` holds `size` bytes.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                size,
            )
        };
        result(status)
    }

    fn delete_run_value() -> std::io::Result<()> {
        let (key, name) = (wide(RUN_KEY), wide(RUN_VALUE));
        // SAFETY: NUL-terminated strings.
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        result(status)
    }

    fn result(status: u32) -> std::io::Result<()> {
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(status as i32))
        }
    }
}
