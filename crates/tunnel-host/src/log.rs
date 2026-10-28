//! The host's log files in `<state_dir>/logs`: the engine's own
//! (`ppvpn-core.log`, written by the engine) and the host's lines
//! (`tunnel-host.log`). Each is moved to `.1` at start once it passes
//! [`MAX_BYTES`], so two generations at most stay on the device.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const CORE_LOG: &str = "ppvpn-core.log";
pub(crate) const HOST_LOG: &str = "tunnel-host.log";
const MAX_BYTES: u64 = 2 << 20;

/// `path`, after moving it to `<path>.1` when it is past [`MAX_BYTES`].
pub(crate) fn rotated(path: &Path) -> PathBuf {
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(path, old);
    }
    path.to_path_buf()
}

/// The host's own lines; a line that cannot be written is dropped.
pub(crate) struct HostLog {
    file: Mutex<Option<File>>,
}

impl HostLog {
    pub(crate) fn open(path: &Path) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(rotated(path))
            .ok();
        Self {
            file: Mutex::new(file),
        }
    }

    pub(crate) fn line(&self, text: &str) {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(file) = file.as_mut() {
            let _ = writeln!(file, "{seconds} {text}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_log_moves_aside_at_open() {
        let dir = std::env::temp_dir().join(format!("ppvpn-tunnel-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(HOST_LOG);
        std::fs::write(&path, vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        let log = HostLog::open(&path);
        log.line("fresh");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with(" fresh\n"), "{text}");
        assert_eq!(
            std::fs::metadata(dir.join(format!("{HOST_LOG}.1")))
                .unwrap()
                .len(),
            MAX_BYTES + 1
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
