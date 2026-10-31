//! One instance of the app per user (#401 step 4), as the native apps keep
//! it: a second launch hands its arguments to the running instance and
//! exits, and the running one shows its window or opens the notification
//! they name. Notification clicks end up in [`next`] whichever way they
//! come:
//!
//! - Linux: the push agent runs `ppvpn --open-notification <push id>`
//!   (apps/linux/App/PPVPNApplication.cs). The running instance holds an
//!   exclusive lock on `instance.lock` in the data directory and listens on
//!   `instance.sock` beside it.
//! - Windows: the toast's COM activator ([`toast`], as
//!   apps/windows/PPVPN.Windows/Platform/AppNotifications.cs). The running
//!   instance owns the first instance of a named pipe named after the data
//!   directory. The push agent writes the activator's registration (and
//!   so does the app at start, `aumid.rs`). The running instance also
//!   owns the installer's quit event (`QUIT_EVENT`): set, it quits the
//!   app as the tray's Quit does.
//! - macOS: nothing here. Launch Services keeps one instance, and the
//!   push agent's `ppvpn://push/<id>` reaches the runner
//!   (runner/macos/Runner/MainFlutterWindow.swift, lib/activation.dart).
//!
//! A forwarded launch is its arguments as a JSON array of strings.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use tokio::sync::Notify;

use crate::api::UiActivation;

/// The push agent's argument on Linux, followed by the push id.
const OPEN_SWITCH: &str = "--open-notification";
/// A click forwarded to the running instance on Windows, as the native
/// app forwards it: `--notification-activated=id%3D<push id>`.
const FORWARD_SWITCH: &str = "--notification-activated=";
/// Set by the installer and the uninstaller (apps/flutter/packaging/windows/ppvpn.nsi,
/// `QUIT_EVENT`) to have the running app quit through its normal path
/// before they replace its files; the native app's QuitSignal
/// (apps/windows/PPVPN.Windows/Platform/AppUpdater.cs). Only the running
/// instance creates it: the installer takes its existence for a running app
/// and starts the app again after a silent upgrade.
#[cfg(windows)]
const QUIT_EVENT: &str = r"Local\PPVPN.Desktop.Quit";
/// More than any launch's arguments.
#[cfg(any(target_os = "linux", windows))]
const MAX_MESSAGE: usize = 64 * 1024;

static QUEUE: Mutex<VecDeque<UiActivation>> = Mutex::new(VecDeque::new());
static READY: Notify = Notify::const_new();

/// See [`crate::api::claim_instance`].
pub(crate) fn claim() -> bool {
    static CLAIMED: OnceLock<bool> = OnceLock::new();
    *CLAIMED.get_or_init(|| {
        let args: Vec<String> = std::env::args_os()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        match platform::claim(&args) {
            Ok(true) => {
                // Started for a click (Linux, or a click forwarded to an
                // instance that has gone since).
                if push_in(&args).is_some() {
                    deliver(activation(&args));
                }
                true
            }
            Ok(false) => false,
            Err(error) => {
                tracing::warn!(%error, "single instance unavailable; running anyway");
                true
            }
        }
    })
}

pub(crate) async fn next() -> UiActivation {
    loop {
        let next = QUEUE.lock().unwrap().pop_front();
        if let Some(activation) = next {
            return activation;
        }
        READY.notified().await;
    }
}

pub(crate) fn deliver(activation: UiActivation) {
    QUEUE.lock().unwrap().push_back(activation);
    READY.notify_one();
}

fn activation(args: &[String]) -> UiActivation {
    UiActivation {
        push_id: push_in(args).and_then(|id| i64::try_from(id).ok()),
        quit: false,
        update: false,
    }
}

/// Another launch's arguments, from the platform's listener.
#[cfg(any(target_os = "linux", windows))]
fn received(message: &[u8]) {
    match serde_json::from_slice::<Vec<String>>(message) {
        Ok(args) => deliver(activation(&args)),
        Err(error) => tracing::warn!(%error, "another launch's arguments unreadable"),
    }
}

