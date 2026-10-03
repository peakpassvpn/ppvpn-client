//! install-service binary — installs ppvpn-service so it runs as a
//! privileged background process.
//!
//! - Windows: registers a Windows service (Service Control Manager).
//! - macOS: writes a LaunchDaemon under /Library/LaunchDaemons/ and a
//!   helper bundle under /Library/PrivilegedHelperTools/, then bootstraps it.
//! - Linux: copies the service and core into /usr/lib/ppvpn-service/ and
//!   enables a systemd unit.
//!
//! Must be run with admin/root privileges. On macOS the app shells out via
//! `osascript ... with administrator privileges`, on Linux via `pkexec`, so
//! the system password dialog appears.

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("install-service supports only Windows, macOS and Linux");
    std::process::exit(2);
}

// ───────────────────────────── Windows ─────────────────────────────

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    use std::env;
    use std::ffi::{OsStr, OsString};
    use windows_service::{
        service::{
            ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
            ServiceType,
        },
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    const SERVICE_NAME: &str = "ppvpn_service";
    const DISPLAY_NAME: &str = "PPVPN Service";
    const DESCRIPTION: &str = "Runs PPVPN transparent routing components";

    let mgr_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let mgr = ServiceManager::local_computer(None::<&str>, mgr_access)?;

    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP;
    if let Ok(existing) = mgr.open_service(SERVICE_NAME, access) {
        if let Ok(status) = existing.query_status() {
            match status.current_state {
                // The app reinstalls a service whose build differs from its
                // own: the binary on disk (next to the app, replaced by the
                // installer) is current, so a restart loads it. Stop runs
                // the ordered data-plane shutdown (service/src/main.rs).
                ServiceState::Running | ServiceState::StartPending => {
                    // The stop handler exits the process instead of
                    // reporting STOPPED, so the call itself may report an
                    // error; the state is what counts.
                    let _ = existing.stop();
                    wait_for_service_state(
                        &existing,
                        ServiceState::Stopped,
                        std::time::Duration::from_secs(40),
                    )?;
                    existing.start(&Vec::<&OsStr>::new())?;
                }
                ServiceState::Stopped => {
                    existing.start(&Vec::<&OsStr>::new())?;
                }
                ServiceState::StopPending | ServiceState::PausePending => {
                    wait_for_service_state(
                        &existing,
                        ServiceState::Stopped,
                        std::time::Duration::from_secs(20),
                    )?;
                    existing.start(&Vec::<&OsStr>::new())?;
                }
                ServiceState::Paused => {
                    existing.start(&Vec::<&OsStr>::new())?;
                }
                _ => {}
            }
            println!("service already installed, (re)started");
            return Ok(());
        }
    }

    let exe = env::current_exe().expect("current_exe");
    let dir = exe.parent().expect("current_exe parent");
    let bin = [
        dir.join("ppvpn-service.exe"),
        dir.join("ppvpn-service-x86_64-pc-windows-msvc.exe"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .unwrap_or_else(|| {
        eprintln!("ppvpn-service.exe not found next to {}", exe.display());
        std::process::exit(2);
    });

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: bin,
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };

    let access = ServiceAccess::CHANGE_CONFIG | ServiceAccess::START;
    let svc = mgr.create_service(&info, access)?;
    svc.set_description(DESCRIPTION)?;
    svc.start(&Vec::<&OsStr>::new())?;

    println!("installed and started {SERVICE_NAME}");
    Ok(())
}

#[cfg(windows)]
fn wait_for_service_state(
    service: &windows_service::service::Service,
    target: windows_service::service::ServiceState,
    timeout: std::time::Duration,
) -> windows_service::Result<()> {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        let status = service.query_status()?;
        if status.current_state == target {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    service.query_status().map(|_| ())
}

// ───────────────────────────── macOS ─────────────────────────────

#[cfg(target_os = "macos")]
const SERVICE_LABEL: &str = "com.peakpassvpn.ppvpn.service";

#[cfg(target_os = "macos")]
const BUNDLE_PATH: &str = "/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle";

#[cfg(target_os = "macos")]
const LAUNCHD_PLIST_PATH: &str = "/Library/LaunchDaemons/com.peakpassvpn.ppvpn.service.plist";

/// Logs of the service and its core (see service/src/logfile.rs).
#[cfg(target_os = "macos")]
const LOG_DIR: &str = "/Library/Logs/PPVPN";

#[cfg(target_os = "macos")]
const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>com.peakpassvpn.ppvpn.service</string>
    <key>CFBundleName</key>
    <string>ppvpn-service</string>
    <key>CFBundleExecutable</key>
    <string>ppvpn-service</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleVersion</key>
    <string>1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
</dict>
</plist>
"#;

#[cfg(target_os = "macos")]
const LAUNCHD_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.peakpassvpn.ppvpn.service</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle/Contents/MacOS/ppvpn-service</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>ThrottleInterval</key>
    <integer>5</integer>
    <key>ExitTimeOut</key>
    <integer>30</integer>
    <key>Umask</key>
    <integer>63</integer>
    <key>StandardOutPath</key>
    <string>/Library/Logs/PPVPN/ppvpn-service.out.log</string>
    <key>StandardErrorPath</key>
    <string>/Library/Logs/PPVPN/ppvpn-service.err.log</string>
</dict>
</plist>
"#;

#[cfg(target_os = "macos")]
fn main() -> Result<(), anyhow::Error> {
    use anyhow::{anyhow, Context};
    use std::env;
    use std::fs;
    use std::path::Path;

    if !is_root() {
        return Err(anyhow!(
            "must run as root (invoke via `sudo` or `osascript with administrator privileges`)"
        ));
    }

    let exe = env::current_exe().context("current_exe")?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("current_exe has no parent"))?;
    let src_bin = [
        dir.join("ppvpn-service"),
        dir.join("ppvpn-service-aarch64-apple-darwin"),
        dir.join("ppvpn-service-x86_64-apple-darwin"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .ok_or_else(|| anyhow!("ppvpn-service binary not found next to {}", exe.display()))?;

    let src_core = [
        dir.join("ppvpn-core"),
        dir.join("ppvpn-core-aarch64-apple-darwin"),
        dir.join("ppvpn-core-x86_64-apple-darwin"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .ok_or_else(|| anyhow!("ppvpn-core binary not found next to {}", exe.display()))?;

    let macos_dir = format!("{BUNDLE_PATH}/Contents/MacOS");
    let target_bin = format!("{macos_dir}/ppvpn-service");
    let target_core = format!("{macos_dir}/ppvpn-core");
    let info_plist = format!("{BUNDLE_PATH}/Contents/Info.plist");

    // Stop the running service first: on SIGTERM it stops ppvpn-core in
    // order (routes, DNS), which takes up to ~17 s (ExitTimeOut is 30 s).
    stop_launchd_job()?;

    // Never rewrite a binary in place: a process still running it (a core
    // orphaned by an earlier crash) would fail its code signature check and
    // be SIGKILLed with its routes up. A new inode leaves it untouched.
    fs::create_dir_all(&macos_dir).with_context(|| format!("mkdir {macos_dir}"))?;
    replace_file(&src_bin, Path::new(&target_bin), 0o544)?;
    replace_file(&src_core, Path::new(&target_core), 0o544)?;
    fs::write(&info_plist, INFO_PLIST).with_context(|| format!("write {info_plist}"))?;

    // The service's and the core's logs (and launchd's stdout / stderr
    // files) live here, outside the helper bundle, so they survive
    // uninstalling and reinstalling the service.
    fs::create_dir_all(LOG_DIR).with_context(|| format!("mkdir {LOG_DIR}"))?;
    run("chown", &["root:wheel", LOG_DIR])?;
    run("chmod", &["755", LOG_DIR])?;

    if let Some(parent) = Path::new(LAUNCHD_PLIST_PATH).parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(LAUNCHD_PLIST_PATH, LAUNCHD_PLIST)
        .with_context(|| format!("write {LAUNCHD_PLIST_PATH}"))?;

    // Permissions: launchd refuses to load plists that aren't root:wheel 0644,
    // and binaries it spawns should be root:wheel 0544 minimum.
    run("chmod", &["644", LAUNCHD_PLIST_PATH])?;
    run("chown", &["root:wheel", LAUNCHD_PLIST_PATH])?;
    run("chmod", &["-R", "755", BUNDLE_PATH])?;
    run("chown", &["-R", "root:wheel", BUNDLE_PATH])?;
    run("chmod", &["544", &target_bin])?;
    run("chmod", &["544", &target_core])?;

    // Enable + load + start.
    run("launchctl", &["enable", &format!("system/{SERVICE_LABEL}")])?;
    run("launchctl", &["bootstrap", "system", LAUNCHD_PLIST_PATH])?;
    // bootstrap auto-starts when RunAtLoad=true; kickstart also turns a stale
    // or previously crashed installation into a deterministic running state.
    run(
        "launchctl",
        &["kickstart", "-k", &format!("system/{SERVICE_LABEL}")],
    )?;

    println!("installed {SERVICE_LABEL}");
    Ok(())
}

/// Boots the LaunchDaemon out if it is loaded and waits until launchd has
/// dropped it and its process has exited.
#[cfg(target_os = "macos")]
fn stop_launchd_job() -> Result<(), anyhow::Error> {
    use anyhow::anyhow;
    use std::process::Command;
    use std::time::{Duration, Instant};

    let target = format!("system/{SERVICE_LABEL}");
    let print = || {
        Command::new("launchctl")
            .args(["print", &target])
            .output()
            .map_err(|e| anyhow!("spawn launchctl print: {e}"))
    };
    let listing = print()?;
    if !listing.status.success() {
        return Ok(()); // not loaded
    }
    let pid = launchd_job_pid(&String::from_utf8_lossy(&listing.stdout));
    let status = Command::new("launchctl")
        .args(["bootout", &target])
        .status()
        .map_err(|e| anyhow!("spawn launchctl bootout: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "launchctl bootout {target} failed (exit {})",
            status.code().unwrap_or(-1)
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let unloaded = !print()?.status.success();
        let exited = pid.is_none_or(|pid| !process_alive(pid));
        if unloaded && exited {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "{SERVICE_LABEL} still running after bootout (pid {pid:?})"
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The `pid = N` line of `launchctl print system/<label>`; `None` when the
/// job is loaded but not running.
#[cfg(any(target_os = "macos", test))]
fn launchd_job_pid(listing: &str) -> Option<i32> {
    listing.lines().find_map(|line| {
        let value = line.trim().strip_prefix("pid = ")?;
        value.trim().parse().ok().filter(|pid| *pid > 0)
    })
}

#[cfg(target_os = "macos")]
fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(target_os = "macos")]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(target_os = "macos")]
fn run(cmd: &str, args: &[&str]) -> Result<(), anyhow::Error> {
    use anyhow::anyhow;
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .map_err(|e| anyhow!("spawn {cmd}: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "{cmd} {} failed (exit {})",
            args.join(" "),
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

// ───────────────────────────── Linux ─────────────────────────────

#[cfg(target_os = "linux")]
const SERVICE_UNIT: &str = "ppvpn-service.service";

#[cfg(target_os = "linux")]
const UNIT_PATH: &str = "/etc/systemd/system/ppvpn-service.service";

#[cfg(target_os = "linux")]
const INSTALL_DIR: &str = "/usr/lib/ppvpn-service";

// RuntimeDirectory/StateDirectory/LogsDirectory back the paths in
// protocol.rs, core.rs and logfile.rs (systemd keeps LogsDirectory= when the
// service stops; the uninstaller leaves it too). KillMode stays
// control-group so ppvpn-core gets SIGTERM with the service and tears down
// its routes; the service refuses new Connects from then on (stopping
// state) so no client starts another core while it shuts down.
#[cfg(target_os = "linux")]
const UNIT: &str = r#"[Unit]
Description=PPVPN privileged service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/lib/ppvpn-service/ppvpn-service
Restart=always
RestartSec=5
# On SIGTERM the service stops ppvpn-core in order (/v1/stop, then up to 7 s
# grace), which may take ~17 s.
TimeoutStopSec=30
UMask=0077
RuntimeDirectory=ppvpn
RuntimeDirectoryMode=0755
StateDirectory=ppvpn
StateDirectoryMode=0700
LogsDirectory=ppvpn
LogsDirectoryMode=0700

[Install]
WantedBy=multi-user.target
"#;

#[cfg(target_os = "linux")]
fn main() -> Result<(), anyhow::Error> {
    use anyhow::{anyhow, Context};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    if unsafe { libc::geteuid() } != 0 {
        return Err(anyhow!("must run as root (invoke via `sudo` or `pkexec`)"));
    }
    if !Path::new("/run/systemd/system").is_dir() {
        return Err(anyhow!("LINUX_SYSTEMD_UNAVAILABLE"));
    }

    let exe = std::env::current_exe().context("current_exe")?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("current_exe has no parent"))?;
    let find = |name: &str| {
        [
            dir.join(name),
            dir.join(format!("{name}-x86_64-unknown-linux-gnu")),
            dir.join(format!("{name}-aarch64-unknown-linux-gnu")),
        ]
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("{name} binary not found next to {}", exe.display()))
    };
    let src_bin = find("ppvpn-service")?;
    let src_core = find("ppvpn-core")?;

    // Stop before replacing binaries; a missing unit is fine.
    let _ = std::process::Command::new("systemctl")
        .args(["stop", SERVICE_UNIT])
        .status();

    fs::create_dir_all(INSTALL_DIR).with_context(|| format!("mkdir {INSTALL_DIR}"))?;
    fs::set_permissions(INSTALL_DIR, fs::Permissions::from_mode(0o755))?;
    for (src, name) in [(&src_bin, "ppvpn-service"), (&src_core, "ppvpn-core")] {
        replace_file(src, &Path::new(INSTALL_DIR).join(name), 0o755)?;
    }
    run("chown", &["-R", "root:root", INSTALL_DIR])?;

    fs::write(UNIT_PATH, UNIT).with_context(|| format!("write {UNIT_PATH}"))?;
    fs::set_permissions(UNIT_PATH, fs::Permissions::from_mode(0o644))?;

    run("systemctl", &["daemon-reload"])?;
    run("systemctl", &["enable", SERVICE_UNIT])?;
    // restart also turns a stale or crashed installation into a running one.
    run("systemctl", &["restart", SERVICE_UNIT])?;

    println!("installed {SERVICE_UNIT}");
    Ok(())
}

#[cfg(target_os = "linux")]
fn run(cmd: &str, args: &[&str]) -> Result<(), anyhow::Error> {
    use anyhow::anyhow;
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .map_err(|e| anyhow!("spawn {cmd}: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "{cmd} {} failed (exit {})",
            args.join(" "),
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

// ───────────────────────────── Unix helpers ─────────────────────────────

/// Installs `src` at `target` as a new file: written to a staging file next
/// to it, synced, then renamed over `target`. A process still executing the
/// old file keeps its (unchanged) inode.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn replace_file(
    src: &std::path::Path,
    target: &std::path::Path,
    mode: u32,
) -> Result<(), anyhow::Error> {
    use anyhow::{anyhow, Context};
    use std::fs::{self, File};
    use std::os::unix::fs::PermissionsExt;

    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", target.display()))?;
    let name = target
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", target.display()))?;
    let staging = dir.join(format!(".{}.new", name.to_string_lossy()));
    let _ = fs::remove_file(&staging);
    {
        let mut input = File::open(src).with_context(|| format!("open {}", src.display()))?;
        let mut output =
            File::create(&staging).with_context(|| format!("create {}", staging.display()))?;
        std::io::copy(&mut input, &mut output)
            .with_context(|| format!("copy {} to {}", src.display(), staging.display()))?;
        output.set_permissions(fs::Permissions::from_mode(mode))?;
        output
            .sync_all()
            .with_context(|| format!("sync {}", staging.display()))?;
    }
    fs::rename(&staging, target).with_context(|| format!("install {}", target.display()))?;
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("sync {}", dir.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn launchd_job_pid_reads_the_running_pid() {
        let running = "system/com.peakpassvpn.ppvpn.service = {\n\tactive count = 1\n\tpath = /Library/LaunchDaemons/x.plist\n\tstate = running\n\n\tprogram = /x\n\tpid = 4321\n\timmediate reason = speculative\n}";
        assert_eq!(super::launchd_job_pid(running), Some(4321));
        let stopped = "system/com.peakpassvpn.ppvpn.service = {\n\tstate = not running\n\tlast exit code = 0\n}";
        assert_eq!(super::launchd_job_pid(stopped), None);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn replace_file_swaps_the_inode() {
        use std::os::unix::fs::MetadataExt;
        let dir =
            std::env::temp_dir().join(format!("ppvpn-install-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src");
        let target = dir.join("target");
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&target, b"old").unwrap();
        let before = std::fs::metadata(&target).unwrap().ino();
        super::replace_file(&src, &target, 0o544).unwrap();
        let after = std::fs::metadata(&target).unwrap();
        assert_ne!(after.ino(), before);
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(after.mode() & 0o777, 0o544);
        assert!(!dir.join(".target.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
