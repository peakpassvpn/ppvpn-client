//! Root-installed global LaunchAgent, one GUI-domain job per OS user. Uses the
//! same signed immutable helper image as the root broker, with a distinct role.
//! No UI/ApplePush callbacks or account store are introduced.
use anyhow::{anyhow, bail, Result};
use std::{
    ffi::CStr,
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
pub const LABEL: &str = "com.peakpassvpn.ppvpn.user-runtime";
pub const PLIST_PATH: &str = "/Library/LaunchAgents/com.peakpassvpn.ppvpn.user-runtime.plist";
const PLIST: &str = include_str!("user-runtime.plist");
fn private(path: &Path, uid: u32) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    // Check all existing ancestors before mkdir/chmod, including intermediate
    // symlinks. Never mutate a redirected tree from a login job.
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(m)
                if m.is_dir()
                    && !m.file_type().is_symlink()
                    && (m.uid() == uid || m.uid() == 0)
                    && m.mode() & 0o022 == 0 => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => bail!("USER_RUNTIME_DIRECTORY_UNSAFE"),
        }
    }
    if !path.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir() || m.file_type().is_symlink() || m.uid() != uid {
        bail!("USER_RUNTIME_DIRECTORY_UNSAFE");
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    super::endpoint::private_directory(path, uid)
}
pub fn run() -> Result<()> {
    super::agent::validate_installation()?;
    let uid = unsafe { libc::geteuid() };
    if uid == 0 || uid != unsafe { libc::getuid() } {
        bail!("USER_RUNTIME_ROOT_FORBIDDEN");
    }
    // Login jobs do not inherit the GUI app's environment. Derive the home from
    // the actual OS uid, not HOME, a plist expansion or an IPC-supplied path.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    let mut bytes = vec![0u8; 64 * 1024];
    if unsafe {
        libc::getpwuid_r(
            uid,
            &mut entry,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &mut found,
        )
    } != 0
        || found.is_null()
        || entry.pw_dir.is_null()
    {
        bail!("USER_RUNTIME_HOME_UNAVAILABLE");
    }
    let home = PathBuf::from(unsafe { CStr::from_ptr(entry.pw_dir) }.to_str()?);
    if !home.is_absolute() {
        bail!("USER_RUNTIME_HOME_INVALID");
    }
    let home = home.canonicalize()?;
    let data = home.join("Library/Application Support/PPVPN");
    let logs = home.join("Library/Logs/PPVPN");
    private(&data, uid)?;
    private(&logs, uid)?;
    super::agent::run(&data, &logs)
}
pub fn root_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    for p in path.ancestors() {
        let m = std::fs::symlink_metadata(p)?;
        if !m.is_dir() || m.uid() != 0 || m.mode() & 0o022 != 0 {
            bail!("USER_RUNTIME_LAUNCH_DIRECTORY_UNSAFE");
        }
    }
    Ok(())
}
fn users() -> std::collections::BTreeSet<u32> {
    let mut users = std::collections::BTreeSet::new();
    if let Ok(m) = std::fs::metadata("/dev/console") {
        if m.uid() != 0 {
            users.insert(m.uid());
        }
    }
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::new()
            .with_user(sysinfo::UpdateKind::Always)
            .with_exe(sysinfo::UpdateKind::Always)
            .with_cmd(sysinfo::UpdateKind::Always),
    );
    for process in system.processes().values() {
        // Dormant LaunchAgent jobs still belong to a GUI user with other
        // processes. Include all process owners, then query exact job labels.
        if let Some(uid) = process
            .user_id()
            .and_then(|u| u.to_string().parse::<u32>().ok())
            .filter(|u| *u != 0)
        {
            users.insert(uid);
        }
    }
    users
}
fn launchctl(args: &[&str]) -> Result<bool> {
    let mut child = {
        let _guard = super::native::spawn_guard();
        Command::new("/bin/launchctl")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?
    };
    Ok(child.wait()?.success())
}
fn user_helpers_running() -> bool {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::new()
            .with_user(sysinfo::UpdateKind::Always)
            .with_exe(sysinfo::UpdateKind::Always),
    );
    system.processes().values().any(|process| {
        process.exe() == Some(Path::new(super::broker::IMAGE))
            && process.user_id().is_some_and(|uid| uid.to_string() != "0")
    })
}
pub fn stop_users() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("USER_RUNTIME_INSTALL_ROOT_REQUIRED");
    }
    // Prevent newly created GUI domains from loading the global job while
    // the helper inode is replaced. Already loaded jobs are booted out below.
    root_directory(Path::new(PLIST_PATH).parent().unwrap())?;
    match std::fs::symlink_metadata(PLIST_PATH) {
        Ok(m) if m.is_file() && m.uid() == 0 && m.nlink() == 1 => {
            std::fs::remove_file(PLIST_PATH)?;
        }
        Ok(_) => bail!("USER_RUNTIME_LAUNCH_PLIST_UNSAFE"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    for uid in users() {
        let job = format!("gui/{uid}/{LABEL}");
        if launchctl(&["print", &job])? && !launchctl(&["bootout", &job])? {
            bail!("USER_RUNTIME_BOOTOUT_FAILED");
        }
    }
    // launchctl bootout can return before ordered core/resource shutdown ends.
    // Preserve the installed image on timeout; never kill an active core.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    while user_helpers_running() {
        if std::time::Instant::now() >= deadline {
            bail!("USER_RUNTIME_STILL_RUNNING");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
pub fn install() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("USER_RUNTIME_INSTALL_ROOT_REQUIRED");
    }
    let parent = Path::new(PLIST_PATH).parent().unwrap();
    root_directory(parent)?;
    if let Ok(m) = std::fs::symlink_metadata(PLIST_PATH) {
        if !m.is_file() || m.uid() != 0 || m.nlink() != 1 {
            bail!("USER_RUNTIME_LAUNCH_PLIST_UNSAFE");
        }
    }
    let tmp = parent.join(format!(".ppvpn-user-{}.plist", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&tmp)?;
    let write = (|| {
        file.write_all(PLIST.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, PLIST_PATH)?;
        File::open(parent)?.sync_all()?;
        Ok::<_, anyhow::Error>(())
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    write?;
    for uid in users() {
        let domain = format!("gui/{uid}");
        if launchctl(&["print", &domain])? && !launchctl(&["bootstrap", &domain, PLIST_PATH])? {
            bail!("USER_RUNTIME_BOOTSTRAP_FAILED");
        }
    }
    Ok(())
}
pub fn uninstall() -> Result<()> {
    stop_users()?;
    match std::fs::symlink_metadata(PLIST_PATH) {
        Ok(m) if m.is_file() && m.uid() == 0 && m.nlink() == 1 => {
            std::fs::remove_file(PLIST_PATH)?;
            Ok(())
        }
        Ok(_) => Err(anyhow!("USER_RUNTIME_LAUNCH_PLIST_UNSAFE")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsafe_system_ancestors_are_rejected_without_permission_repairs() {
        let mut checked = 0;
        const SYSTEM_RUN: &str = "/private/var/run"; // sensitive-check: allow — canonical macOS system directory
        for path in [SYSTEM_RUN, "/private/tmp"] {
            let path = Path::new(path);
            let before = std::fs::symlink_metadata(path).unwrap();
            if before.mode() & 0o022 == 0 {
                continue;
            }
            assert!(root_directory(path).is_err());
            let after = std::fs::symlink_metadata(path).unwrap();
            assert_eq!(
                (before.uid(), before.gid(), before.mode(), before.ino()),
                (after.uid(), after.gid(), after.mode(), after.ino())
            );
            checked += 1;
        }
        assert!(checked > 0);
    }
    #[test]
    fn root_directory_refuses_a_symlink_without_touching_its_target() {
        let root = std::env::temp_dir().join(format!("ppvpn-root-path-{}", uuid::Uuid::new_v4()));
        let target = root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        let before = std::fs::symlink_metadata(&target).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        assert!(root_directory(&alias).is_err());
        assert_eq!(
            before.mode(),
            std::fs::symlink_metadata(&target).unwrap().mode()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
