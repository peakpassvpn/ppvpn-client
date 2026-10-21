//! The platform hooks, in Rust for the prototype: the client calls them from
//! its own threads, where a call into Dart would have to wait for the UI
//! isolate. The saved sign-in is the native apps' own (credentials.rs).
//!
//! The privileged service (enhanced mode) is the native apps' own: the same
//! paths, helpers and admin prompts as apps/linux LinuxPlatformHooks,
//! apps/macos MacPlatformHooks and apps/windows PrivilegedService, so that
//! the service (crates/service) accepts this app as it does those. The
//! helpers sit next to the app's executable, as in the native packages.

use std::path::{Path, PathBuf};
use std::process::Command;

use ppvpn_client::{PlatformError, PlatformHooks};

use crate::credentials;

pub(crate) struct Hooks {
    credentials: credentials::Store,
}

impl Hooks {
    pub(crate) fn new(data_dir: &str) -> Self {
        Self {
            credentials: credentials::Store::new(data_dir),
        }
    }
}

fn failure(message: impl Into<String>) -> PlatformError {
    PlatformError::Failed {
        message: message.into(),
    }
}

/// The directory of the running executable, where the packages put the
/// service helpers (macOS: the bundle's Contents/MacOS).
fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(Path::to_path_buf)
}

impl PlatformHooks for Hooks {
    fn credential_load(&self) -> Result<Option<Vec<u8>>, PlatformError> {
        self.credentials.load()
    }

    fn credential_save(&self, blob: Vec<u8>) -> Result<(), PlatformError> {
        self.credentials.save(blob)
    }

    fn credential_delete(&self) -> Result<(), PlatformError> {
        self.credentials.delete()
    }

    fn open_url(&self, url: String) -> bool {
        open_url(&url)
    }

    fn privileged_service_installed(&self) -> bool {
        service::installed()
    }

    fn install_privileged_service(&self) -> Result<(), PlatformError> {
        service::install()
    }

    fn uninstall_privileged_service(&self) -> Result<(), PlatformError> {
        service::uninstall()
    }
}

pub(crate) fn open_url(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        Command::new("xdg-open")
    };
    command.arg(url).spawn().is_ok()
}

/// Linux: a systemd unit, installed by a helper run as root through pkexec
/// (the polkit password prompt). Paths are owned by service/src/install.rs.
#[cfg(target_os = "linux")]
mod service {
    use std::path::Path;
    use std::process::{Command, Stdio};

    use ppvpn_client::PlatformError;

    use super::failure;

    const UNIT_PATH: &str = "/etc/systemd/system/ppvpn-service.service";
    const SERVICE_BINARY: &str = "/usr/lib/ppvpn-service/ppvpn-service";

    // pkexec exit codes: 126 = the authentication dialog was dismissed,
    // 127 = not authorized or no polkit agent is running.
    const PKEXEC_DISMISSED: i32 = 126;
    const PKEXEC_NOT_AUTHORIZED: i32 = 127;

    pub(super) fn installed() -> bool {
        Path::new(UNIT_PATH).is_file() && Path::new(SERVICE_BINARY).is_file()
    }

    pub(super) fn install() -> Result<(), PlatformError> {
        run_helper_as_admin("ppvpn-service-install")
    }

    pub(super) fn uninstall() -> Result<(), PlatformError> {
        run_helper_as_admin("ppvpn-service-uninstall")
    }

    /// Runs a helper installed next to the app as root behind the polkit
    /// password prompt.
    fn run_helper_as_admin(name: &str) -> Result<(), PlatformError> {
        let helper = super::exe_dir()
            .map(|dir| dir.join(name))
            .filter(|path| path.is_file())
            .ok_or_else(|| failure(format!("LINUX_PRIVILEGED_HELPER_MISSING:{name}")))?;
        // `output` drains stdout too, so a chatty helper cannot block on a
        // full pipe.
        let output = Command::new("pkexec")
            .arg(&helper)
            .stdin(Stdio::null())
            .output()
            .map_err(|_| failure("LINUX_PKEXEC_MISSING"))?;
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        match output.status.code() {
            Some(0) => Ok(()),
            Some(PKEXEC_DISMISSED) => Err(PlatformError::Cancelled),
            Some(PKEXEC_NOT_AUTHORIZED)
                if stderr.to_lowercase().contains("authentication agent") =>
            {
                Err(failure(format!("LINUX_POLKIT_AGENT_MISSING:{stderr}")))
            }
            Some(PKEXEC_NOT_AUTHORIZED) => Err(failure(format!(
                "LINUX_ADMIN_AUTHORIZATION_FAILED:{stderr}"
            ))),
            // The helper reports LINUX_SYSTEMD_UNAVAILABLE itself.
            _ => Err(failure(format!(
                "LINUX_SERVICE_HELPER_FAILED:{name}:{stderr}"
            ))),
        }
    }
}

/// macOS: a launch daemon, installed by a helper from the bundle's
/// Contents/MacOS run as root behind the administrator password prompt.
/// Paths are owned by service/src/install.rs.
#[cfg(target_os = "macos")]
mod service {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use ppvpn_client::PlatformError;

