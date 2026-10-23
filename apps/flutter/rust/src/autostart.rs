//! Launch at login (#401 step 4), as the native apps do it: macOS through
//! `SMAppService.mainApp` (apps/macos LoginItem), Windows through the
//! per-user Run key (apps/windows LaunchAtLogin), Linux through an XDG
//! autostart entry (apps/linux Autostart). The system holds the state: it is
//! read back each time, never kept in the app's settings. Off by default.
//!
//! The Windows and Linux entries start the app with `--background`: the tray
//! only, no window. The macOS login item starts it without arguments and the
//! window opens, as the native app's does.

/// Command-line flag that starts the app hidden in the tray.
pub(crate) const BACKGROUND_ARGUMENT: &str = "--background";

/// The app was started with [`BACKGROUND_ARGUMENT`].
pub(crate) fn started_in_background() -> bool {
    std::env::args_os()
        .skip(1)
        .any(|arg| arg == BACKGROUND_ARGUMENT)
}

pub(crate) use platform::{open_settings, refresh, set, state};

/// macOS 13+: the app itself as a login item. Called through the
/// Objective-C runtime (no bindings crate for four messages).
///
/// Every message goes through [`catching`]: an Objective-C exception must
/// not unwind into the bridge, whose `catch_unwind` aborts on a foreign
/// exception (0.7.0 acceptance: Settings crashed the app each time).
#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_void, CStr};

    use crate::api::UiLaunchAtLogin;

    type Id = *mut c_void;
    type Sel = *mut c_void;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
        fn free(pointer: *mut c_void);
    }

    // "C-unwind": an Objective-C exception may unwind through these calls,
    // up to ppvpn_objc_catch.
    #[link(name = "objc")]
    extern "C-unwind" {
        /// No signature of its own: each call casts it to the method's.
        fn objc_msgSend();
    }

    extern "C-unwind" {
        /// src/objc_catch.m (build.rs): runs `body(context)` in `@try`;
        /// null, or the exception it raised as text the caller frees.
        fn ppvpn_objc_catch(
            body: unsafe extern "C-unwind" fn(*mut c_void),
            context: *mut c_void,
        ) -> *mut c_char;
    }

    // Loads SMAppService for objc_getClass.
    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    /// SMAppServiceStatus.
    const ENABLED: isize = 1;
    const REQUIRES_APPROVAL: isize = 2;

    // SAFETY (the four below): `receiver` is a live object or class that
    // answers `selector` with exactly this signature.
    unsafe fn send_id(receiver: Id, selector: &CStr) -> Id {
        let send = std::mem::transmute::<
            unsafe extern "C-unwind" fn(),
            unsafe extern "C-unwind" fn(Id, Sel) -> Id,
        >(objc_msgSend);
        send(receiver, sel_registerName(selector.as_ptr()))
    }

    unsafe fn send_int(receiver: Id, selector: &CStr) -> isize {
        let send = std::mem::transmute::<
            unsafe extern "C-unwind" fn(),
            unsafe extern "C-unwind" fn(Id, Sel) -> isize,
        >(objc_msgSend);
        send(receiver, sel_registerName(selector.as_ptr()))
    }

    unsafe fn send_void(receiver: Id, selector: &CStr) {
        let send = std::mem::transmute::<
            unsafe extern "C-unwind" fn(),
            unsafe extern "C-unwind" fn(Id, Sel),
        >(objc_msgSend);
        send(receiver, sel_registerName(selector.as_ptr()));
    }

    /// `- (BOOL)…AndReturnError:(NSError **)error`; BOOL is one byte on
    /// both architectures.
    unsafe fn send_with_error(receiver: Id, selector: &CStr, error: *mut Id) -> bool {
        let send = std::mem::transmute::<
            unsafe extern "C-unwind" fn(),
            unsafe extern "C-unwind" fn(Id, Sel, *mut Id) -> i8,
        >(objc_msgSend);
        send(receiver, sel_registerName(selector.as_ptr()), error) != 0
    }

    /// Runs `body` in an autorelease pool: the calls come from the bridge's
    /// threads, which have none.
    fn in_pool<T>(body: impl FnOnce() -> T) -> T {
        // SAFETY: the pool is popped once, after the body.
        let pool = unsafe { objc_autoreleasePoolPush() };
        let result = body();
        // SAFETY: as above.
        unsafe { objc_autoreleasePoolPop(pool) };
        result
    }

    /// Runs `body` with Objective-C exceptions caught: one it raises is the
    /// error, its name and reason.
    fn catching<T>(body: impl FnOnce() -> T) -> Result<T, String> {
        unsafe extern "C-unwind" fn run<F: FnOnce()>(context: *mut c_void) {
            // SAFETY: `context` is the `Option<F>` of `call`, alive for the call.
            if let Some(body) = unsafe { (*context.cast::<Option<F>>()).take() } {
                body();
            }
        }
        fn call<F: FnOnce()>(body: F) -> *mut c_char {
            let mut body = Some(body);
            // SAFETY: `run::<F>` takes the `Option<F>` it is given.
            unsafe { ppvpn_objc_catch(run::<F>, (&mut body as *mut Option<F>).cast()) }
        }
        let mut output = None;
        let error = call(|| output = Some(body()));
        if error.is_null() {
            return output.ok_or_else(|| "Objective-C call did not return".into());
        }
        // SAFETY: a NUL-terminated string from strdup, freed once.
        let text = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: as above.
        unsafe { free(error.cast()) };
        Err(text)
    }

    /// The SMAppService class; null before macOS 13.
    fn class() -> Id {
        // SAFETY: a NUL-terminated name.
        unsafe { objc_getClass(c"SMAppService".as_ptr()) }
    }

    /// `[SMAppService mainApp]`; null before macOS 13.
    fn main_app() -> Id {
        let class = class();
        if class.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: `+ (SMAppService *)mainApp`.
        unsafe { send_id(class, c"mainApp") }
    }

    pub(crate) fn state() -> Result<UiLaunchAtLogin, String> {
        let status = in_pool(|| {
            catching(|| {
                let app = main_app();
                if app.is_null() {
                    return 0;
                }
                // SAFETY: `@property(readonly) SMAppServiceStatus status`, an NSInteger.
                unsafe { send_int(app, c"status") }
            })
        })?;
        Ok(UiLaunchAtLogin {
            enabled: status == ENABLED,
            needs_approval: status == REQUIRES_APPROVAL,
        })
    }

    pub(crate) fn set(enabled: bool) -> Result<(), String> {
        in_pool(|| catching(|| register(enabled)))?
    }

    fn register(enabled: bool) -> Result<(), String> {
        let app = main_app();
        if app.is_null() {
            return Err("launch at login needs macOS 13 or later".into());
        }
        let selector = if enabled {
            c"registerAndReturnError:"
        } else {
            c"unregisterAndReturnError:"
        };
        let mut error: Id = std::ptr::null_mut();
        // SAFETY: `- (BOOL)registerAndReturnError:(NSError **)error`,
        // and the same for unregister.
        if unsafe { send_with_error(app, selector, &mut error) } {
            return Ok(());
        }
        Err(description(error))
    }

    /// The error's `localizedDescription`; inside the pool that holds it.
    fn description(error: Id) -> String {
        if error.is_null() {
            return "SMAppService failed".into();
        }
        // SAFETY: an NSError, its NSString, and that string's UTF-8 bytes,
        // which live as long as the pool.
        unsafe {
            let text = send_id(error, c"localizedDescription");
            if text.is_null() {
                return "SMAppService failed".into();
            }
            let bytes = send_id(text, c"UTF8String") as *const c_char;
            if bytes.is_null() {
                return "SMAppService failed".into();
            }
            CStr::from_ptr(bytes).to_string_lossy().into_owned()
        }
    }

    /// System Settings › General › Login Items.
    pub(crate) fn open_settings() -> Result<(), String> {
        let class = class();
        if class.is_null() {
            return Ok(());
        }
        // SAFETY: `+ (void)openSystemSettingsLoginItems`.
        in_pool(|| catching(|| unsafe { send_void(class, c"openSystemSettingsLoginItems") }))
    }

    /// The login item follows the bundle: nothing to point anywhere.
    pub(crate) fn refresh() {}

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn an_objective_c_exception_is_an_error() {
            // `[[NSArray array] objectAtIndex:1]` raises NSRangeException.
            let result = in_pool(|| {
                catching(|| unsafe {
                    let array = send_id(objc_getClass(c"NSArray".as_ptr()), c"array");
                    let send = std::mem::transmute::<
                        unsafe extern "C-unwind" fn(),
                        unsafe extern "C-unwind" fn(Id, Sel, usize) -> Id,
                    >(objc_msgSend);
                    send(array, sel_registerName(c"objectAtIndex:".as_ptr()), 1)
                })
            });
            let error = result.expect_err("objectAtIndex: past the end raises");
            assert!(error.starts_with("NSRangeException: "), "{error}");
        }

        #[test]
        fn a_call_without_an_exception_returns_its_value() {
            assert_eq!(catching(|| 7), Ok(7));
        }

        /// Read from a test binary, outside any app bundle: a state or an
        /// error, not an abort.
        #[test]
        fn the_state_is_read_without_aborting() {
            let _ = state();
        }
    }
}

