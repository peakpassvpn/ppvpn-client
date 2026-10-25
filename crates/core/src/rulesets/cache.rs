//! Blocking cache work. Runtime configurations name immutable objects, while
//! `<id>.srs` remembers only the last activated copy for restart/stale fallback.
//! A snapshot leases its objects so another apply's cleanup cannot remove them.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::{sha256_hex, sha256_hex_of, srs, LocalCopy, RuleSet, Snapshot, MAX_SIZE};
use super::{INVALID, SHA256_MISMATCH, STORAGE_FAILED};

pub(super) struct Cache {
    dir: Option<PathBuf>,
    leased: HashMap<PathBuf, Weak<LocalCopy>>,
}

impl Cache {
    pub(super) fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            leased: HashMap::new(),
        }
    }

    pub(super) fn path(&self, id: &str) -> Option<PathBuf> {
        let name = format!("{id}.srs");
        let mut parts = Path::new(&name).components();
        match (parts.next(), parts.next()) {
            (Some(Component::Normal(_)), None) => Some(self.dir.as_ref()?.join(name)),
            _ => None,
        }
    }

    fn object_path(&self, digest: &str) -> Option<PathBuf> {
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(
            self.dir
                .as_ref()?
                .join("objects")
                .join(format!("{digest}.srs")),
        )
    }

    pub(super) fn read_local(
        &mut self,
        set: &RuleSet,
        previous: Option<Arc<LocalCopy>>,
    ) -> Option<Arc<LocalCopy>> {
        let want = sha256_hex_of(set);
        if let Some(local) = self.load_object(&want) {
            return Some(local);
        }
        // An apply in this process uses the active snapshot as its fallback.
        // On restart, the legacy cache name contains only activated bytes.
        if previous.is_some() {
            return previous;
        }
        let data = read_bounded(&self.path(&set.id)?).ok()?;
        let digest = sha256_hex(&data);
        self.install(&data, &digest).ok()
    }

    fn load_object(&mut self, digest: &str) -> Option<Arc<LocalCopy>> {
        let path = self.object_path(digest)?;
        let data = read_bounded(&path).ok()?;
        if sha256_hex(&data) != digest {
            return None;
        }
        let mirror_dns = srs::inspect(&data).ok()?;
        Some(self.lease(path, digest, mirror_dns))
    }

    fn lease(&mut self, path: PathBuf, digest: &str, mirror_dns: bool) -> Arc<LocalCopy> {
        if let Some(local) = self.leased.get(&path).and_then(Weak::upgrade) {
            return local;
        }
        let modified = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .map(DateTime::<Utc>::from);
        let local = Arc::new(LocalCopy {
            path: path.clone(),
            sha256: digest.into(),
            mirror_dns,
            modified,
        });
        self.leased.insert(path, Arc::downgrade(&local));
        local
    }

    pub(super) fn install(
        &mut self,
        body: &[u8],
        want: &str,
    ) -> Result<Arc<LocalCopy>, &'static str> {
        if body.len() > MAX_SIZE || sha256_hex(body) != want {
            return Err(SHA256_MISMATCH);
        }
        let mirror_dns = srs::inspect(body).map_err(|_| INVALID)?;
        if let Some(local) = self.load_object(want) {
            return Ok(local);
        }
        let path = self.object_path(want).ok_or(STORAGE_FAILED)?;
        // Repair the root created by older versions with default permissions.
        private_dir(self.dir.as_ref().ok_or(STORAGE_FAILED)?).map_err(|_| STORAGE_FAILED)?;
        write_atomic(&path, body).map_err(|_| STORAGE_FAILED)?;
        Ok(self.lease(path, want, mirror_dns))
    }

    /// Called only after the host accepted the configuration. Failures here
    /// affect the next restart's cache, never the running immutable object.
    pub(super) fn activate(&mut self, snapshot: &Snapshot) {
        let mut keep = HashSet::new();
        for entry in &snapshot.entries {
            let Some(path) = self.path(&entry.set.id) else {
                continue;
            };
            keep.insert(path.clone());
            let Some(local) = &entry.local else { continue };
            if read_bounded(&path).is_ok_and(|data| sha256_hex(&data) == local.sha256) {
                continue;
            }
            let result = read_bounded(&local.path).and_then(|data| write_atomic(&path, &data));
            if let Err(error) = result {
                tracing::warn!(%error, "rule set activation cache");
            }
        }
        if let Some(dir) = &self.dir {
            prune(dir, &keep);
            self.leased.retain(|_, lease| lease.strong_count() > 0);
            prune(&dir.join("objects"), &self.leased.keys().cloned().collect());
        }
    }
}

/// Check the size before allocating, and cap the read too in case the file
/// grows after metadata was read. Non-files (including symlinks) are not cache.
fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_SIZE as u64 {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    read_limited(file, MAX_SIZE)
}

fn read_limited(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut data)?;
    if data.len() > limit {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(data)
}

fn prune(dir: &Path, keep: &HashSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_ok_and(|t| !t.is_dir())
            && ((name.ends_with(".srs") && !keep.contains(&entry.path()))
                || name.starts_with(".tmp-"))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    private_dir(dir)?;
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = write_new(&tmp, data).and_then(|()| rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_new(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()
}

fn rename(from: &Path, to: &Path) -> io::Result<()> {
    for attempt in 0..10 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt == 9
                    || (error.kind() != io::ErrorKind::PermissionDenied && !cfg!(windows)) =>
            {
                return Err(error)
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    unreachable!()
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
