//! Windows, as apps/windows/PPVPN.PushAgent (Program.cs, PushNotifier.cs,
//! Toasts.cs):
//! - One per session: [`INSTANCE_MUTEX`]; a second start exits at once.
//! - The app (on sign-out) and the installer set [`QUIT_EVENT`]: stop, exit.
//! - Revoked (the push token was rejected): remove our Run value, exit. The
//!   app writes it again and restarts us after it registered the device
//!   again (apps/flutter/rust/src/push_agent.rs).
//! - Idle (no push-agent.json): keep running; the client re-checks the file.
//! - Toasts are not shown while they are turned off for PPVPN (the client
//!   keeps those pushes pending); the setting is polled every minute and a
//!   change to on retries them at once.
//!
//! Toasts are the app's identity: the AUMID, its registration (the click
//! activator is ppvpn.exe) and the `launch` arguments (`id=<push id>`) are
//! those of the native app's AppNotifications.cs.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ppvpn_client::{
    MessageSeverity, PushAgent, PushAgentConfig, PushAgentListener, PushAgentState, PushMessage,
};
use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

/// Also in apps/flutter/rust/src/push_agent.rs and installer/ppvpn.nsi.
const INSTANCE_MUTEX: &str = r"Local\PPVPN.PushAgent";
const QUIT_EVENT: &str = r"Local\PPVPN.PushAgent.Quit";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "PPVPNPushAgent";
const SETTING_POLL: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

const AUMID: &str = "PeakPass.PPVPN";
const DISPLAY_NAME: &str = "PPVPN";
const ACTIVATOR_CLSID: &str = "FCD3C3FA-FCA6-4F2D-BC9E-BEA74D70EBBE";
const TOAST_ACTIVATED_SWITCH: &str = "--toast-activated";
const APP_EXE: &str = "ppvpn.exe";
const TOAST_GROUP: &str = "inbox";
/// Windows 11 22H2: toast scenario "urgent" (important notifications).
const URGENT_SCENARIO_BUILD: u32 = 22546;

