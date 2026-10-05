//! Pushes the OS has shown, for notification clicks.
//!
//! The push agent records every push that [`crate::PushAgentListener::on_push`]
//! accepted in `<data_dir>/push-agent-shown.json`; the main app looks one up
//! with [`crate::Client::shown_push`] when the user clicks its notification.
//! The notification itself carries only the push id, so nothing it carries is
//! ever trusted as a link.
//!
//! File format (JSON, 0600 on Unix, replaced atomically with a temp file +
//! rename), oldest first:
//!
//! ```json
//! {"items": [{"id": 17, "message_id": 4711, "title": "…", "body": "…",
//!             "severity": "critical", "category": "billing",
//!             "event_key": "invoice.issued",
//!             "deep_link": "https://www.peakpassvpn.com/app/…",
//!             "created_at": "2026-09-29T00:00:00Z", "shown_at": 1790640000}]}
//! ```
//!
//! `id` is the push queue id, `message_id` the inbox message (absent for
//! pushes without one, e.g. broadcasts), `shown_at` Unix seconds. At most
//! [`MAX_ENTRIES`] entries are kept, none older than [`TTL_SECS`] (by
//! `shown_at`); showing the same id again replaces its entry. A missing or
//! corrupt file reads as empty, and an entry that does not parse is skipped.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PushMessage;

pub(crate) const SHOWN_FILE: &str = "push-agent-shown.json";
/// Entries kept at most (the newest).
pub(crate) const MAX_ENTRIES: usize = 50;
/// Entries older than this are dropped.
pub(crate) const TTL_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Entry {
    #[serde(flatten)]
    message: PushMessage,
    shown_at: u64,
}

#[derive(Serialize)]
struct FileOut<'a> {
    items: &'a [Entry],
}

#[derive(Deserialize)]
struct FileIn {
    #[serde(default)]
    items: Vec<Value>,
}

pub(crate) fn shown_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SHOWN_FILE)
}

/// Live entries, oldest first; never fails.
fn load(path: &Path, now: u64) -> Vec<Entry> {
    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_slice::<FileIn>(&raw) else {
        tracing::info!("{} unreadable; starting empty", path.display());
        return Vec::new();
    };
    file.items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<Entry>(item).ok())
        .filter(|entry| live(entry, now))
        .collect()
}

fn live(entry: &Entry, now: u64) -> bool {
    now.saturating_sub(entry.shown_at) < TTL_SECS
}

/// Records `message` as shown at `now` (replacing an entry with the same id),
/// then trims to the TTL and the cap.
pub(crate) fn record_at(data_dir: &Path, message: &PushMessage, now: u64) -> Result<(), String> {
    let path = shown_path(data_dir);
    let mut entries = load(&path, now);
    entries.retain(|entry| entry.message.id != message.id);
    entries.push(Entry {
        message: message.clone(),
        shown_at: now,
    });
    if entries.len() > MAX_ENTRIES {
        entries.drain(..entries.len() - MAX_ENTRIES);
    }
    let raw = serde_json::to_vec(&FileOut { items: &entries }).map_err(|e| e.to_string())?;
    crate::device::write_private(&path, &raw)
}

pub(crate) fn record(data_dir: &Path, message: &PushMessage) -> Result<(), String> {
    record_at(data_dir, message, crate::push_agent::now_secs())
}

/// The shown push `push_id`, if recorded and not expired at `now`.
pub(crate) fn find_at(data_dir: &Path, push_id: u64, now: u64) -> Option<PushMessage> {
    load(&shown_path(data_dir), now)
        .into_iter()
        .rev()
        .find(|entry| entry.message.id == push_id)
        .map(|entry| entry.message)
}

