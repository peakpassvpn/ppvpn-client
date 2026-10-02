//! The daemon's process record and how a later invocation recognises it.
//!
//! The record lives in the persistent runtime directory, so it can outlive a
//! reboot, after which its PID may belong to an unrelated process, even one
//! running the same executable. A record is the CLI's daemon only when the
//! PID is alive, runs the recorded executable, and started within
//! [`START_SKEW`] of the recorded time.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const START_SKEW: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub pid: u32,
    pub executable: PathBuf,
    /// Unix time in milliseconds.
    pub started_at_ms: u64,
}

impl ProcessRecord {
    pub fn current() -> std::io::Result<ProcessRecord> {
        let started = process_start_time(std::process::id())?;
        Ok(ProcessRecord {
            pid: std::process::id(),
            executable: std::env::current_exe()?,
            started_at_ms: millis(started),
        })
    }

    /// Whether this record still describes a running process.
    pub fn is_alive(&self) -> bool {
        let Ok(pid) = i32::try_from(self.pid) else {
            return false;
        };
        if pid <= 0 || !process_running(pid) {
            return false;
        }
        let Ok(actual) = process_executable(self.pid) else {
            return false;
        };
        if !same_file(&actual, &self.executable) {
            return false;
        }
        let Ok(started) = process_start_time(self.pid) else {
            return false;
        };
        millis(started).abs_diff(self.started_at_ms) < START_SKEW.as_millis() as u64
    }
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn process_running(pid: i32) -> bool {
    // Signal 0 checks existence; EPERM means it exists but is not ours.
    let alive = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    alive && !is_zombie(pid)
}

#[cfg(target_os = "linux")]
fn is_zombie(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn is_zombie(pid: i32) -> bool {
    bsd_info(pid as u32).is_some_and(|info| info.pbi_status == libc::SZOMB)
}

#[cfg(target_os = "linux")]
fn process_executable(pid: u32) -> std::io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> std::io::Result<SystemTime> {
    let invalid = || std::io::Error::other("malformed /proc stat");
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // Fields after the parenthesised command start at field 3; starttime is
    // field 22, in clock ticks after boot.
    let (_, rest) = stat.rsplit_once(')').ok_or_else(invalid)?;
    let ticks: u64 = rest
        .split_whitespace()
        .nth(19)
        .and_then(|v| v.parse().ok())
        .ok_or_else(invalid)?;
    let boot: u64 = std::fs::read_to_string("/proc/stat")?
        .lines()
        .find_map(|line| {
            line.strip_prefix("btime ")
                .and_then(|v| v.trim().parse().ok())
        })
        .ok_or_else(invalid)?;
    let hz = match unsafe { libc::sysconf(libc::_SC_CLK_TCK) } {
        hz if hz > 0 => hz as u64,
        _ => 100,
    };
    Ok(UNIX_EPOCH + Duration::from_secs(boot) + Duration::from_millis(ticks * 1000 / hz))
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let written = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast::<libc::c_void>(),
            size,
        )
    };
    (written == size).then_some(info)
}

#[cfg(target_os = "macos")]
fn process_executable(pid: u32) -> std::io::Result<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe {
        libc::proc_pidpath(
            pid as i32,
            buf.as_mut_ptr().cast::<libc::c_void>(),
            buf.len() as u32,
        )
    };
    if len <= 0 {
        return Err(std::io::Error::last_os_error());
    }
    buf.truncate(len as usize);
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(buf)))
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> std::io::Result<SystemTime> {
    let info = bsd_info(pid).ok_or_else(std::io::Error::last_os_error)?;
    Ok(UNIX_EPOCH
        + Duration::from_secs(info.pbi_start_tvsec)
        + Duration::from_micros(info.pbi_start_tvusec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};

    fn sleeper() -> (Child, ProcessRecord) {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let executable = process_executable(child.id()).unwrap();
        let record = ProcessRecord {
            pid: child.id(),
            executable,
            started_at_ms: millis(SystemTime::now()),
        };
        (child, record)
    }

    #[test]
    fn a_live_process_matches_its_record() {
        let (mut child, record) = sleeper();
        assert!(record.is_alive());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!record.is_alive(), "an exited process must not match");
    }

    #[test]
    fn a_reused_pid_does_not_match() {
        let (mut child, record) = sleeper();
        let earlier = ProcessRecord {
            started_at_ms: record.started_at_ms - 3_600_000,
            ..record.clone()
        };
        assert!(
            !earlier.is_alive(),
            "a process started an hour after the record"
        );
        let other = ProcessRecord {
            executable: PathBuf::from("/nonexistent/ppvpn"),
            ..record
        };
        assert!(!other.is_alive(), "a process running another executable");
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn the_current_process_is_recognised() {
        let record = ProcessRecord::current().unwrap();
        assert!(record.is_alive());
        let age = millis(SystemTime::now()).saturating_sub(record.started_at_ms);
        assert!(age < 24 * 3_600_000, "start time {age} ms ago");
    }
}
