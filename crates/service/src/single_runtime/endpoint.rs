//! Private user socket lifecycle. No installed endpoint is touched by tests.
use anyhow::{anyhow, Result};
use std::fs::{File, OpenOptions};
use std::os::{
    fd::AsRawFd,
    unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
};
use std::path::{Path, PathBuf};
pub fn private_directory(dir: &Path, uid: u32) -> Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != uid
        || meta.mode() & 0o077 != 0
        || dir.canonicalize()? != dir
    {
        return Err(anyhow!("RUNTIME_DIRECTORY_UNSAFE"));
    }
    Ok(())
}
pub struct Endpoint {
    pub listener: UnixListener,
    path: PathBuf,
    inode: u64,
    _lock: File,
}
impl Endpoint {
    pub fn bind(dir: &Path, uid: u32) -> Result<Self> {
        if uid == 0 || uid != unsafe { libc::geteuid() } {
            return Err(anyhow!("USER_RUNTIME_ROOT_FORBIDDEN"));
        }
        private_directory(dir, uid)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir.join("runtime.lock"))?;
        let meta = lock.metadata()?;
        if !meta.is_file() || meta.uid() != uid || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
            return Err(anyhow!("RUNTIME_LOCK_UNSAFE"));
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(anyhow!("RUNTIME_ALREADY_RUNNING"));
        }
        let path = dir.join("runtime.sock");
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                if !meta.file_type().is_socket() || meta.uid() != uid {
                    return Err(anyhow!("RUNTIME_ENDPOINT_UNSAFE"));
                }
                match std::os::unix::net::UnixStream::connect(&path) {
                    Ok(_) => return Err(anyhow!("RUNTIME_ENDPOINT_ACTIVE")),
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        std::fs::remove_file(&path)?
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let inode = std::fs::symlink_metadata(&path)?.ino();
        Ok(Self {
            listener,
            path,
            inode,
            _lock: lock,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|meta| meta.file_type().is_socket() && meta.ino() == self.inode)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn dir() -> PathBuf {
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "pv-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[test]
    fn singleton_lock_prevents_unlinking_live_socket_and_drop_removes_only_its_inode() {
        let path = dir();
        let uid = unsafe { libc::geteuid() };
        let endpoint = Endpoint::bind(&path, uid).unwrap();
        assert!(Endpoint::bind(&path, uid).is_err());
        assert!(endpoint.path().exists());
        let socket = endpoint.path().to_path_buf();
        std::fs::remove_file(&socket).unwrap();
        std::fs::write(&socket, b"replacement").unwrap();
        drop(endpoint);
        assert_eq!(std::fs::read(&socket).unwrap(), b"replacement");
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn rejects_open_directory_and_lock_symlink() {
        let path = dir();
        let uid = unsafe { libc::geteuid() };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Endpoint::bind(&path, uid).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink("elsewhere", path.join("runtime.lock")).unwrap();
        assert!(Endpoint::bind(&path, uid).is_err());
        std::fs::remove_dir_all(path).unwrap();
    }
}
