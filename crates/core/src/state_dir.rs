//! `state_dir` is one instance's (host-integration.md section 3, CLI A2): an
//! exclusive lock on `<state_dir>/.lock`, taken at `Engine::new` and held
//! until `shutdown` or the last handle's drop. A second instance on the same
//! directory, in this process or another, gets `STATE_DIR_IN_USE`.
//!
//! `File::try_lock`: flock on Unix and LockFileEx on Windows, both held per
//! open file, so a second open in the same process is refused too. The lock
//! goes with the file: a process that dies releases it.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use crate::error::{codes, Error};

pub(crate) struct StateDirLock {
    _file: File,
}

impl StateDirLock {
    /// Creates `dir` (private to this account on Unix) and locks it.
    pub(crate) fn acquire(dir: &Path) -> Result<StateDirLock, Error> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(dir)
            .map_err(|e| failed("create the state directory", dir, e))?;
        let path = dir.join(".lock");
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        // Private like everything else in the directory; a lock file left
        // by an older version (0644) is made private too.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options
            .open(&path)
            .map_err(|e| failed("open the state directory's lock", &path, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| failed("make the state directory's lock private", &path, e))?;
        }
        match file.try_lock() {
            Ok(()) => Ok(StateDirLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(Error::new(
                codes::STATE_DIR_IN_USE,
                false,
                format!("{} is in use by another instance", dir.display()),
            )),
            Err(TryLockError::Error(e)) => Err(failed("lock the state directory", &path, e)),
        }
    }
}

fn failed(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(
        codes::CORE_OPERATION_FAILED,
        false,
        format!("{what} {}: {e}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ppvpn-core-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn one_holder_at_a_time() {
        let dir = dir("lock");
        let first = StateDirLock::acquire(&dir.join("nested")).expect("created and locked");
        let err = StateDirLock::acquire(&dir.join("nested"))
            .err()
            .expect("held");
        assert_eq!((err.code, err.retryable), (codes::STATE_DIR_IN_USE, false));
        drop(first);
        StateDirLock::acquire(&dir.join("nested")).expect("free once dropped");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("nested"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir = dir("lock-mode");
        let lock = dir.join(".lock");
        drop(StateDirLock::acquire(&dir).expect("created and locked"));
        assert_eq!(mode(&lock), 0o600);
        // One an older version left readable by others.
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(StateDirLock::acquire(&dir).expect("locked again"));
        assert_eq!(mode(&lock), 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
