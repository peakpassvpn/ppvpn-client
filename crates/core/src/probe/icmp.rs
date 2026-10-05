//! One ICMP echo, never with privilege (Go's probe/icmp_*.go): an
//! unprivileged datagram ICMP socket (SOCK_DGRAM + IPPROTO_ICMP[V6]) on
//! macOS/iOS (any user) and Linux/Android (groups in
//! net.ipv4.ping_group_range); the IP Helper API (IcmpSendEcho2) on Windows.
//! When the host refuses, the probe reports ICMP_UNSUPPORTED.

use std::net::IpAddr;
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub(super) use dgram::ping;
#[cfg(windows)]
pub(super) use windows::ping;

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
pub(super) async fn ping(_to: IpAddr, _timeout: Duration) -> Result<Duration, &'static str> {
    Err(super::result_codes::ICMP_UNSUPPORTED)
}

/// Bytes nobody can guess, to match a reply to its request.
#[cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        windows
    )),
    allow(dead_code)
)]
fn random_bytes<const N: usize>() -> [u8; N] {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(8) {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            hasher.write_u128(now.as_nanos());
        }
        chunk.copy_from_slice(&hasher.finish().to_ne_bytes()[..chunk.len()]);
    }
    out
}

/// The echo request and its reply on the wire, after any IP header.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple")),
    allow(dead_code)
)]
mod wire {
    use crate::probe::result_codes as rc;

    /// An echo request. The IPv4 checksum is ours; the kernel computes the
    /// ICMPv6 one (it covers a pseudo-header).
    pub(super) fn echo_request(v6: bool, id: u16, seq: u16, token: &[u8]) -> Vec<u8> {
        let mut packet = vec![if v6 { 128 } else { 8 }, 0, 0, 0];
        packet.extend_from_slice(&id.to_be_bytes());
        packet.extend_from_slice(&seq.to_be_bytes());
        packet.extend_from_slice(token);
        if !v6 {
            let sum = checksum(&packet);
            packet[2..4].copy_from_slice(&sum.to_be_bytes());
        }
        packet
    }