pub(crate) fn find(data_dir: &Path, push_id: u64) -> Option<PushMessage> {
    find_at(data_dir, push_id, crate::push_agent::now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_backend::temp_dir;
    use crate::{MessageCategory, MessageSeverity};

    const NOW: u64 = 1_790_640_000;

    fn push(id: u64) -> PushMessage {
        PushMessage {
            id,
            message_id: id.is_multiple_of(2).then_some(id * 10),
            title: format!("p{id}"),
            body: "b".into(),
            severity: MessageSeverity::Critical,
            category: MessageCategory::Billing,
            event_key: "invoice.issued".into(),
            deep_link: Some("https://www.peakpassvpn.com/app/x".into()),
            created_at: "2026-09-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn records_round_trip_and_replace_by_id() {
        let dir = PathBuf::from(temp_dir("ppvpn-shown-test"));
        assert_eq!(find_at(&dir, 1, NOW), None, "missing file is empty");
        record_at(&dir, &push(1), NOW).unwrap();
        record_at(&dir, &push(2), NOW).unwrap();
        assert_eq!(find_at(&dir, 1, NOW), Some(push(1)));
        assert_eq!(find_at(&dir, 2, NOW).unwrap().message_id, Some(20));
        assert_eq!(find_at(&dir, 3, NOW), None);

        let mut changed = push(1);
        changed.title = "again".into();
        record_at(&dir, &changed, NOW + 1).unwrap();
        assert_eq!(find_at(&dir, 1, NOW + 1).unwrap().title, "again");
        assert_eq!(load(&shown_path(&dir), NOW + 1).len(), 2);

        // The file carries every field (and no temp files are left behind).
        let raw: Value = serde_json::from_slice(&std::fs::read(shown_path(&dir)).unwrap()).unwrap();
        let first = &raw["items"][0];
        assert_eq!(first["id"], 2);
        assert_eq!(first["message_id"], 20);
        assert_eq!(first["severity"], "critical");
        assert_eq!(first["category"], "billing");
        assert_eq!(first["shown_at"], NOW);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keeps_the_newest_fifty() {
        let dir = PathBuf::from(temp_dir("ppvpn-shown-test"));
        for id in 1..=60 {
            record_at(&dir, &push(id), NOW + id).unwrap();
        }
        let entries = load(&shown_path(&dir), NOW + 60);
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries.first().unwrap().message.id, 11);
        assert_eq!(find_at(&dir, 10, NOW + 60), None);
        assert!(find_at(&dir, 60, NOW + 60).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn drops_entries_older_than_seven_days() {
        let dir = PathBuf::from(temp_dir("ppvpn-shown-test"));
        record_at(&dir, &push(1), NOW).unwrap();
        record_at(&dir, &push(2), NOW + 3600).unwrap();
        assert!(find_at(&dir, 1, NOW + TTL_SECS - 1).is_some());
        assert_eq!(find_at(&dir, 1, NOW + TTL_SECS), None);
        assert!(find_at(&dir, 2, NOW + TTL_SECS).is_some());
        // Expired entries are dropped from the file on the next write.
        record_at(&dir, &push(3), NOW + TTL_SECS).unwrap();
        let ids: Vec<u64> = load(&shown_path(&dir), 0)
            .iter()
            .map(|e| e.message.id)
            .collect();
        assert_eq!(ids, vec![2, 3]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_file_or_entry_reads_as_empty() {
        let dir = PathBuf::from(temp_dir("ppvpn-shown-test"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(shown_path(&dir), b"{not json").unwrap();
        assert_eq!(find_at(&dir, 1, NOW), None);
        record_at(&dir, &push(1), NOW).unwrap();
        assert_eq!(find_at(&dir, 1, NOW), Some(push(1)));

        // One bad entry does not hide the others.
        let mut raw: Value =
            serde_json::from_slice(&std::fs::read(shown_path(&dir)).unwrap()).unwrap();
        raw["items"]
            .as_array_mut()
            .unwrap()
            .insert(0, serde_json::json!({"id": 9, "severity": "nope"}));
        std::fs::write(shown_path(&dir), serde_json::to_vec(&raw).unwrap()).unwrap();
        assert_eq!(find_at(&dir, 1, NOW), Some(push(1)));
        assert_eq!(find_at(&dir, 9, NOW), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = PathBuf::from(temp_dir("ppvpn-shown-test"));
        record_at(&dir, &push(1), NOW).unwrap();
        record_at(&dir, &push(2), NOW).unwrap();
        let mode = std::fs::metadata(shown_path(&dir))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }
}