    use super::failure;

    const LAUNCH_DAEMON_PLIST: &str = "/Library/LaunchDaemons/com.peakpassvpn.ppvpn.service.plist";
    const SERVICE_BINARY: &str = "/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle/Contents/MacOS/ppvpn-service";

    pub(super) fn installed() -> bool {
        Path::new(LAUNCH_DAEMON_PLIST).exists() && Path::new(SERVICE_BINARY).exists()
    }

    pub(super) fn install() -> Result<(), PlatformError> {
        run_helper_as_admin("ppvpn-service-install", &["ppvpn-service"])
    }

    pub(super) fn uninstall() -> Result<(), PlatformError> {
        run_helper_as_admin("ppvpn-service-uninstall", &[])
    }

    fn is_executable(path: &Path) -> bool {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }

    /// `text` as one single-quoted word for /bin/sh.
    fn quoted(text: &str) -> String {
        format!("'{}'", text.replace('\'', "'\\''"))
    }

    /// Runs a bundled helper from Contents/MacOS as root behind the standard
    /// administrator password prompt.
    ///
    /// The bundle belongs to the user, so in a release the root shell first
    /// copies the helper (and the files it installs) into a directory only
    /// root can write, checks the copy with /usr/bin/codesign against the
    /// helper's identifier and the certificate that signed this app, and
    /// runs the copy: a helper changed on disk is never run as root.
    /// ppvpn-service-install checks the service binary the same way
    /// (crates/service/src/install.rs).
    fn run_helper_as_admin(name: &str, alongside: &[&str]) -> Result<(), PlatformError> {
        let missing = || failure(format!("MACOS_PRIVILEGED_HELPER_MISSING:{name}"));
        let directory = super::exe_dir().ok_or_else(missing)?;
        for file in std::iter::once(name).chain(alongside.iter().copied()) {
            if !is_executable(&directory.join(file)) {
                return Err(failure(format!("MACOS_PRIVILEGED_HELPER_MISSING:{file}")));
            }
        }
        // The script is AppleScript text: a path that is not UTF-8 cannot
        // be written into it.
        let path = |file: &str| {
            directory
                .join(file)
                .to_str()
                .map(quoted)
                .ok_or_else(|| failure(format!("MACOS_PRIVILEGED_HELPER_PATH:{file}")))
        };

        let script = if cfg!(debug_assertions) {
            // Ad-hoc signed: nothing to check against.
            path(name)?
        } else {
            let leaf = signing_certificate_sha1(&directory)
                .ok_or_else(|| failure("MACOS_PRIVILEGED_HELPER_UNSIGNED"))?;
            let identifier = format!("com.peakpassvpn.ppvpn.{}", name.replace("ppvpn-", ""));
            let requirement =
                format!("=identifier \"{identifier}\" and certificate leaf = H\"{leaf}\"");
            let copies = std::iter::once(name)
                .chain(alongside.iter().copied())
                .map(|file| Ok(format!("/bin/cp {} \"$d/\"", path(file)?)))
                .collect::<Result<Vec<_>, PlatformError>>()?
                .join(" && ");
            format!(
                "d=$(/usr/bin/mktemp -d /tmp/ppvpn-helper.XXXXXX) || exit 1; \
                 trap '/bin/rm -rf \"$d\"' EXIT; \
                 {copies} && /usr/bin/codesign --verify --strict --test-requirement {} \"$d/{name}\" && \"$d/{name}\"",
                quoted(&requirement)
            )
        };
        let apple_literal = format!("\"{}\"", script.replace('\\', "\\\\").replace('"', "\\\""));

        let output = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(format!(
                "do shell script {apple_literal} with administrator privileges"
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .output()
            .map_err(|error| failure(error.to_string()))?;
        if output.status.success() {
            return Ok(());
        }
        let message = String::from_utf8_lossy(&output.stderr);
        // -128 is AppleScript's userCanceledErr.
        if message.contains("-128") {
            return Err(PlatformError::Cancelled);
        }
        Err(failure(format!(
            "MACOS_ADMIN_AUTHORIZATION_FAILED:{}",
            message.trim()
        )))
    }

    /// The SHA-1 of the certificate this app is signed with, from its
    /// designated requirement (`certificate leaf = H"…"`, which
    /// package-dmg.sh pins to crates/service/src/macos_client_cert.sha1).
    /// `directory` is the bundle's Contents/MacOS.
    fn signing_certificate_sha1(directory: &Path) -> Option<String> {
        let bundle = directory.parent()?.parent()?;
        let output = Command::new("/usr/bin/codesign")
            .args(["-d", "-r-"])
            .arg(bundle)
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        // The requirement goes to stdout; older codesign versions wrote it
        // to stderr with the rest.
        let text = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        leaf_sha1(&text)
    }

    /// The 40 hex digits of the first `certificate leaf = H"…"`, lower case.
    fn leaf_sha1(designated: &str) -> Option<String> {
        const MARK: &str = "certificate leaf = H\"";
        designated.match_indices(MARK).find_map(|(at, _)| {
            let rest = &designated[at + MARK.len()..];
            let hex = rest.get(..40)?;
            (hex.bytes().all(|byte| byte.is_ascii_hexdigit()) && rest[40..].starts_with('"'))
                .then(|| hex.to_ascii_lowercase())
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{leaf_sha1, quoted};

        #[test]
        fn reads_the_leaf_from_a_designated_requirement() {
            let text = "Executable=/Applications/PPVPN.app/Contents/MacOS/PPVPN\n\
                designated => identifier \"com.peakpassvpn.ppvpn\" and anchor apple generic \
                and certificate leaf = H\"0123456789ABCDEF0123456789abcdef01234567\"\n";
            assert_eq!(
                leaf_sha1(text).as_deref(),
                Some("0123456789abcdef0123456789abcdef01234567")
            );
            assert_eq!(leaf_sha1("certificate leaf = H\"0123\""), None);
            assert_eq!(leaf_sha1("designated => anchor apple"), None);
        }

        #[test]
        fn quotes_for_the_shell() {
            assert_eq!(quoted("/Apps/P P"), "'/Apps/P P'");
            assert_eq!(quoted("/a'b"), "'/a'\\''b'");
        }
    }
}

/// Windows: a service (SCM), installed by a helper next to the app's
/// executable run through the UAC prompt. Names match service/src/install.rs
/// and installer/ppvpn.nsi.
#[cfg(windows)]
mod service {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use ppvpn_client::PlatformError;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };

    use super::failure;

    /// SERVICE_NAME in service/src/install.rs.
    const SERVICE_NAME: &str = "ppvpn_service";
    const INSTALL_HELPER: &str = "ppvpn-service-install.exe";
    const UNINSTALL_HELPER: &str = "ppvpn-service-uninstall.exe";
    /// SW_HIDE (Win32_UI_WindowsAndMessaging, not worth the feature).
    const SW_HIDE: i32 = 0;

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(Some(0)).collect()
    }

    pub(super) fn installed() -> bool {
        let name = wide(OsStr::new(SERVICE_NAME));
        // SAFETY: plain SCM calls on a NUL-terminated name; every handle
        // opened here is closed here.
        unsafe {
            let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if manager.is_null() {
                return false;
            }
            let service = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
            let found = !service.is_null();
            if found {
                CloseServiceHandle(service);
            }
            CloseServiceHandle(manager);
            found
        }
    }

    pub(super) fn install() -> Result<(), PlatformError> {
        run_elevated(INSTALL_HELPER)
    }

    pub(super) fn uninstall() -> Result<(), PlatformError> {
        run_elevated(UNINSTALL_HELPER)
    }

    /// Launches the helper next to the app's executable through the UAC
    /// prompt and waits for it. Blocks (the client calls hooks off its
    /// async threads).
    fn run_elevated(name: &str) -> Result<(), PlatformError> {
        let directory =
            super::exe_dir().ok_or_else(|| failure(format!("{name} not found next to the app")))?;
        let helper = directory.join(name);
        if !helper.is_file() {
            return Err(failure(format!(
                "{name} not found next to {}",
                directory.display()
            )));
        }
        let verb = wide(OsStr::new("runas"));
        let file = wide(helper.as_os_str());
        let dir = wide(directory.as_os_str());
        // SAFETY: SHELLEXECUTEINFOW is plain data; all-zero is its empty
        // value (null pointers and handles).
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpDirectory = dir.as_ptr();
        info.nShow = SW_HIDE;
        // SAFETY: `info` and the strings it points to outlive the call.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            // SAFETY: no other call in between.
            let error = unsafe { GetLastError() };
            if error == ERROR_CANCELLED {
                return Err(PlatformError::Cancelled);
            }
            return Err(failure(
                std::io::Error::from_raw_os_error(error as i32).to_string(),
            ));
        }
        let process = info.hProcess;
        if process.is_null() {
            return Err(failure(format!("{name} did not start")));
        }
        let mut code = 0u32;
        // SAFETY: `process` is the helper's handle (SEE_MASK_NOCLOSEPROCESS),
        // closed once here.
        let waited = unsafe {
            let waited = WaitForSingleObject(process, INFINITE) == WAIT_OBJECT_0
                && GetExitCodeProcess(process, &mut code) != 0;
            CloseHandle(process);
            waited
        };
        match (waited, code) {
            (false, _) => Err(failure(format!("{name}: waiting for it failed"))),
            (true, 0) => Ok(()),
            (true, code) => Err(failure(format!("{name} exited with {code}"))),
        }
    }
}

/// Other systems have no privileged service.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod service {
    use ppvpn_client::PlatformError;

    pub(super) fn installed() -> bool {
        false
    }

    pub(super) fn install() -> Result<(), PlatformError> {
        Err(super::failure(
            "enhanced mode is not available on this system",
        ))
    }

    pub(super) fn uninstall() -> Result<(), PlatformError> {
        Ok(())
    }
}