    pub(super) fn checksum(data: &[u8]) -> u16 {
        let mut sum: u32 = data
            .chunks(2)
            .map(|c| u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])))
            .sum();
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    /// What a datagram says of our echo: None when it is not about it.
    /// Linux rewrites the echo identifier to the socket's port, so replies
    /// are matched on the sequence number and the token.
    pub(super) fn classify(
        packet: &[u8],
        v6: bool,
        seq: u16,
        token: &[u8],
    ) -> Option<Result<(), &'static str>> {
        // macOS hands IPv4 replies over with their IP header.
        let packet = match packet.first() {
            Some(b) if !v6 && b >> 4 == 4 => packet.get(usize::from(b & 0x0f) * 4..)?,
            _ => packet,
        };
        if packet.len() < 8 {
            return None;
        }
        let (reply, unreachable) = if v6 { (129, 1) } else { (0, 3) };
        let body = &packet[8..];
        if packet[0] == reply && packet[6..8] == seq.to_be_bytes() && body == token {
            return Some(Ok(()));
        }
        if packet[0] == unreachable && quotes_echo(body, v6, seq) {
            return Some(Err(rc::ICMP_UNREACHABLE));
        }
        None
    }

    /// Whether an ICMP error quotes our echo request.
    fn quotes_echo(data: &[u8], v6: bool, seq: u16) -> bool {
        let offset = if v6 {
            40
        } else {
            match data.first() {
                Some(b) => usize::from(b & 0x0f) * 4,
                None => return false,
            }
        };
        data.get(offset + 6..offset + 8) == Some(&seq.to_be_bytes()[..])
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod dgram {
    use std::io;
    use std::net::{IpAddr, SocketAddr};
    use std::time::{Duration, Instant};

    use socket2::{Domain, Protocol, Socket, Type};
    use tokio::net::UdpSocket;

    use super::wire::{classify, echo_request};
    use crate::probe::result_codes as rc;

    pub(in crate::probe) async fn ping(
        to: IpAddr,
        timeout: Duration,
    ) -> Result<Duration, &'static str> {
        match tokio::time::timeout(timeout, echo(to.to_canonical())).await {
            Ok(outcome) => outcome,
            Err(_) => Err(rc::ICMP_TIMEOUT),
        }
    }

    async fn echo(to: IpAddr) -> Result<Duration, &'static str> {
        let v6 = to.is_ipv6();
        let (domain, protocol) = if v6 {
            (Domain::IPV6, Protocol::ICMPV6)
        } else {
            (Domain::IPV4, Protocol::ICMPV4)
        };
        let socket =
            Socket::new(domain, Type::DGRAM, Some(protocol)).map_err(|e| socket_code(&e))?;
        socket.set_nonblocking(true).map_err(|_| rc::ICMP_FAILED)?;
        let socket = UdpSocket::from_std(socket.into()).map_err(|_| rc::ICMP_FAILED)?;
        let random: [u8; 20] = super::random_bytes();
        let id = u16::from_be_bytes([random[0], random[1]]);
        let seq = u16::from_be_bytes([random[2], random[3]]);
        let token = &random[4..];
        let packet = echo_request(v6, id, seq, token);
        let started = Instant::now();
        socket
            .send_to(&packet, SocketAddr::new(to, 0))
            .await
            .map_err(|e| io_code(&e))?;
        let mut buffer = [0u8; 1500];
        loop {
            let (n, peer) = socket
                .recv_from(&mut buffer)
                .await
                .map_err(|e| io_code(&e))?;
            let rtt = started.elapsed();
            match classify(&buffer[..n], v6, seq, token) {
                Some(Ok(())) if peer.ip().to_canonical() == to => return Ok(rtt),
                Some(Err(code)) => return Err(code),
                _ => continue,
            }
        }
    }

    fn socket_code(error: &io::Error) -> &'static str {
        match error.raw_os_error() {
            Some(
                libc::EACCES
                | libc::EPERM
                | libc::EPROTONOSUPPORT
                | libc::EAFNOSUPPORT
                | libc::ESOCKTNOSUPPORT,
            ) => rc::ICMP_UNSUPPORTED,
            _ => rc::ICMP_FAILED,
        }
    }

    fn io_code(error: &io::Error) -> &'static str {
        match error.raw_os_error() {
            Some(
                libc::EHOSTUNREACH
                | libc::ENETUNREACH
                | libc::ECONNREFUSED
                | libc::EHOSTDOWN
                | libc::EADDRNOTAVAIL,
            ) => rc::ICMP_UNREACHABLE,
            Some(libc::EACCES | libc::EPERM) => rc::ICMP_UNSUPPORTED,
            _ if error.kind() == io::ErrorKind::TimedOut => rc::ICMP_TIMEOUT,
            _ => rc::ICMP_FAILED,
        }
    }
}

#[cfg(windows)]
mod windows {
    //! IcmpSendEcho2 / Icmp6SendEcho2 from iphlpapi.dll: no raw socket.

    use std::ffi::c_void;
    use std::net::IpAddr;
    use std::time::{Duration, Instant};

    use crate::probe::result_codes as rc;

    type Handle = *mut c_void;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    const AF_INET6: u16 = 23;

    #[repr(C)]
    struct SockaddrIn6 {
        family: u16,
        port: u16,
        flowinfo: u32,
        addr: [u8; 16],
        scope_id: u32,
    }

    #[link(name = "iphlpapi")]
    extern "system" {
        fn IcmpCreateFile() -> Handle;
        fn Icmp6CreateFile() -> Handle;
        fn IcmpCloseHandle(handle: Handle) -> i32;
        #[allow(clippy::too_many_arguments)]
        fn IcmpSendEcho2(
            handle: Handle,
            event: Handle,
            apc_routine: *mut c_void,
            apc_context: *mut c_void,
            destination: u32,
            request_data: *const c_void,
            request_size: u16,
            request_options: *const c_void,
            reply_buffer: *mut c_void,
            reply_size: u32,
            timeout: u32,
        ) -> u32;
        #[allow(clippy::too_many_arguments)]
        fn Icmp6SendEcho2(
            handle: Handle,
            event: Handle,
            apc_routine: *mut c_void,
            apc_context: *mut c_void,
            source: *const SockaddrIn6,
            destination: *const SockaddrIn6,
            request_data: *const c_void,
            request_size: u16,
            request_options: *const c_void,
            reply_buffer: *mut c_void,
            reply_size: u32,
            timeout: u32,
        ) -> u32;
    }

