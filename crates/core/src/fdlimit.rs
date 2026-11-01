//! The process's open-file limit (#545). Each proxied connection holds two
//! descriptors (and a DNS lookup one more), and macOS starts GUI apps and
//! launchd daemons with a soft limit of 256: a speed test through the local
//! proxy ran out (`EMFILE` on accept, connect and getaddrinfo alike), and
//! traffic stalled until connections closed. systemd's default is 1024.
//!
//! [`raise`] lifts the soft limit as far as the hard limit and the kernel's
//! per-process cap allow, up to [`TARGET`]; the hard limit is never touched.
//! Android and iOS hosts keep their own limits.

/// The soft limit asked for: far above any connection count an instance
/// sees, and within every hard limit a desktop ships with.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const TARGET: libc::rlim_t = 65_536;

/// The soft limit before and after [`raise`] (equal when it was already
/// high enough).
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Raised {
    pub before: libc::rlim_t,
    pub after: libc::rlim_t,
}

/// The soft limit to set: `TARGET` within `hard` and the kernel's cap, and
/// never below `current`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn target(current: libc::rlim_t, hard: libc::rlim_t, cap: Option<libc::rlim_t>) -> libc::rlim_t {
    TARGET
        .min(hard)
        .min(cap.unwrap_or(libc::RLIM_INFINITY))
        .max(current)
}

/// macOS refuses a soft limit above `kern.maxfilesperproc` (even with an
/// unlimited hard limit).
#[cfg(target_os = "macos")]
fn kernel_cap() -> Option<libc::rlim_t> {
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is NUL-terminated, and `value`/`size` describe a
    // c_int the call writes at most `size` bytes into.
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&mut value as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return None;
    }
    libc::rlim_t::try_from(value).ok().filter(|cap| *cap > 0)
}

/// Linux bounds the hard limit by `fs.nr_open` already.
#[cfg(target_os = "linux")]
fn kernel_cap() -> Option<libc::rlim_t> {
    None
}

/// Raises the soft open-file limit (see the module docs). Idempotent;
/// `Err` carries the OS error of a failed call.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn raise() -> Result<Raised, String> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes one rlimit into `limit`.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(format!("getrlimit: {}", std::io::Error::last_os_error()));
    }
    let before = limit.rlim_cur;
    let wanted = target(before, limit.rlim_max, kernel_cap());
    if wanted != before {
        limit.rlim_cur = wanted;
        // SAFETY: setrlimit reads one rlimit from `limit`.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
            return Err(format!(
                "setrlimit {before} -> {wanted}: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(Raised {
        before,
        after: wanted,
    })
}

/// [`raise`], logged: the line to look for when connections fail with
/// "Too many open files". Other platforms keep the limit they start with.
pub(crate) fn raise_and_log() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    match raise() {
        Ok(Raised { before, after }) => {
            tracing::info!(before, after, "open file limit");
        }
        Err(error) => tracing::warn!(%error, "open file limit not raised"),
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn the_soft_limit_rises_within_the_hard_limit_and_the_kernel_cap() {
        // macOS GUI apps: 256 soft, unlimited hard, 92160 per process.
        assert_eq!(target(256, libc::RLIM_INFINITY, Some(92_160)), TARGET);
        assert_eq!(target(256, libc::RLIM_INFINITY, Some(10_240)), 10_240);
        // systemd: 1024 soft, 524288 hard.
        assert_eq!(target(1_024, 524_288, None), TARGET);
        assert_eq!(target(1_024, 4_096, None), 4_096);
        // Already higher: left alone.
        assert_eq!(target(1_048_576, 1_048_576, None), 1_048_576);
    }

    #[test]
    fn raising_leaves_room_for_hundreds_of_connections() {
        let raised = raise().unwrap();
        assert!(raised.after >= raised.before);
        // Idempotent.
        assert_eq!(raise().unwrap().before, raised.after);

        // 300 concurrent connections through a loopback listener: 601
        // descriptors (client and accepted end each, and the listener), past 256.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut held = Vec::new();
        for _ in 0..300 {
            let client = std::net::TcpStream::connect(address).unwrap();
            let (accepted, _) = listener.accept().unwrap();
            held.push((client, accepted));
        }
        assert_eq!(held.len(), 300);
    }
}
