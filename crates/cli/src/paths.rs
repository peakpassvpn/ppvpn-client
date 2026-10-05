//! Where the CLI keeps its files.
//!
//! Everything must survive reboots and logouts: core's `state_dir` holds the
//! local proxy's username prefix, password and port, which users copy into
//! other applications. On Linux that rules out `XDG_RUNTIME_DIR`, a tmpfs
//! that is cleared on reboot and may be removed when the user's last session
//! ends.
//!
//! | | macOS | Linux |
//! | --- | --- | --- |
//! | settings | `~/Library/Application Support/ppvpn-cli/settings.json` | `$XDG_CONFIG_HOME/ppvpn-cli/settings.json` |
//! | runtime (socket, process record, logs) | `~/Library/Application Support/ppvpn-cli/runtime/` | `$XDG_STATE_HOME/ppvpn-cli/runtime/` |
//! | core state | `~/Library/Application Support/ppvpn-cli/state/` | `$XDG_STATE_HOME/ppvpn-cli/state/` |
//!
//! XDG variables that are unset or relative fall back to `~/.config` and
//! `~/.local/state`. The CLI never shares a directory with the desktop app.

use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::env::{Env, Os};
use crate::error::{CliError, Result};

pub const APP_DIR: &str = "ppvpn-cli";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub settings: PathBuf,
    pub runtime_dir: PathBuf,
    /// The daemon's control socket. In `runtime_dir` unless that path is too
    /// long for a Unix socket address, then in `socket_dir`.
    pub socket: PathBuf,
    pub socket_dir: PathBuf,
    pub process_record: PathBuf,
    pub lock: PathBuf,
    /// Shared secret every control request carries (0600).
    pub secret: PathBuf,
    /// The background daemon's stdout and stderr.
    pub daemon_log: PathBuf,
    /// Core's and sail's log lines (0600); the previous one is `core.log.1`.
    pub core_log: PathBuf,
    /// Core's `state_dir`.
    pub state_dir: PathBuf,
}

impl Paths {
    pub fn resolve(env: &Env) -> Result<Paths> {
        let home = env
            .var("HOME")
            .filter(|h| h.starts_with('/'))
            .map(PathBuf::from)
            .ok_or_else(|| {
                CliError::environment("HOME_UNAVAILABLE", "HOME is not set to an absolute path")
            })?;
        let xdg = |name: &str, fallback: &[&str]| -> PathBuf {
            match env.var(name).filter(|v| v.starts_with('/')) {
                Some(dir) => PathBuf::from(dir),
                None => fallback.iter().fold(home.clone(), |p, c| p.join(c)),
            }
        };
        let (config_dir, base) = match env.os {
            Os::Macos => {
                let dir = home.join("Library/Application Support").join(APP_DIR);
                (dir.clone(), dir)
            }
            Os::Linux => (
                xdg("XDG_CONFIG_HOME", &[".config"]).join(APP_DIR),
                xdg("XDG_STATE_HOME", &[".local", "state"]).join(APP_DIR),
            ),
        };
        Ok(Paths::from_dirs(
            config_dir,
            base.join("runtime"),
            base.join("state"),
            env,
        ))
    }

    pub fn from_dirs(
        config_dir: PathBuf,
        runtime_dir: PathBuf,
        state_dir: PathBuf,
        env: &Env,
    ) -> Paths {
        let mut socket_dir = runtime_dir.clone();
        let mut socket = runtime_dir.join("ctl.sock");
        if socket.as_os_str().len() > max_socket_path(env.os) {
            // A hash of the runtime directory keeps the name stable and
            // distinct per directory; the parent is checked as private.
            socket_dir = temp_dir(env).join(format!("{APP_DIR}-{}", unsafe { libc::getuid() }));
            socket = socket_dir.join(format!(
                "{:016x}.sock",
                fnv1a(runtime_dir.as_os_str().as_encoded_bytes())
            ));
        }
        Paths {
            settings: config_dir.join("settings.json"),
            config_dir,
            process_record: runtime_dir.join("daemon.json"),
            lock: runtime_dir.join("daemon.lock"),
            secret: runtime_dir.join("session.secret"),
            daemon_log: runtime_dir.join("daemon.log"),
            core_log: runtime_dir.join("core.log"),
            socket,
            socket_dir,
            runtime_dir,
            state_dir,
        }
    }

    /// Creates the runtime, state and socket directories as private (0700)
    /// directories owned by the current user.
    pub fn ensure(&self) -> Result<()> {
        let fail = |dir: &Path, err: io::Error| {
            CliError::environment(
                "RUNTIME_DIRECTORY_UNAVAILABLE",
                format!("cannot prepare {}: {err}", dir.display()),
            )
        };
        if self.socket.as_os_str().len() > max_socket_path(Os::current()) {
            return Err(CliError::environment(
                "RUNTIME_DIRECTORY_UNAVAILABLE",
                "the control socket path is too long",
            ));
        }
        for dir in [&self.runtime_dir, &self.state_dir] {
            ensure_private_dir(dir, true).map_err(|e| fail(dir, e))?;
        }
        if self.socket_dir != self.runtime_dir {
            // A shared parent such as /tmp: never create parents, never adopt a
            // directory that another user owns or that is a symlink.
            ensure_private_dir(&self.socket_dir, false).map_err(|e| fail(&self.socket_dir, e))?;
        }
        Ok(())
    }
}

/// `sun_path` holds 104 bytes on macOS and 108 on Linux, NUL included.
fn max_socket_path(os: Os) -> usize {
    match os {
        Os::Macos => 103,
        Os::Linux => 107,
    }
}