/// Windows: HKCU\…\Run, value `PPVPN` = `"<exe>" --background`.
#[cfg(windows)]
mod platform {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
        RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
    };
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};

    use crate::api::UiLaunchAtLogin;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    const VALUE_NAME: &str = "PPVPN";
    /// SW_SHOWNORMAL (Win32_UI_WindowsAndMessaging, not worth the feature).
    const SW_SHOWNORMAL: i32 = 1;

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(Some(0)).collect()
    }

    /// `"<exe>" --background`, without the NUL.
    fn command() -> Option<Vec<u16>> {
        let exe = std::env::current_exe().ok()?;
        let mut command = OsString::from("\"");
        command.push(exe.as_os_str());
        command.push("\" ");
        command.push(super::BACKGROUND_ARGUMENT);
        Some(command.encode_wide().collect())
    }

    /// The value `PPVPN` under HKCU\`key`; None when it is missing or not
    /// of a type `kind` (RRF_RT_*) allows.
    fn read(key: &str, kind: u32) -> Option<Vec<u8>> {
        let key = wide(OsStr::new(key));
        let name = wide(OsStr::new(VALUE_NAME));
        let mut data: Vec<u8> = Vec::new();
        loop {
            let mut size = u32::try_from(data.len()).ok()?;
            let buffer = if data.is_empty() {
                std::ptr::null_mut()
            } else {
                data.as_mut_ptr().cast()
            };
            // SAFETY: NUL-terminated names; `buffer` is null (a size query)
            // or holds `size` bytes.
            let status = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    kind,
                    std::ptr::null_mut(),
                    buffer,
                    &mut size,
                )
            };
            match status {
                ERROR_SUCCESS if !data.is_empty() || size == 0 => {
                    data.truncate(size as usize);
                    return Some(data);
                }
                // The size, or the value grew since: again with room for it.
                ERROR_SUCCESS | ERROR_MORE_DATA => data = vec![0; size as usize],
                _ => return None,
            }
        }
    }

    /// The Run entry's command without NULs; empty when there is none.
    fn entry() -> Vec<u16> {
        let bytes = read(RUN_KEY, RRF_RT_REG_SZ).unwrap_or_default();
        let mut text: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        while text.last() == Some(&0) {
            text.pop();
        }
        text
    }

    fn write(command: &[u16]) -> u32 {
        let key = wide(OsStr::new(RUN_KEY));
        let name = wide(OsStr::new(VALUE_NAME));
        let data: Vec<u16> = command.iter().copied().chain(Some(0)).collect();
        // SAFETY: NUL-terminated names; `data` is a NUL-terminated string of
        // the given size in bytes.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                std::mem::size_of_val(data.as_slice()) as u32,
            )
        }
    }

    /// The Run entry exists but was turned off in Settings › Apps › Startup
    /// (or Task Manager): Explorer records that in StartupApproved\Run as a
    /// binary value whose first byte is odd (02/06 = enabled, 03/07 =
    /// disabled). Only Windows can turn it back on.
    pub(crate) fn state() -> Result<UiLaunchAtLogin, String> {
        let enabled = !entry().is_empty();
        let blocked = || {
            read(APPROVED_KEY, RRF_RT_REG_BINARY)
                .and_then(|value| value.first().copied())
                .is_some_and(|first| first & 1 == 1)
        };
        Ok(UiLaunchAtLogin {
            enabled,
            needs_approval: enabled && blocked(),
        })
    }

    pub(crate) fn set(enabled: bool) -> Result<(), String> {
        let status = if enabled {
            write(&command().ok_or("the app's path is unknown")?)
        } else {
            let key = wide(OsStr::new(RUN_KEY));
            let name = wide(OsStr::new(VALUE_NAME));
            // SAFETY: NUL-terminated names.
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                status => status,
            }
        };
        match status {
            ERROR_SUCCESS => Ok(()),
            status => Err(std::io::Error::from_raw_os_error(status as i32).to_string()),
        }
    }

    /// Settings › Apps › Startup.
    pub(crate) fn open_settings() -> Result<(), String> {
        let verb = wide(OsStr::new("open"));
        let page = wide(OsStr::new("ms-settings:startupapps"));
        // SAFETY: SHELLEXECUTEINFOW is plain data; all-zero is its empty
        // value (null pointers and handles).
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = page.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        // SAFETY: `info` and the strings it points to outlive the call.
        unsafe { ShellExecuteExW(&mut info) };
        Ok(())
    }

    /// Points an existing entry at the current exe (the app may have
    /// moved); at every start, as the native app does.
    pub(crate) fn refresh() {
        let current = entry();
        if current.is_empty() {
            return;
        }
        if let Some(command) = command().filter(|command| *command != current) {
            write(&command);
        }
    }
}