    // IP_STATUS values (ipexport.h); the IPv6 aliases share the numbers.
    const IP_SUCCESS: u32 = 0;
    const IP_STATUS_BASE: u32 = 11000;
    const IP_STATUS_MAX: u32 = 11999;
    const IP_REQ_TIMED_OUT: u32 = 11010;
    const UNREACHABLE: [u32; 11] = [
        11002, 11003, 11004, 11005, 11012, 11013, 11014, 11018, 11040, 11041, 11045,
    ];
    const ERROR_ACCESS_DENIED: u32 = 5;
    const ERROR_NOT_SUPPORTED: u32 = 50;
    const PAYLOAD: usize = 16;

    pub(in crate::probe) async fn ping(
        to: IpAddr,
        timeout: Duration,
    ) -> Result<Duration, &'static str> {
        let ms = u32::try_from(timeout.as_millis())
            .unwrap_or(u32::MAX)
            .max(1);
        // The call blocks for at most `ms`; dropping the future leaves it to
        // finish on its own thread.
        tokio::task::spawn_blocking(move || send_echo(to.to_canonical(), ms))
            .await
            .unwrap_or(Err(rc::ICMP_FAILED))
    }

    fn send_echo(to: IpAddr, timeout_ms: u32) -> Result<Duration, &'static str> {
        // SAFETY: plain Win32 calls; every pointer is to a live local buffer
        // of the size passed with it, and the handle is closed once.
        unsafe {
            let handle = if to.is_ipv6() {
                Icmp6CreateFile()
            } else {
                IcmpCreateFile()
            };
            if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                return Err(rc::ICMP_UNSUPPORTED);
            }
            let payload: [u8; PAYLOAD] = super::random_bytes();
            // ICMP(V6)_ECHO_REPLY, the payload, an ICMP error's quote and the
            // 8 bytes the API asks for; u64 for the struct's alignment.
            let mut reply = [0u64; (128 + PAYLOAD + 8) / 8];
            let reply_size = std::mem::size_of_val(&reply) as u32;
            let started = Instant::now();
            let count = match to {
                IpAddr::V4(ip) => IcmpSendEcho2(
                    handle,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    u32::from_ne_bytes(ip.octets()),
                    payload.as_ptr().cast(),
                    PAYLOAD as u16,
                    std::ptr::null(),
                    reply.as_mut_ptr().cast(),
                    reply_size,
                    timeout_ms,
                ),
                IpAddr::V6(ip) => {
                    let source = SockaddrIn6 {
                        family: AF_INET6,
                        port: 0,
                        flowinfo: 0,
                        addr: [0; 16],
                        scope_id: 0,
                    };
                    let destination = SockaddrIn6 {
                        family: AF_INET6,
                        port: 0,
                        flowinfo: 0,
                        addr: ip.octets(),
                        scope_id: 0,
                    };
                    Icmp6SendEcho2(
                        handle,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &source,
                        &destination,
                        payload.as_ptr().cast(),
                        PAYLOAD as u16,
                        std::ptr::null(),
                        reply.as_mut_ptr().cast(),
                        reply_size,
                        timeout_ms,
                    )
                }
            };
            let rtt = started.elapsed();
            let last_error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
            IcmpCloseHandle(handle);
            if count == 0 {
                return Err(status_code(last_error));
            }
            let bytes: Vec<u8> = reply.iter().flat_map(|w| w.to_ne_bytes()).collect();
            match reply_status(&bytes, to.is_ipv6()) {
                IP_SUCCESS => Ok(rtt),
                status => Err(status_code(status)),
            }
        }
    }

    /// IP_STATUS of the first reply. ICMP_ECHO_REPLY starts with Address(4)
    /// Status(4). ICMPV6_ECHO_REPLY starts with a packed 26-byte
    /// IPV6_ADDRESS_EX, Status at 28 when aligned: the four bytes at 26 are
    /// zero exactly when the status is IP_SUCCESS under either layout.
    fn reply_status(reply: &[u8], v6: bool) -> u32 {
        let at = |i: usize| u32::from_le_bytes(reply[i..i + 4].try_into().unwrap());
        if !v6 {
            return at(4);
        }
        let packed = at(26);
        if packed == IP_SUCCESS {
            return IP_SUCCESS;
        }
        let aligned = at(28);
        if (IP_STATUS_BASE..=IP_STATUS_MAX).contains(&aligned) {
            aligned
        } else {
            packed
        }
    }

    fn status_code(status: u32) -> &'static str {
        match status {
            IP_REQ_TIMED_OUT => rc::ICMP_TIMEOUT,
            s if UNREACHABLE.contains(&s) => rc::ICMP_UNREACHABLE,
            ERROR_ACCESS_DENIED | ERROR_NOT_SUPPORTED => rc::ICMP_UNSUPPORTED,
            _ => rc::ICMP_FAILED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wire::{checksum, classify, echo_request};
    use super::*;
    use crate::probe::result_codes as rc;

    #[test]
    fn echo_request_and_reply() {
        let token = [7u8; 16];
        let request = echo_request(false, 0x1234, 0x0042, &token);
        assert_eq!(&request[..2], &[8, 0]);
        assert_eq!(
            checksum(&request),
            0,
            "a packet with its checksum sums to 0"
        );
        // The reply, with the IPv4 header macOS keeps on it.
        let mut reply = request.clone();
        reply[0] = 0;
        let mut with_header = vec![0x45; 1];
        with_header.extend_from_slice(&[0; 19]);
        with_header.extend_from_slice(&reply);
        assert_eq!(classify(&reply, false, 0x0042, &token), Some(Ok(())));
        assert_eq!(classify(&with_header, false, 0x0042, &token), Some(Ok(())));
        assert_eq!(classify(&reply, false, 0x0043, &token), None);
        assert_eq!(classify(&reply, false, 0x0042, &[0; 16]), None);
        // Destination unreachable, quoting our request after its IP header.
        let mut unreachable = vec![3, 1, 0, 0, 0, 0, 0, 0, 0x45];
        unreachable.extend_from_slice(&[0; 19]);
        unreachable.extend_from_slice(&request);
        assert_eq!(
            classify(&unreachable, false, 0x0042, &token),
            Some(Err(rc::ICMP_UNREACHABLE))
        );
        assert_eq!(classify(&unreachable, false, 0x0001, &token), None);
        let v6 = echo_request(true, 1, 9, &token);
        assert_eq!((v6[0], &v6[2..4]), (128, &[0, 0][..]));
        let mut v6_reply = v6.clone();
        v6_reply[0] = 129;
        assert_eq!(classify(&v6_reply, true, 9, &token), Some(Ok(())));
    }

    // Go: TestPingLoopback. Skipped where the host forbids unprivileged
    // ICMP (Linux outside ping_group_range), which is reported as such.
    #[tokio::test]
    async fn ping_loopback() {
        for target in ["127.0.0.1", "::1"] {
            match ping(target.parse().unwrap(), Duration::from_secs(2)).await {
                Err(rc::ICMP_UNSUPPORTED) => eprintln!("{target}: unprivileged ICMP unavailable"),
                Err(rc::ICMP_UNREACHABLE | rc::ICMP_FAILED) if target == "::1" => {
                    eprintln!("{target}: IPv6 loopback unavailable")
                }
                // Windows reports whole milliseconds: a loopback echo may take 0.
                Ok(rtt) => assert!(rtt <= Duration::from_secs(2), "{target}: {rtt:?}"),
                Err(code) => panic!("{target}: {code}"),
            }
        }
    }

    // Go: TestPingTimeoutAndCancel. 192.0.2.0/24 (TEST-NET-1) is never
    // routed: the echo times out or is unreachable, never succeeds.
    #[tokio::test]
    async fn ping_timeout_and_cancel() {
        let target: IpAddr = "192.0.2.1".parse().unwrap();
        match ping(target, Duration::from_millis(200)).await {
            Err(rc::ICMP_UNSUPPORTED) => return,
            Err(rc::ICMP_TIMEOUT | rc::ICMP_UNREACHABLE) => {}
            other => panic!("{other:?}"),
        }
        // Cancelling is dropping the future: it returns at once.
        let started = std::time::Instant::now();
        let cancelled = tokio::time::timeout(
            Duration::from_millis(50),
            ping(target, Duration::from_secs(5)),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!matches!(cancelled, Ok(Ok(_))));
    }
}
