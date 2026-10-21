//! The app's identity in the Windows shell, its AppUserModelID, as the
//! native app has it (apps/windows/PPVPN.Windows/Platform/AppNotifications.cs,
//! ShortcutAumid.cs and Program.cs). The Start menu entry, the taskbar
//! button and the push agent's notifications share it:
//!
//! - the process's explicit AppUserModelID, set before any window shows;
//! - the per-user registration of the id (display name, icon) and of
//!   ppvpn.exe as its notifications' click activator (`instance.rs`). The
//!   push agent writes the same; the uninstaller removes both keys;
//! - `ppvpn.exe --set-shortcut-aumid <.lnk>`, run by the installer
//!   (apps/windows/installer/ppvpn.nsi) right after it creates a shortcut:
//!   stamps the id on the shortcut as System.AppUserModel.ID and exits, 0
//!   on success, 1 on failure, 2 without a path.
//!
//! Nothing on macOS and Linux.

/// See [`crate::api::claim_instance`]: first thing in the process.
pub(crate) fn prepare() {
    #[cfg(windows)]
    win::prepare();
}

#[cfg(windows)]
mod win {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::{Interface, GUID, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{E_FAIL, PROPERTYKEY};
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READWRITE,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    /// Also in installer/ppvpn.nsi and the push agent (push-agent/src/win.rs).
    const AUMID: &str = "PeakPass.PPVPN";
    const DISPLAY_NAME: &str = "PPVPN";
    /// The notifications' click activator (`instance.rs`, `toast::CLSID`).
    const ACTIVATOR_CLSID: &str = "FCD3C3FA-FCA6-4F2D-BC9E-BEA74D70EBBE";
    const TOAST_ACTIVATED_SWITCH: &str = "--toast-activated";
    const SHORTCUT_SWITCH: &str = "--set-shortcut-aumid";
    /// PKEY_AppUserModel_ID.
    const APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
        pid: 5,
    };

    pub(super) fn prepare() {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if let Some(at) = args
            .iter()
            .position(|arg| arg.as_os_str() == SHORTCUT_SWITCH)
        {
            let code = match args.get(at + 1) {
                None => 2,
                Some(shortcut) => match stamp(Path::new(shortcut)) {
                    Ok(()) => 0,
                    Err(error) => {
                        eprintln!("{SHORTCUT_SWITCH}: {error}");
                        1
                    }
                },
            };
            std::process::exit(code);
        }
        let aumid = wide(AUMID);
        // SAFETY: a NUL-terminated string.
        let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(aumid.as_ptr()) };
        if hr < 0 {
            tracing::warn!("AppUserModelID not set: {hr:#010x}");
        }
        // The end-to-end tests' runs (PPVPN_FLUTTER_HOME) leave the
        // installed app's registration alone.
        if crate::dirs::home_override().is_none() {
            register();
        }
    }

    fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
        text.as_ref().encode_wide().chain(Some(0)).collect()
    }

    /// The registration the native app writes at every start: notifications
    /// show "PPVPN" with its icon, and a click starts this executable.
    fn register() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let aumid_key = format!(r"Software\Classes\AppUserModelId\{AUMID}");
        let server_key = format!(r"Software\Classes\CLSID\{{{ACTIVATOR_CLSID}}}\LocalServer32");
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
                format!("\"{}\" {TOAST_ACTIVATED_SWITCH}", exe.display()),
            ),
        ];
        // Only an existing file: the notification platform caches the icon
        // per AUMID, and a missing one stays blank. The installer stages
        // the native app's (apps/flutter/scripts/package-windows.ps1).
        if let Some(icon) = exe
            .parent()
            .map(|dir| dir.join("Assets").join("AppLogo.png"))
            .filter(|icon| icon.is_file())
        {
            values.push((aumid_key.as_str(), "IconUri", icon.display().to_string()));
        }
        for (key, name, value) in values {
            let (key, name, value) = (wide(key), wide(name), wide(&value));
            let Ok(size) = u32::try_from(value.len() * 2) else {
                return;
            };
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
            if status != ERROR_SUCCESS {
                tracing::warn!(
                    "notification registration failed: {}",
                    std::io::Error::from_raw_os_error(status as i32)
                );
                return;
            }
        }
    }

    /// Sets System.AppUserModel.ID on the shortcut, on a thread of its own
    /// in a single-threaded apartment, as the shell's link object needs.
    fn stamp(shortcut: &Path) -> windows::core::Result<()> {
        let shortcut = wide(shortcut);
        std::thread::spawn(move || {
            // SAFETY: COM is initialised for this thread alone and
            // uninitialised after the interfaces are gone.
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
                let result = stamp_in_apartment(&shortcut);
                CoUninitialize();
                result
            }
        })
        .join()
        .unwrap_or_else(|_| Err(E_FAIL.into()))
    }

    /// `shortcut` is NUL-terminated.
    unsafe fn stamp_in_apartment(shortcut: &[u16]) -> windows::core::Result<()> {
        let mut aumid = wide(AUMID);
        // SAFETY: COM is initialised on this thread; `shortcut` and `aumid`
        // are NUL-terminated and outlive the calls. The PROPVARIANT only
        // borrows `aumid` (it is never cleared).
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let file: IPersistFile = link.cast()?;
            file.Load(PCWSTR(shortcut.as_ptr()), STGM_READWRITE)?;
            let store: IPropertyStore = link.cast()?;
            let mut value = PROPVARIANT::default();
            {
                let inner = &mut *value.Anonymous.Anonymous;
                inner.vt = VT_LPWSTR;
                inner.Anonymous.pwszVal = PWSTR(aumid.as_mut_ptr());
            }
            store.SetValue(&APP_USER_MODEL_ID, &value)?;
            store.Commit()?;
            file.Save(PCWSTR(shortcut.as_ptr()), true)?;
        }
        Ok(())
    }
}
