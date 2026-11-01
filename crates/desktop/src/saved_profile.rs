//! The last proxy profile a core accepted, kept across restarts (#532): a
//! client launched while the backend serves a profile its core rejects, or
//! while the backend is unreachable, still has a profile to run.
//!
//! It carries node credentials, so it is written like the other secrets in
//! the data directory ([`crate::device::write_private`]: 0600 on Unix, the
//! profile directory's ACL on Windows). It belongs to the saved login and
//! its team: the session drops it at sign-out, at a sign-in (another
//! account may follow) and at a team switch. A file saved against another
//! API base is ignored, since the core pins rule sets to that host.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::storage::SAVED_PROFILE_FILE;

#[derive(Serialize, Deserialize)]
struct SavedFile {
    version: u32,
    api_base: String,
    revision: String,
    /// The profile document as downloaded.
    profile: String,
}

/// A profile a core accepted, as the session keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavedProfile {
    pub(crate) revision: String,
    pub(crate) raw: std::sync::Arc<Vec<u8>>,
}

/// The profile saved for `api_base`, if any.
pub(crate) fn load(data_dir: &str, api_base: &str) -> Option<SavedProfile> {
    let path = Path::new(data_dir).join(SAVED_PROFILE_FILE);
    let raw = std::fs::read(&path).ok()?;
    let file = match serde_json::from_slice::<SavedFile>(&raw) {
        Ok(file) if file.version == 1 => file,
        _ => {
            tracing::warn!("saved profile: unreadable, removed");
            remove(data_dir);
            return None;
        }
    };
    if file.api_base != api_base {
        tracing::info!("saved profile: from another backend, removed");
        remove(data_dir);
        return None;
    }
    Some(SavedProfile {
        revision: file.revision,
        raw: std::sync::Arc::new(file.profile.into_bytes()),
    })
}

pub(crate) fn save(data_dir: &str, api_base: &str, profile: &SavedProfile) {
    let Ok(text) = std::str::from_utf8(&profile.raw) else {
        return;
    };
    let file = SavedFile {
        version: 1,
        api_base: api_base.to_string(),
        revision: profile.revision.clone(),
        profile: text.to_string(),
    };
    let result = serde_json::to_vec(&file)
        .map_err(|error| error.to_string())
        .and_then(|raw| {
            crate::device::write_private(&Path::new(data_dir).join(SAVED_PROFILE_FILE), &raw)
        });
    match result {
        Ok(()) => tracing::info!("saved profile: revision {}", profile.revision),
        Err(detail) => tracing::warn!("saved profile: {detail}"),
    }
}

pub(crate) fn remove(data_dir: &str) {
    match std::fs::remove_file(Path::new(data_dir).join(SAVED_PROFILE_FILE)) {
        Ok(()) => tracing::info!("saved profile: removed"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!("saved profile: remove: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_profile_loads_only_for_its_backend() {
        let dir = crate::test_backend::temp_dir("ppvpn-saved-profile");
        let profile = SavedProfile {
            revision: "r1".into(),
            raw: std::sync::Arc::new(br#"{"revision":"r1"}"#.to_vec()),
        };
        save(&dir, "https://www.example.com", &profile);
        assert_eq!(load(&dir, "https://www.example.com"), Some(profile.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Path::new(&dir).join(SAVED_PROFILE_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Another backend: ignored and dropped.
        assert_eq!(load(&dir, "https://www.example.net"), None);
        assert!(!Path::new(&dir).join(SAVED_PROFILE_FILE).exists());

        save(&dir, "https://www.example.com", &profile);
        remove(&dir);
        assert_eq!(load(&dir, "https://www.example.com"), None);
        remove(&dir);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unreadable_file_is_dropped() {
        let dir = crate::test_backend::temp_dir("ppvpn-saved-profile");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Path::new(&dir).join(SAVED_PROFILE_FILE), b"not json").unwrap();
        assert_eq!(load(&dir, "https://www.example.com"), None);
        assert!(!Path::new(&dir).join(SAVED_PROFILE_FILE).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
