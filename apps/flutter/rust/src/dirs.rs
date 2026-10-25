//! Where the app keeps its files. Also compiled into the push agent
//! (apps/flutter/push-agent), which must read the `push-agent.json` the
//! client writes here: keep it to std.

/// `PPVPN_FLUTTER_HOME`, when set: the end-to-end tests' own directory for
/// the data and logs. With it the app also keeps its sign-in and settings
/// in plain files there (hooks.rs, settings.rs), away from the keyring and
/// the native apps' settings.
pub(crate) fn home_override() -> Option<String> {
    std::env::var("PPVPN_FLUTTER_HOME")
        .ok()
        .filter(|value| !value.is_empty())
}

/// Android and iOS: the app's own directory, which only the native side
/// knows there (main.dart asks it); `api::set_app_dir` sets it before
/// `start`.
pub(crate) static APP_DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// `$XDG_*_HOME` when set to an absolute path, else `~/<fallback>`, as the
/// native Linux app reads them (apps/linux LinuxPaths).
pub(crate) fn xdg(variable: &str, fallback: &str) -> Result<String, String> {
    match std::env::var(variable) {
        Ok(value) if value.starts_with('/') => Ok(value),
        _ => {
            let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
            Ok(format!("{home}/{fallback}"))
        }
    }
}

/// Per-user directories: (data, logs). They are the native apps' own, so
/// that this app takes over their sign-in, node and connection settings in
/// place, and the native app finds them again after a rollback:
/// - macOS: `~/Library/Application Support/PPVPN` (0700) and
///   `~/Library/Logs/PPVPN` (apps/macos AppDirectories);
/// - Windows: `%LOCALAPPDATA%\PPVPN` and its `logs` (apps/windows AppPaths);
/// - Linux: `$XDG_DATA_HOME/ppvpn` and `$XDG_STATE_HOME/ppvpn/logs`
///   (apps/linux LinuxPaths);
/// - Android and iOS: `data` and `logs` under [`APP_DIR`] (#448).
///
/// [`home_override`] puts both under one directory (the end-to-end test:
/// HOME and LOCALAPPDATA also hold Flutter's own caches, so it cannot move
/// those).
pub(crate) fn dirs() -> Result<(String, String), String> {
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let (data, logs) = if let Some(base) = home_override() {
        (format!("{base}/data"), format!("{base}/logs"))
    } else if cfg!(any(target_os = "android", target_os = "ios")) {
        let base = APP_DIR.get().ok_or("the app directory is not set")?;
        (format!("{base}/data"), format!("{base}/logs"))
    } else if cfg!(windows) {
        let base = env("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")? + "\\PPVPN";
        let logs = format!("{base}\\logs");
        (base, logs)
    } else if cfg!(target_os = "macos") {
        let home = env("HOME").ok_or("HOME is not set")?;
        (
            format!("{home}/Library/Application Support/PPVPN"),
            format!("{home}/Library/Logs/PPVPN"),
        )
    } else {
        (
            format!("{}/ppvpn", xdg("XDG_DATA_HOME", ".local/share")?),
            format!("{}/ppvpn/logs", xdg("XDG_STATE_HOME", ".local/state")?),
        )
    };
    for dir in [&data, &logs] {
        std::fs::create_dir_all(dir).map_err(|error| format!("{dir}: {error}"))?;
    }
    // The data directory holds the sign-in and the push token: private to
    // the user, as the native macOS app keeps it (and as it does, a failure
    // here is not fatal).
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700));
    }
    Ok((data, logs))
}