/// The push a launch's arguments name.
fn push_in(args: &[String]) -> Option<u64> {
    if let Some(at) = args.iter().position(|arg| arg == OPEN_SWITCH) {
        return args.get(at + 1).map(String::as_str).and_then(digits);
    }
    args.iter()
        .find_map(|arg| arg.strip_prefix(FORWARD_SWITCH))
        .and_then(|value| toast_id(&percent_decoded(value)))
}

/// The push id in a toast's launch arguments, `id=<push id>` (the push
/// agent writes them; other pairs are ignored).
fn toast_id(arguments: &str) -> Option<u64> {
    arguments
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(name, _)| *name == "id")
        .filter_map(|(_, value)| digits(value))
        .next_back()
}

/// Only ASCII digits (no sign, no spaces).
fn digits(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs::{File, Permissions, TryLockError};
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::sync::OnceLock;
    use std::time::Duration;

    /// Held for the process's life: holding the lock is being the instance.
    static LOCK: OnceLock<File> = OnceLock::new();

    pub(super) fn claim(args: &[String]) -> Result<bool, String> {
        let (data_dir, _) = crate::dirs::dirs()?;
        let dir = Path::new(&data_dir);
        let socket = dir.join("instance.sock");
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("instance.lock"))
            .map_err(|error| format!("instance.lock: {error}"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                // Another instance runs: it shows its window. Exit even when
                // it does not answer (as the native apps give up).
                if let Err(error) = forward(&socket, args) {
                    tracing::warn!(%error, "the running instance did not take this launch");
                }
                return Ok(false);
            }
            Err(TryLockError::Error(error)) => return Err(format!("instance.lock: {error}")),
        }
        let _ = LOCK.set(lock);
        // Left by an instance that has gone.
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket)
            .map_err(|error| format!("{}: {error}", socket.display()))?;
        std::fs::set_permissions(&socket, Permissions::from_mode(0o600))
            .map_err(|error| format!("{}: {error}", socket.display()))?;
        std::thread::Builder::new()
            .name("ppvpn-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut message = Vec::new();
                    if stream
                        .take(super::MAX_MESSAGE as u64)
                        .read_to_end(&mut message)
                        .is_ok()
                    {
                        super::received(&message);
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    fn forward(socket: &Path, args: &[String]) -> Result<(), String> {
        let message = serde_json::to_vec(args).map_err(|error| error.to_string())?;
        // The running instance takes the lock a moment before it listens.
        let mut last = String::new();
        for _ in 0..20 {
            match UnixStream::connect(socket) {
                Ok(mut stream) => {
                    return stream
                        .write_all(&message)
                        .map_err(|error| error.to_string())
                }
                Err(error) => last = error.to_string(),
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(last)
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE,
        FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
        WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }

    fn os_error(code: u32) -> String {
        std::io::Error::from_raw_os_error(code as i32).to_string()
    }

    pub(super) fn claim(args: &[String]) -> Result<bool, String> {
        let (data_dir, _) = crate::dirs::dirs()?;
        // Per user (and per PPVPN_FLUTTER_HOME); a pipe name has no "\".
        let name = wide(&format!(r"\\.\pipe\ppvpn-{}", data_dir.replace('\\', "/")));
        let mut forwarded: Option<Vec<String>> = None;
        for _ in 0..30 {
            // SAFETY: a NUL-terminated name and no security attributes (the
            // default ones let only this user, SYSTEM and admins write).
            let pipe = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    0,
                    super::MAX_MESSAGE as u32,
                    0,
                    std::ptr::null(),
                )
            };
            if pipe != INVALID_HANDLE_VALUE {
                serve(pipe as usize)?;
                super::toast::register();
                listen_for_quit();
                return Ok(true);
            }
            // SAFETY: no other call in between.
            let error = unsafe { GetLastError() };
            if error != ERROR_ACCESS_DENIED {
                return Err(os_error(error));
            }
            // Another instance has the pipe. Once it has gone (exiting a
            // moment ago), try to be the instance again.
            let message = forwarded.get_or_insert_with(|| super::toast::forwarded(args));
            if forward(&name, message).is_ok() {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        tracing::warn!("the running instance did not take this launch");
        Ok(false)
    }

    /// Answers one launch at a time on the pipe's one instance; a launch
    /// meanwhile waits (`ERROR_PIPE_BUSY`). The handle travels as a number
    /// (`HANDLE` is a pointer, not `Send`).
    fn serve(pipe: usize) -> Result<(), String> {
        std::thread::Builder::new()
            .name("ppvpn-instance".into())
            .spawn(move || {
                let pipe = pipe as HANDLE;
                loop {
                    // SAFETY: the pipe's handle stays open for the process's
                    // life; the calls are synchronous.
                    unsafe {
                        if ConnectNamedPipe(pipe, std::ptr::null_mut()) != 0
                            || GetLastError() == ERROR_PIPE_CONNECTED
                        {
                            let message = read(pipe);
                            if !message.is_empty() {
                                super::received(&message);
                            }
                        } else {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        DisconnectNamedPipe(pipe);
                    }
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Waits for the installer's quit event on a thread of its own and
    /// hands it on as an activation. Without the event the installer falls
    /// back to terminating the process, as with the native app.
    fn listen_for_quit() {
        let name = wide(super::QUIT_EVENT);
        // SAFETY: a NUL-terminated name; an auto-reset event, not set. The
        // handle stays open for the process's life.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if event.is_null() {
            // SAFETY: no other call in between.
            let error = unsafe { GetLastError() };
            tracing::warn!("{}: {}", super::QUIT_EVENT, os_error(error));
            return;
        }
        let event = event as usize;
        let spawned = std::thread::Builder::new()
            .name("ppvpn-quit".into())
            .spawn(move || {
                // SAFETY: the handle is open (above) and never closed.
                if unsafe { WaitForSingleObject(event as HANDLE, INFINITE) } == WAIT_OBJECT_0 {
                    tracing::info!("quit requested ({})", super::QUIT_EVENT);
                    super::deliver(crate::api::UiActivation {
                        push_id: None,
                        quit: true,
                        update: false,
                    });
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "quit event not watched");
        }
    }

    /// Until the launch closes its end.
    fn read(pipe: HANDLE) -> Vec<u8> {
        let mut message = Vec::new();
        let mut buffer = [0u8; 4096];
        while message.len() < super::MAX_MESSAGE {
            let mut read = 0u32;
            // SAFETY: `buffer` outlives the synchronous call.
            let ok = unsafe {
                ReadFile(
                    pipe,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || read == 0 {
                break;
            }
            message.extend_from_slice(&buffer[..read as usize]);
        }
        message
    }

    fn forward(name: &[u16], args: &[String]) -> Result<(), String> {
        let message = serde_json::to_vec(args).map_err(|error| error.to_string())?;
        // SAFETY: a NUL-terminated name; the handle is closed below.
        let pipe = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_NONE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            // SAFETY: no other call in between; then a bounded wait.
            let error = unsafe { GetLastError() };
            if error == ERROR_PIPE_BUSY {
                unsafe { WaitNamedPipeW(name.as_ptr(), 2000) };
            }
            return Err(os_error(error));
        }
        let mut rest = &message[..];
        // SAFETY: `pipe` is open until CloseHandle; `rest` outlives each
        // synchronous write.
        unsafe {
            // Let the running instance bring its window to the front.
            let mut server = 0u32;
            if GetNamedPipeServerProcessId(pipe, &mut server) != 0 {
                AllowSetForegroundWindow(server);
            }
            while !rest.is_empty() {
                let mut written = 0u32;
                if WriteFile(
                    pipe,
                    rest.as_ptr(),
                    rest.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                ) == 0
                {
                    break;
                }
                rest = &rest[written as usize..];
            }
            // Returns once the instance has read it all.
            FlushFileBuffers(pipe);
            CloseHandle(pipe);
        }
        if rest.is_empty() {
            Ok(())
        } else {
            Err("the running instance stopped reading".into())
        }
    }
}

/// macOS keeps one instance itself (see the module's documentation).
#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    pub(super) fn claim(_args: &[String]) -> Result<bool, String> {
        Ok(true)
    }
}

/// The toast's click activator: a COM class object (`CLSID`) that the
/// notification platform calls with the toast's launch arguments. The push
/// agent registers this exe as its local server (`--toast-activated`), so
/// a click also starts the app. The objects are statics, so counting
/// references is moot.
#[cfg(windows)]
mod toast {
    use std::ffi::c_void;
    use std::sync::{Condvar, Mutex, Once};
    use std::time::{Duration, Instant};

    use windows_sys::core::{BOOL, GUID, HRESULT, PCWSTR};
    use windows_sys::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_NOINTERFACE, E_POINTER, S_OK};
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoRegisterClassObject, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED,
        REGCLS_MULTIPLEUSE,
    };

    /// COM starts the app with this (and `-Embedding`) to deliver a click.
    const TOAST_SWITCH: &str = "--toast-activated";
    /// The AUMID's CustomActivator (AppNotifications.cs, the push agent's
    /// PushNotifier.cs, installer/ppvpn.nsi).
    const CLSID: GUID = GUID::from_u128(0xFCD3C3FA_FCA6_4F2D_BC9E_BEA74D70EBBE);
    const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_C000_000000000046);
    const IID_ICLASSFACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_C000_000000000046);
    /// INotificationActivationCallback.
    const IID_CALLBACK: GUID = GUID::from_u128(0x53E31837_6600_4A81_9395_75CFFE746F94);

    type Raw = *mut c_void;

    // COM reads these fields through the object's pointer; Rust never does.
    #[allow(dead_code)]
    #[repr(C)]
    struct Object<V: 'static> {
        vtable: &'static V,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct FactoryVtable {
        query_interface: unsafe extern "system" fn(Raw, *const GUID, *mut Raw) -> HRESULT,
        add_ref: unsafe extern "system" fn(Raw) -> u32,
        release: unsafe extern "system" fn(Raw) -> u32,
        create_instance: unsafe extern "system" fn(Raw, Raw, *const GUID, *mut Raw) -> HRESULT,
        lock_server: unsafe extern "system" fn(Raw, BOOL) -> HRESULT,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct CallbackVtable {
        query_interface: unsafe extern "system" fn(Raw, *const GUID, *mut Raw) -> HRESULT,
        add_ref: unsafe extern "system" fn(Raw) -> u32,
        release: unsafe extern "system" fn(Raw) -> u32,
        activate: unsafe extern "system" fn(Raw, PCWSTR, PCWSTR, *const c_void, u32) -> HRESULT,
    }

    static FACTORY_VTABLE: FactoryVtable = FactoryVtable {
        query_interface: factory_query_interface,
        add_ref,
        release,
        create_instance,
        lock_server,
    };
    static FACTORY: Object<FactoryVtable> = Object {
        vtable: &FACTORY_VTABLE,
    };
    static CALLBACK_VTABLE: CallbackVtable = CallbackVtable {
        query_interface: callback_query_interface,
        add_ref,
        release,
        activate,
    };
    static CALLBACK: Object<CallbackVtable> = Object {
        vtable: &CALLBACK_VTABLE,
    };

    /// The latest click in this process, for [`forwarded`].
    static CLICK: Mutex<Option<u64>> = Mutex::new(None);
    static CLICKED: Condvar = Condvar::new();

    /// Receive clicks in this process from now on. The class object lives
    /// in the multithreaded apartment, which a parked thread keeps; COM
    /// calls it on its own threads.
    pub(super) fn register() {
        static STARTED: Once = Once::new();
        STARTED.call_once(|| {
            let spawned = std::thread::Builder::new()
                .name("ppvpn-toast".into())
                .spawn(|| {
                    let mut cookie = 0u32;
                    // SAFETY: the class object is a static; COM only calls
                    // through its vtable.
                    let hr = unsafe {
                        let hr = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
                        if hr < 0 {
                            hr
                        } else {
                            CoRegisterClassObject(
                                &CLSID,
                                (&raw const FACTORY).cast_mut().cast(),
                                CLSCTX_LOCAL_SERVER,
                                REGCLS_MULTIPLEUSE as u32,
                                &mut cookie,
                            )
                        }
                    };
                    if hr < 0 {
                        tracing::warn!("notification activator not registered: {hr:#010x}");
                        return;
                    }
                    loop {
                        std::thread::park();
                    }
                });
            if let Err(error) = spawned {
                tracing::warn!(%error, "notification activator not started");
            }
        });
    }

    /// The arguments to hand to the running instance. For a launch COM
    /// started for a click while another instance runs, that is the click:
    /// it is taken here (up to 10 s) and forwarded as the native app does.
    pub(super) fn forwarded(args: &[String]) -> Vec<String> {
        if !args.iter().any(|arg| arg == TOAST_SWITCH) {
            return args.to_vec();
        }
        register();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut click = CLICK.lock().unwrap();
        while click.is_none() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            click = CLICKED.wait_timeout(click, left).unwrap().0;
        }
        match *click {
            Some(id) => vec![format!("{}id%3D{id}", super::FORWARD_SWITCH)],
            None => args.to_vec(),
        }
    }

    fn same(a: &GUID, b: &GUID) -> bool {
        a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
    }

    /// Hands out `object` when `iid` is one of `iids`.
    unsafe fn answer(iid: *const GUID, out: *mut Raw, iids: &[GUID], object: Raw) -> HRESULT {
        if out.is_null() || iid.is_null() {
            return E_POINTER;
        }
        // SAFETY: COM passes a valid IID and out pointer.
        unsafe {
            if iids.iter().any(|known| same(known, &*iid)) {
                *out = object;
                S_OK
            } else {
                *out = std::ptr::null_mut();
                E_NOINTERFACE
            }
        }
    }

    unsafe extern "system" fn factory_query_interface(
        this: Raw,
        iid: *const GUID,
        out: *mut Raw,
    ) -> HRESULT {
        unsafe { answer(iid, out, &[IID_IUNKNOWN, IID_ICLASSFACTORY], this) }
    }

    unsafe extern "system" fn callback_query_interface(
        this: Raw,
        iid: *const GUID,
        out: *mut Raw,
    ) -> HRESULT {
        unsafe { answer(iid, out, &[IID_IUNKNOWN, IID_CALLBACK], this) }
    }

    unsafe extern "system" fn add_ref(_this: Raw) -> u32 {
        1
    }

    unsafe extern "system" fn release(_this: Raw) -> u32 {
        1
    }

    unsafe extern "system" fn create_instance(
        _this: Raw,
        outer: Raw,
        iid: *const GUID,
        out: *mut Raw,
    ) -> HRESULT {
        if !outer.is_null() {
            return CLASS_E_NOAGGREGATION;
        }
        let callback = (&raw const CALLBACK).cast_mut().cast();
        unsafe { answer(iid, out, &[IID_IUNKNOWN, IID_CALLBACK], callback) }
    }

    unsafe extern "system" fn lock_server(_this: Raw, _lock: BOOL) -> HRESULT {
        S_OK
    }

    /// INotificationActivationCallback::Activate: a click on one of our
    /// toasts, with its launch arguments.
    unsafe extern "system" fn activate(
        _this: Raw,
        _app: PCWSTR,
        arguments: PCWSTR,
        _data: *const c_void,
        _count: u32,
    ) -> HRESULT {
        if arguments.is_null() {
            return S_OK;
        }
        // SAFETY: COM passes a NUL-terminated string.
        let arguments = unsafe {
            let mut len = 0;
            while *arguments.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(arguments, len))
        };
        if let Some(id) = super::toast_id(&arguments) {
            *CLICK.lock().unwrap() = Some(id);
            CLICKED.notify_all();
            super::deliver(crate::api::UiActivation {
                push_id: i64::try_from(id).ok(),
                quit: false,
                update: false,
            });
        }
        S_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn push_from_the_arguments() {
        assert_eq!(push_in(&args(&["--open-notification", "42"])), Some(42));
        assert_eq!(
            push_in(&args(&["--notification-activated=id%3D7"])),
            Some(7)
        );
        assert_eq!(
            push_in(&args(&["--notification-activated=a%3Db%26id%3D9"])),
            Some(9)
        );
        assert_eq!(push_in(&args(&["--open-notification"])), None);
        assert_eq!(push_in(&args(&["--open-notification", "+4"])), None);
        assert_eq!(push_in(&args(&["--toast-activated", "-Embedding"])), None);
        assert_eq!(push_in(&args(&[])), None);
    }

    #[test]
    fn toast_arguments() {
        assert_eq!(toast_id("id=12"), Some(12));
        assert_eq!(toast_id("x=1&id=3"), Some(3));
        assert_eq!(toast_id("id="), None);
        assert_eq!(toast_id("id=-1"), None);
        assert_eq!(toast_id(""), None);
    }
}
