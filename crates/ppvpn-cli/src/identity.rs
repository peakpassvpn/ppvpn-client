//! The daemon's process record and how a later invocation recognises it.
//!
//! The record lives in the persistent runtime directory, so it can outlive a
//! reboot, after which its PID may belong to an unrelated process, even one
//! running the same executable. A record is the CLI's daemon only when the
//! PID is alive, runs the recorded executable, and is the process that was
//! recorded:
//!
//! - Linux: the same boot (the kernel's boot ID) and the same start, in
//!   clock ticks since that boot. Both are the kernel's own values and do
//!   not depend on the wall clock, so setting the clock while the daemon
//!   runs cannot make a live daemon look gone.
//! - macOS: started within [`START_SKEW`] of the recorded time. The kernel
//!   keeps the start as the wall time it was at that moment, so a later
//!   clock change does not move it.

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
    /// Linux: the process's start as the kernel has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<BootStart>,
}

/// A process's start relative to a boot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootStart {
    /// `/proc/sys/kernel/random/boot_id`: new with every boot.
    pub boot_id: String,
    /// `/proc/<pid>/stat` field 22: clock ticks from boot to the start.
    pub start_ticks: u64,
}

impl ProcessRecord {
    pub fn current() -> std::io::Result<ProcessRecord> {
        let pid = std::process::id();
        Ok(ProcessRecord {
            pid,
            executable: std::env::current_exe()?,
            started_at_ms: millis(process_start_time(pid)?),
            boot: boot_start(pid)?,
        })
    }

    /// The record of another running process.
    pub fn of(pid: u32) -> std::io::Result<ProcessRecord> {
        Ok(ProcessRecord {
            pid,
            executable: process_executable(pid)?,
            started_at_ms: millis(process_start_time(pid)?),
            boot: boot_start(pid)?,
        })
    }

    /// Whether this record still describes a running process.
    pub fn is_alive(&self) -> bool {
        self.mismatch().is_none()
    }

    /// Why this record does not describe a running process, with the
    /// recorded and the observed values; `None` when it does.
    pub fn mismatch(&self) -> Option<String> {
        let pid = match i32::try_from(self.pid) {
            Ok(pid) if pid > 0 => pid,
            _ => return Some(format!("pid {} is not a process ID", self.pid)),
        };
        if !process_running(pid) {
            return Some(format!("pid {pid} is not running"));
        }
        let actual = match process_executable(self.pid) {
            Ok(actual) => actual,
            Err(err) => return Some(format!("pid {pid}: cannot read its executable: {err}")),
        };
        if !same_file(&actual, &self.executable) {
            return Some(format!(
                "pid {pid} runs {}, the record has {}",
                actual.display(),
                self.executable.display()
            ));
        }
        let actual = match boot_start(self.pid) {
            Ok(actual) => actual,
            Err(err) => return Some(format!("pid {pid}: cannot read its start: {err}")),
        };
        if let (Some(recorded), Some(actual)) = (&self.boot, &actual) {
            // Exact, and all there is to compare: no clock is involved.
            return (recorded != actual).then(|| {
                format!(
                    "pid {pid} started at tick {} of boot {}, the record has tick {} of boot {}",
                    actual.start_ticks, actual.boot_id, recorded.start_ticks, recorded.boot_id
                )
            });
        }
        let started = match process_start_time(self.pid) {
            Ok(started) => millis(started),
            Err(err) => return Some(format!("pid {pid}: cannot read its start time: {err}")),
        };
        if started.abs_diff(self.started_at_ms) >= START_SKEW.as_millis() as u64 {
            return Some(format!(
                "pid {pid} started at {started} ms, the record has {} ms",
                self.started_at_ms
            ));
        }
        None
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
fn start_ticks(pid: u32) -> std::io::Result<u64> {
    let invalid = || std::io::Error::other("malformed /proc stat");
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // Fields after the parenthesised command start at field 3; starttime is
    // field 22, in clock ticks after boot.
    let (_, rest) = stat.rsplit_once(')').ok_or_else(invalid)?;
    rest.split_whitespace()
        .nth(19)
        .and_then(|v| v.parse().ok())
        .ok_or_else(invalid)
}

#[cfg(target_os = "linux")]
fn boot_start(pid: u32) -> std::io::Result<Option<BootStart>> {
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_string();
    if boot_id.is_empty() {
        return Err(std::io::Error::other("empty boot ID"));
    }
    Ok(Some(BootStart {
        boot_id,
        start_ticks: start_ticks(pid)?,
    }))
}

#[cfg(target_os = "macos")]
fn boot_start(_pid: u32) -> std::io::Result<Option<BootStart>> {
    Ok(None)
}

/// For the record only (Linux compares [`BootStart`]): the boot time the
/// kernel reports follows the wall clock when it is set.
#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> std::io::Result<SystemTime> {
    let invalid = || std::io::Error::other("malformed /proc stat");
    let ticks = start_ticks(pid)?;
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
        let record = ProcessRecord::of(child.id()).unwrap();
        (child, record)
    }

    fn end(mut child: Child) {
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_live_process_matches_its_record() {
        let (child, record) = sleeper();
        assert_eq!(record.mismatch(), None);
        // As the daemon's record is read back from its file.
        let stored: ProcessRecord =
            serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(stored.mismatch(), None);
        end(child);
        assert!(!record.is_alive(), "an exited process must not match");
    }

    #[test]
    fn live_processes_match_their_records_every_time() {
        for round in 0..300 {
            let (child, record) = sleeper();
            let mismatch = record.mismatch();
            end(child);
            assert_eq!(mismatch, None, "round {round}");
        }
    }

    #[test]
    fn a_reused_pid_does_not_match() {
        let (child, record) = sleeper();
        // The same PID and executable, started an hour before this one.
        let mut earlier = record.clone();
        earlier.started_at_ms -= 3_600_000;
        if let Some(boot) = &mut earlier.boot {
            boot.start_ticks = boot.start_ticks.wrapping_sub(360_000);
        }
        assert!(!earlier.is_alive(), "a process started an hour later");
        let other = ProcessRecord {
            executable: PathBuf::from("/nonexistent/ppvpn"),
            ..record
        };
        assert!(!other.is_alive(), "a process running another executable");
        end(child);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_compares_the_boot_and_the_start_tick_not_the_clock() {
        let (child, record) = sleeper();
        assert!(record.boot.is_some());
        // The wall clock was set an hour forward since the record was
        // written: the same process is still recognised.
        let clock_set = ProcessRecord {
            started_at_ms: record.started_at_ms - 3_600_000,
            ..record.clone()
        };
        assert_eq!(clock_set.mismatch(), None);
        // The same PID and start tick after a reboot is another process.
        let mut rebooted = record.clone();
        rebooted.boot.as_mut().unwrap().boot_id = "another-boot".to_string();
        assert!(!rebooted.is_alive());
        // A record without the boot start falls back to the start time.
        let old = ProcessRecord {
            boot: None,
            ..record
        };
        assert_eq!(old.mismatch(), None);
        end(child);
    }

    #[test]
    fn the_current_process_is_recognised() {
        let record = ProcessRecord::current().unwrap();
        assert_eq!(record.mismatch(), None);
        let age = millis(SystemTime::now()).saturating_sub(record.started_at_ms);
        assert!(age < 24 * 3_600_000, "start time {age} ms ago");
    }
}