pub(crate) fn main() -> ExitCode {
    let Ok((data_dir, log_dir)) = crate::dirs::dirs() else {
        return ExitCode::FAILURE;
    };
    let Some(revoked) = Handle::event(None, true) else {
        return ExitCode::FAILURE;
    };
    let notifier = Arc::new(Notifier {
        enabled: Mutex::new(None),
        revoked,
    });
    // Installs the log (ppvpn-push-agent.* in the log directory).
    let agent = PushAgent::new(
        PushAgentConfig {
            data_dir,
            log_dir,
            platform: "windows".into(),
            app_version: crate::version().into(),
        },
        notifier.clone(),
    );

    let name = wide(INSTANCE_MUTEX);
    // SAFETY: a NUL-terminated name; the handle is closed by `Handle`.
    let instance = Handle(unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) });
    // SAFETY: no other call in between.
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if instance.0.is_null() {
        tracing::error!("push agent: cannot create {INSTANCE_MUTEX}");
        return ExitCode::FAILURE;
    }
    if exists {
        tracing::info!(
            "push agent: another one runs in this session; pid {} exits",
            std::process::id()
        );
        return ExitCode::SUCCESS;
    }
    let Some(quit) = Handle::event(Some(QUIT_EVENT), false) else {
        tracing::error!("push agent: cannot create {QUIT_EVENT}");
        return ExitCode::FAILURE;
    };

    tracing::info!(
        "push agent {} starting, pid {}, {}",
        crate::version(),
        std::process::id(),
        exe_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default()
    );
    // The process joins the MTA, so the client's callback threads use the
    // implicit MTA for the WinRT calls.
    // SAFETY: plain calls; the MTA is kept for the process lifetime.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let aumid = wide(AUMID);
        SetCurrentProcessExplicitAppUserModelID(aumid.as_ptr());
    }
    register_aumid();
    notifier.poll_setting();

    let Some(ended) = Handle::event(None, true).map(Arc::new) else {
        return ExitCode::FAILURE;
    };
    let runner = {
        let agent = agent.clone();
        let ended = SetOnDrop(ended.clone());
        std::thread::Builder::new()
            .name("push-agent".into())
            .spawn(move || {
                let _ended = ended;
                agent.run();
            })
    };
    let Ok(runner) = runner else {
        tracing::error!("push agent: cannot start its thread");
        return ExitCode::FAILURE;
    };

    let mut code = ExitCode::SUCCESS;
    let handles = [quit.0, notifier.revoked.0, ended.0];
    loop {
        // SAFETY: three valid handles, owned above and alive until the end.
        let signaled =
            unsafe { WaitForMultipleObjects(3, handles.as_ptr(), 0, millis(SETTING_POLL)) };
        if signaled == WAIT_TIMEOUT {
            if notifier.poll_setting() {
                tracing::info!("push agent: notifications turned on; retrying pending pushes");
                agent.retry_pending();
            }
            continue;
        }
        if signaled == WAIT_OBJECT_0 {
            tracing::info!("push agent: quit requested");
        } else if signaled == WAIT_OBJECT_0 + 1 {
            tracing::info!("push agent: push token revoked; removing autostart and exiting");
            if let Err(error) = reg_delete(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE) {
                tracing::warn!("push agent: removing the Run value failed: {error}");
            }
        } else {
            tracing::error!("push agent: stopped unexpectedly");
            code = ExitCode::FAILURE;
        }
        break;
    }

    agent.stop();
    // SAFETY: a valid handle, alive until the end.
    if unsafe { WaitForSingleObject(ended.0, millis(STOP_TIMEOUT)) } == WAIT_OBJECT_0 {
        let _ = runner.join();
    } else {
        tracing::warn!(
            "push agent: did not stop within {} s",
            STOP_TIMEOUT.as_secs()
        );
    }
    tracing::info!("push agent: exit");
    drop(instance);
    code
}

fn millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(Some(0)).collect()
}

fn exe_dir() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(std::path::Path::to_path_buf)
}

/// A kernel object handle, closed on drop.
struct Handle(HANDLE);

// SAFETY: a kernel object handle may be used from any thread.
unsafe impl Send for Handle {}
// SAFETY: as above; SetEvent and the waits are thread-safe.
unsafe impl Sync for Handle {}

impl Handle {
    /// A new event (manual or auto reset), named or not; opens the named
    /// one when it exists.
    fn event(name: Option<&str>, manual_reset: bool) -> Option<Self> {
        let name = name.map(wide);
        let name_ptr = name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr());
        // SAFETY: a NUL-terminated name or null.
        let handle = unsafe { CreateEventW(std::ptr::null(), manual_reset.into(), 0, name_ptr) };
        (!handle.is_null()).then_some(Self(handle))
    }

    fn set(&self) {
        // SAFETY: a valid event handle.
        unsafe { SetEvent(self.0) };
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: owned, closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Signals the event when the runner thread ends, however it ends.
struct SetOnDrop(Arc<Handle>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.set();
    }
}

/// The client's listener: shows each push as a toast of the app's identity.
struct Notifier {
    /// The toast setting as last seen (`None`: not read yet).
    enabled: Mutex<Option<bool>>,
    /// Set on [`PushAgentState::Revoked`].
    revoked: Handle,
}

impl PushAgentListener for Notifier {
    fn on_push(&self, message: PushMessage) -> bool {
        let setting = match toast_setting() {
            Ok(setting) => setting,
            Err(error) => {
                tracing::warn!("push {}: not shown: {error}", message.id);
                return false;
            }
        };
        self.remember(setting);
        if setting != NotificationSetting::Enabled {
            tracing::info!(
                "push {}: notifications are off ({}); kept pending",
                message.id,
                setting.0
            );
            return false;
        }
        match show_toast(&toast_xml(&message), &message.id.to_string()) {
            Ok(()) => {
                tracing::info!(
                    "push {}: shown ({:?}, {:?})",
                    message.id,
                    message.severity,
                    message.category
                );
                true
            }
            Err(error) => {
                tracing::warn!("push {}: not shown: {error}", message.id);
                false
            }
        }
    }

