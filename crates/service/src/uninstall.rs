//! uninstall-service binary — removes the ppvpn-service installation.
//! Must run with admin/root privileges (mirrors install.rs).

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("uninstall-service supports only Windows, macOS and Linux");
    std::process::exit(2);
}

// ───────────────────────────── Windows ─────────────────────────────

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    use std::thread::sleep;
    use std::time::Duration;
    use windows_service::{
        service::{ServiceAccess, ServiceState},
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    const SERVICE_NAME: &str = "ppvpn_service";

    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;

    let access = ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE;
    let svc = match mgr.open_service(SERVICE_NAME, access) {
        Ok(s) => s,
        Err(_) => {
            println!("service {SERVICE_NAME} not installed, nothing to do");
            return Ok(());
        }
    };

    if let Ok(status) = svc.query_status() {
        if status.current_state != ServiceState::Stopped {
            let _ = svc.stop();
            for _ in 0..20 {
                if let Ok(s) = svc.query_status() {
                    if s.current_state == ServiceState::Stopped {
                        break;
                    }
                }
                sleep(Duration::from_millis(250));
            }
        }
    }

    svc.delete()?;
    let _ = std::fs::remove_file(secret_file_path());
    // %ProgramData%\PPVPN\logs stays: the logs must survive a reinstall.
    println!("service {SERVICE_NAME} removed");
    Ok(())
}

#[cfg(windows)]
fn secret_file_path() -> std::path::PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"));
    base.join("PPVPN").join("service.secret")
}

// ───────────────────────────── macOS ─────────────────────────────

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod macdns;

#[cfg(target_os = "macos")]
fn main() -> Result<(), anyhow::Error> {
    use anyhow::anyhow;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    const SERVICE_LABEL: &str = "com.peakpassvpn.ppvpn.service";
    const BUNDLE_PATH: &str = "/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle";
    const LAUNCHD_PLIST_PATH: &str = "/Library/LaunchDaemons/com.peakpassvpn.ppvpn.service.plist";

    if unsafe { libc::geteuid() } != 0 {
        return Err(anyhow!(
            "must run as root (invoke via `sudo` or `osascript with administrator privileges`)"
        ));
    }

    // Best-effort: try to stop and bootout even if the plist is missing.
    let _ = Command::new("launchctl")
        .args(["stop", SERVICE_LABEL])
        .status();
    let _ = Command::new("launchctl")
        .args(["bootout", "system", LAUNCHD_PLIST_PATH])
        .status();
    // The service removes its system DNS override when it stops; one left
    // by a service that was killed instead goes here.
    macdns::TunDns::default().clean_leftover("uninstall");

    let _ = fs::remove_file(LAUNCHD_PLIST_PATH);
    if Path::new(BUNDLE_PATH).exists() {
        let _ = fs::remove_dir_all(BUNDLE_PATH);
    }
    let _ = fs::remove_dir_all("/Library/Application Support/PPVPN");
    // Logs of earlier builds (launchd's stdout / stderr files). The current
    // ones, in /Library/Logs/PPVPN, stay: they are the evidence of what went
    // wrong when a tester reinstalls the service.
    let _ = fs::remove_file("/Library/Logs/ppvpn-service.out.log");
    let _ = fs::remove_file("/Library/Logs/ppvpn-service.err.log");

    println!("uninstalled {SERVICE_LABEL}");
    Ok(())
}

// ───────────────────────────── Linux ─────────────────────────────

#[cfg(target_os = "linux")]
fn main() -> Result<(), anyhow::Error> {
    use anyhow::anyhow;
    use std::fs;
    use std::process::Command;

    const SERVICE_UNIT: &str = "ppvpn-service.service";
    const UNIT_PATH: &str = "/etc/systemd/system/ppvpn-service.service";
    const INSTALL_DIR: &str = "/usr/lib/ppvpn-service";

    if unsafe { libc::geteuid() } != 0 {
        return Err(anyhow!("must run as root (invoke via `sudo` or `pkexec`)"));
    }

    // Best-effort: continue even when the unit is already gone.
    let _ = Command::new("systemctl")
        .args(["disable", "--now", SERVICE_UNIT])
        .status();
    let _ = fs::remove_file(UNIT_PATH);
    let _ = Command::new("systemctl").arg("daemon-reload").status();

    let _ = fs::remove_dir_all(INSTALL_DIR);
    let _ = fs::remove_dir_all("/run/ppvpn");
    let _ = fs::remove_dir_all("/var/lib/ppvpn");
    // /var/log/ppvpn stays: the service and core logs must survive a
    // reinstall.

    println!("uninstalled {SERVICE_UNIT}");
    Ok(())
}
