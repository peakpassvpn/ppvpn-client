//! Names of what the crate stores in `ClientConfig::data_dir`, and one-time
//! migrations from earlier, generic names that could collide with the apps'
//! own files (they share the directory).
//!
//! Crate-owned entries: `client-settings.json`, `enhanced-connection.json`,
//! `ppvpn-core/` (standard-core state), `installation.json`,
//! `push-agent.json`, `push-agent-cursor.json`, `push-agent.heartbeat`,
//! `selection.json`, `system-proxy-backup.json`. `settings.json`,
//! `config.json`, `state.json` and similar belong to the apps.

use std::path::Path;

/// Connection mode and other crate settings (was `settings.json`).
pub(crate) const CLIENT_SETTINGS_FILE: &str = "client-settings.json";
/// Where earlier builds kept the connection mode; owned by the apps now.
pub(crate) const LEGACY_SETTINGS_FILE: &str = "settings.json";
/// Enhanced-mode connection state (was `connection/state.json`).
pub(crate) const ENHANCED_STATE_FILE: &str = "enhanced-connection.json";
const LEGACY_ENHANCED_STATE: &str = "connection/state.json";
/// Standard-core runtime directory (was `core/`).
pub(crate) const CORE_DIR: &str = "ppvpn-core";
const LEGACY_CORE_DIR: &str = "core";

/// Moves crate-owned entries from their legacy names. Idempotent; never
/// touches `settings.json` (see [`crate::connection::load_mode`]).
pub(crate) fn migrate(data_dir: &str) {
    let dir = Path::new(data_dir);
    // connection/state.json → enhanced-connection.json
    let old_state = dir.join(LEGACY_ENHANCED_STATE);
    let new_state = dir.join(ENHANCED_STATE_FILE);
    if old_state.is_file() && !new_state.exists() {
        match std::fs::rename(&old_state, &new_state) {
            Ok(()) => {
                tracing::info!("storage: moved connection/state.json to {ENHANCED_STATE_FILE}");
                // Removes the directory only when it is now empty.
                let _ = std::fs::remove_dir(dir.join("connection"));
            }
            Err(error) => tracing::warn!("storage: migrate enhanced state: {error}"),
        }
    }
    // core/ → ppvpn-core/ (keeps the core's persisted local-proxy ports),
    // only when it is recognisably the core's state directory.
    let old_core = dir.join(LEGACY_CORE_DIR);
    let new_core = dir.join(CORE_DIR);
    let is_core_state = old_core.join("state").is_dir() || old_core.join("session.secret").exists();
    if old_core.is_dir() && is_core_state && !new_core.exists() {
        match std::fs::rename(&old_core, &new_core) {
            Ok(()) => tracing::info!("storage: moved core/ to {CORE_DIR}/"),
            Err(error) => tracing::warn!("storage: migrate core state: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_entries_move_once() {
        let dir = crate::test_backend::temp_dir("ppvpn-storage-test");
        let base = Path::new(&dir);
        std::fs::create_dir_all(base.join("connection")).unwrap();
        std::fs::write(
            base.join(LEGACY_ENHANCED_STATE),
            r#"{"generation":4,"desired":"connected"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(base.join("core/state")).unwrap();
        std::fs::write(base.join("core/state/local-proxies.json"), "{}").unwrap();

        migrate(&dir);
        assert!(base.join(ENHANCED_STATE_FILE).is_file());
        assert!(!base.join("connection").exists());
        assert!(base.join("ppvpn-core/state/local-proxies.json").is_file());
        assert!(!base.join("core").exists());

        // Idempotent; an app's own `core` directory is left alone.
        std::fs::create_dir_all(base.join("core/app-stuff")).unwrap();
        migrate(&dir);
        assert!(base.join("core/app-stuff").is_dir());
        assert_eq!(
            std::fs::read_to_string(base.join(ENHANCED_STATE_FILE)).unwrap(),
            r#"{"generation":4,"desired":"connected"}"#
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