    fn on_state(&self, state: PushAgentState) {
        // The client logs the state itself.
        if state == PushAgentState::Revoked {
            self.revoked.set();
        }
    }
}

impl Notifier {
    /// Polled by the main thread. True when notifications just became
    /// enabled again (the caller then retries what the client holds back).
    fn poll_setting(&self) -> bool {
        match toast_setting() {
            Ok(setting) => self.remember(setting),
            Err(error) => {
                tracing::warn!("push agent: notification setting unavailable: {error}");
                false
            }
        }
    }

    /// True on a change to enabled.
    fn remember(&self, setting: NotificationSetting) -> bool {
        let enabled = setting == NotificationSetting::Enabled;
        let previous = self.enabled.lock().unwrap().replace(enabled);
        if previous != Some(enabled) {
            tracing::info!("push agent: notification setting {}", setting.0);
        }
        enabled && previous == Some(false)
    }
}

/// The per-app setting in Settings › System › Notifications (or policy).
fn toast_setting() -> windows::core::Result<NotificationSetting> {
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Setting()
}

fn show_toast(xml: &str, tag: &str) -> windows::core::Result<()> {
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&document)?;
    toast.SetTag(&HSTRING::from(tag))?;
    toast.SetGroup(&HSTRING::from(TOAST_GROUP))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
}

fn toast_xml(message: &PushMessage) -> String {
    let (title, body) = if message.title.trim().is_empty() {
        let title = if message.body.trim().is_empty() {
            DISPLAY_NAME
        } else {
            message.body.as_str()
        };
        (title, "")
    } else {
        (message.title.as_str(), message.body.as_str())
    };
    // Only the push id: the app resolves it through the client's record of
    // shown pushes and never takes a link from the launch arguments.
    let launch = format!("id={}", message.id);
    let urgent =
        message.severity == MessageSeverity::Critical && os_build() >= URGENT_SCENARIO_BUILD;
    // Pushes retried later keep the time they were sent.
    let timestamp = chrono::DateTime::parse_from_rfc3339(message.created_at.trim())
        .map(|created| {
            format!(
                " displayTimestamp=\"{}\"",
                created
                    .with_timezone(&chrono::Utc)
                    .format("%Y-%m-%dT%H:%M:%SZ")
            )
        })
        .unwrap_or_default();
    let content = if body.trim().is_empty() {
        String::new()
    } else {
        format!("<text>{}</text>", escape(body))
    };
    format!(
        "<toast launch=\"{}\"{}{timestamp}><visual><binding template=\"ToastGeneric\">\
         <text>{}</text>{content}</binding></visual></toast>",
        escape(&launch),
        if urgent { " scenario=\"urgent\"" } else { "" },
        escape(title),
    )
}

/// XML text and attribute values (SecurityElement.Escape).
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// The Windows build number (0 when unknown). Read from the registry: the
/// version functions report Windows 8 to a binary without a manifest.
fn os_build() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| {
        reg_read(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuildNumber",
        )
        .and_then(|build| build.trim().parse().ok())
        .unwrap_or(0)
    })
}

