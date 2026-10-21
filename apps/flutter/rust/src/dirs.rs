//! Where the app keeps its files. Also compiled into the push agent
//! (apps/flutter/push-agent), which must read the `push-agent.json` the
//! client writes here: keep it to std.

/// Per-user directories of the prototype's own, apart from the native
/// apps': (data, logs). `PPVPN_FLUTTER_HOME` puts both under one directory
/// (the end-to-end test: HOME and LOCALAPPDATA also hold Flutter's own
/// caches, so it cannot move those).
pub(crate) fn dirs() -> Result<(String, String), String> {
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let (data, logs) = if let Some(base) = env("PPVPN_FLUTTER_HOME") {
        (format!("{base}/data"), format!("{base}/logs"))
    } else if cfg!(windows) {
        let base = env("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")? + "\\PPVPN Flutter";
        (format!("{base}\\data"), format!("{base}\\logs"))
    } else {
        let home = env("HOME").ok_or("HOME is not set")?;
        if cfg!(target_os = "macos") {
            (
                format!("{home}/Library/Application Support/PPVPN Flutter"),
                format!("{home}/Library/Logs/PPVPN Flutter"),
            )
        } else {
            let data = env("XDG_DATA_HOME").unwrap_or(format!("{home}/.local/share"));
            let state = env("XDG_STATE_HOME").unwrap_or(format!("{home}/.local/state"));
            (
                format!("{data}/ppvpn-flutter"),
                format!("{state}/ppvpn-flutter/logs"),
            )
        }
    };
    for dir in [&data, &logs] {
        std::fs::create_dir_all(dir).map_err(|error| format!("{dir}: {error}"))?;
    }
    Ok((data, logs))
}