fn temp_dir(env: &Env) -> PathBuf {
    env.var("TMPDIR")
        .filter(|d| d.starts_with('/'))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// FNV-1a: a small stable hash, enough to name a socket after a directory.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Creates `dir` with mode 0700, or checks that an existing one is a real
/// directory owned by the current user, and tightens its mode to 0700.
pub fn ensure_private_dir(dir: &Path, create_parents: bool) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(io::Error::other("not a private directory"));
            }
            if meta.uid() != unsafe { libc::getuid() } {
                return Err(io::Error::other("owned by another user"));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.recursive(create_parents);
            match builder.create(dir) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    return ensure_private_dir(dir, false)
                }
                Err(err) => return Err(err),
            }
        }
        Err(err) => return Err(err),
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Writes `data` to `path` atomically with mode 0600.
pub fn write_private_file(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("no parent directory"))?;
    let mut tmp = tempfile_in(dir)?;
    tmp.1.write_all(data)?;
    tmp.1.sync_all()?;
    fs::rename(&tmp.0, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp.0);
    })
}

fn tempfile_in(dir: &Path) -> io::Result<(PathBuf, fs::File)> {
    use std::os::unix::fs::OpenOptionsExt;
    for attempt in 0..16u32 {
        let name = dir.join(format!(".tmp-{}-{attempt}", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&name)
        {
            Ok(file) => return Ok((name, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other("cannot create a temporary file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Env, Os};

    #[test]
    fn linux_paths_are_persistent_and_follow_xdg() {
        let env = Env::with_vars(
            Os::Linux,
            &[
                ("HOME", "/srv/u"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
                ("XDG_STATE_HOME", "/srv/state"),
                ("XDG_CONFIG_HOME", "/srv/config"),
            ],
        );
        let paths = Paths::resolve(&env).unwrap();
        assert_eq!(
            paths.settings,
            Path::new("/srv/config/ppvpn-cli/settings.json")
        );
        assert_eq!(paths.runtime_dir, Path::new("/srv/state/ppvpn-cli/runtime"));
        assert_eq!(paths.state_dir, Path::new("/srv/state/ppvpn-cli/state"));
        assert_eq!(
            paths.socket,
            Path::new("/srv/state/ppvpn-cli/runtime/ctl.sock")
        );
        for path in [&paths.runtime_dir, &paths.state_dir, &paths.socket] {
            assert!(
                !path.starts_with("/run/user"),
                "{} is on the volatile XDG_RUNTIME_DIR",
                path.display()
            );
        }
    }

    #[test]
    fn unset_or_relative_xdg_falls_back_to_home() {
        for state in [None, Some("relative/state")] {
            let mut vars = vec![("HOME", "/srv/u")];
            if let Some(state) = state {
                vars.push(("XDG_STATE_HOME", state));
                vars.push(("XDG_CONFIG_HOME", state));
            }
            let paths = Paths::resolve(&Env::with_vars(Os::Linux, &vars)).unwrap();
            assert_eq!(
                paths.runtime_dir,
                Path::new("/srv/u/.local/state/ppvpn-cli/runtime")
            );
            assert_eq!(
                paths.settings,
                Path::new("/srv/u/.config/ppvpn-cli/settings.json")
            );
        }
    }

    #[test]
    fn macos_keeps_everything_in_application_support() {
        let paths = Paths::resolve(&Env::with_vars(Os::Macos, &[("HOME", "/srv/u")])).unwrap();
        let base = Path::new("/srv/u/Library/Application Support/ppvpn-cli");
        assert_eq!(paths.settings, base.join("settings.json"));
        assert_eq!(paths.runtime_dir, base.join("runtime"));
        assert_eq!(paths.state_dir, base.join("state"));
    }

    #[test]
    fn a_missing_home_is_an_environment_error() {
        let err = Paths::resolve(&Env::with_vars(Os::Linux, &[])).unwrap_err();
        assert_eq!(err.exit_code(), 8);
    }

    #[test]
    fn long_runtime_directories_get_a_short_private_socket() {
        let base = tempfile::tempdir().unwrap();
        let long = base.path().join("d".repeat(120));
        let env = Env::with_vars(Os::current(), &[("TMPDIR", base.path().to_str().unwrap())]);
        let paths = Paths::from_dirs(
            long.join("config"),
            long.join("runtime"),
            long.join("state"),
            &env,
        );
        assert_ne!(paths.socket_dir, paths.runtime_dir);
        assert!(paths.socket.as_os_str().len() <= max_socket_path(Os::current()));
        assert_eq!(
            paths,
            Paths::from_dirs(
                long.join("config"),
                long.join("runtime"),
                long.join("state"),
                &env
            ),
            "socket path must be stable"
        );
        paths.ensure().unwrap();
        let meta = fs::symlink_metadata(&paths.socket_dir).unwrap();
        assert!(meta.is_dir());
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn private_dirs_reject_symlinks_and_files() {
        let base = tempfile::tempdir().unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(base.path(), &link).unwrap();
        assert!(ensure_private_dir(&link, false).is_err());
        let file = base.path().join("file");
        fs::write(&file, b"x").unwrap();
        assert!(ensure_private_dir(&file, false).is_err());
        let fresh = base.path().join("a/b");
        assert!(
            ensure_private_dir(&fresh, false).is_err(),
            "must not create parents when asked not to"
        );
        ensure_private_dir(&fresh, true).unwrap();
        assert_eq!(
            fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn private_files_are_0600_and_replace_atomically() {
        let base = tempfile::tempdir().unwrap();
        let path = base.path().join("settings.json");
        write_private_file(&path, b"one").unwrap();
        write_private_file(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_dir(base.path()).unwrap().count(),
            1,
            "no temporary files left behind"
        );
    }
}