/// The per-user registration the app writes on every start, so toasts show
/// "PPVPN" with its icon even when the app has not run since an upgrade,
/// and a click starts ppvpn.exe (never the agent). Only when ppvpn.exe is
/// next to the agent (the install directory).
fn register_aumid() {
    let Some(dir) = exe_dir() else {
        return;
    };
    let app = dir.join(APP_EXE);
    if !app.is_file() {
        tracing::warn!(
            "push agent: {APP_EXE} is not in {}; leaving the notification registration alone",
            dir.display()
        );
        return;
    }
    let aumid_key = format!(r"Software\Classes\AppUserModelId\{AUMID}");
    let server_key = format!(r"Software\Classes\CLSID\{{{ACTIVATOR_CLSID}}}\LocalServer32");
    let icon = dir.join("Assets").join("AppLogo.png");
    let mut values = vec![
        (aumid_key.as_str(), "DisplayName", DISPLAY_NAME.to_string()),
        (
            aumid_key.as_str(),
            "CustomActivator",
            format!("{{{ACTIVATOR_CLSID}}}"),
        ),
        (
            server_key.as_str(),
            "",
            format!("\"{}\" {TOAST_ACTIVATED_SWITCH}", app.display()),
        ),
    ];
    // Only an existing file.
    if icon.is_file() {
        values.push((aumid_key.as_str(), "IconUri", icon.display().to_string()));
    }
    let mut changed = false;
    for (key, name, value) in &values {
        if reg_read(HKEY_CURRENT_USER, key, name).as_deref() == Some(value.as_str()) {
            continue;
        }
        if let Err(error) = reg_write(HKEY_CURRENT_USER, key, name, value) {
            tracing::warn!("push agent: notification registration failed: {error}");
            return;
        }
        changed = true;
    }
    if changed {
        tracing::info!("push agent: notification registration written for {AUMID}");
    } else {
        tracing::info!("push agent: notification registration for {AUMID} is current");
    }
}

/// A REG_SZ value (`name` empty: the key's default value).
fn reg_read(root: HKEY, key: &str, name: &str) -> Option<String> {
    let (key, name) = (wide(key), wide(name));
    let mut size = 0u32;
    // SAFETY: NUL-terminated strings; a size query without a buffer.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    let mut size = u32::try_from(buffer.len() * 2).ok()?;
    // SAFETY: `buffer` holds `size` bytes.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// Writes a REG_SZ value, creating the key when it is missing.
fn reg_write(root: HKEY, key: &str, name: &str, value: &str) -> std::io::Result<()> {
    let (key, name, value) = (wide(key), wide(name), wide(value));
    let size = u32::try_from(value.len() * 2).map_err(std::io::Error::other)?;
    // SAFETY: NUL-terminated strings; `value` holds `size` bytes.
    let status = unsafe {
        RegSetKeyValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            value.as_ptr().cast(),
            size,
        )
    };
    win32_result(status)
}

fn reg_delete(root: HKEY, key: &str, name: &str) -> std::io::Result<()> {
    let (key, name) = (wide(key), wide(name));
    // SAFETY: NUL-terminated strings.
    let status = unsafe { RegDeleteKeyValueW(root, key.as_ptr(), name.as_ptr()) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    win32_result(status)
}

fn win32_result(status: u32) -> std::io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ppvpn_client::MessageCategory;

    fn message(title: &str, body: &str) -> PushMessage {
        PushMessage {
            id: 42,
            message_id: None,
            title: title.into(),
            body: body.into(),
            severity: MessageSeverity::Normal,
            category: MessageCategory::Other,
            event_key: "order.paid".into(),
            deep_link: None,
            created_at: "2026-09-29T08:00:00.5+08:00".into(),
        }
    }

    #[test]
    fn toast_xml_is_escaped_and_carries_only_the_id() {
        let xml = toast_xml(&message("A & B", "<b>\"x\"</b>"));
        assert_eq!(
            xml,
            "<toast launch=\"id=42\" displayTimestamp=\"2026-09-29T00:00:00Z\">\
             <visual><binding template=\"ToastGeneric\"><text>A &amp; B</text>\
             <text>&lt;b&gt;&quot;x&quot;&lt;/b&gt;</text></binding></visual></toast>"
        );
    }

    #[test]
    fn an_empty_title_takes_the_body_or_the_name() {
        assert!(toast_xml(&message(" ", "only body")).contains("<text>only body</text></binding>"));
        assert!(toast_xml(&message("", "")).contains("<text>PPVPN</text></binding>"));
    }
}