/// Linux (and other Unix desktops): an XDG autostart entry, honoured by
/// GNOME, KDE and most other desktops. Like the menu entry, it caps glibc's
/// malloc arenas: the standard engine runs in this process.
#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use std::path::PathBuf;

    use crate::api::UiLaunchAtLogin;

    /// The native app's application id (apps/linux PPVPNApplication.Id):
    /// the entry's file name and icon.
    const APP_ID: &str = "com.peakpassvpn.ppvpn.desktop";

    /// `$XDG_CONFIG_HOME/autostart/<id>.desktop`, `~/.config` when the
    /// variable is unset or not absolute.
    fn entry_path() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(config.join("autostart").join(format!("{APP_ID}.desktop")))
    }

    pub(crate) fn state() -> Result<UiLaunchAtLogin, String> {
        Ok(UiLaunchAtLogin {
            enabled: entry_path().is_some_and(|path| path.exists()),
            needs_approval: false,
        })
    }

    pub(crate) fn set(enabled: bool) -> Result<(), String> {
        let path = entry_path().ok_or("HOME is not set")?;
        let failed = |error: std::io::Error| format!("{}: {error}", path.display());
        if !enabled {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(failed(error)),
                _ => Ok(()),
            };
        }
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(failed)?;
        }
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=PPVPN\n\
             Exec=env MALLOC_ARENA_MAX=2 \"{}\" {}\n\
             Icon={APP_ID}\n\
             X-GNOME-Autostart-enabled=true\n\
             NoDisplay=true\n",
            exe.display(),
            super::BACKGROUND_ARGUMENT,
        );
        std::fs::write(&path, entry).map_err(failed)
    }

    /// The desktop has no setting that holds the entry back.
    pub(crate) fn open_settings() -> Result<(), String> {
        Ok(())
    }

    /// Not rewritten at start, as in the native app.
    pub(crate) fn refresh() {}
}
