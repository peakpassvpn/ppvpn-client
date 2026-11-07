//! Exact one-descriptor transfer on an already authenticated, serialized Unix
//! connection. This is a transport primitive, not authorization: validate the
//! OS peer and signed single-use capture ticket before calling `receive`.
use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

#[derive(Debug)]
pub enum ReceiveError {
    Transport(io::Error),
    InvalidEnvelope,
    /// Darwin may already have installed fds absent from the returned bytes.
    /// The receiving process must contain this by exiting/reconciling, not
    /// continue serving requests and accumulate undisclosed capabilities.
    ContainmentRequired,
}
impl ReceiveError {
    pub fn requires_process_restart(&self) -> bool {
        matches!(self, Self::ContainmentRequired)
    }
    pub fn kind(&self) -> io::ErrorKind {
        match self {
            Self::Transport(error) => error.kind(),
            _ => io::ErrorKind::InvalidData,
        }
    }
}

const MARKER: u8 = 0x46;
// Darwin internalizes one mbuf of rights: UIPC_MAX_CMSG_FD = 512, with
// sockargs additionally bounding expanded rights to MCLBYTES. Allocate for
// that entire kernel bound, not just the one fd accepted by our protocol.
// https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/uipc_usrreq.c
// Darwin can externalize undisclosed fds when an ancillary buffer truncates.
const MAX_RECEIVED: usize = 512;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid descriptor envelope")
}
fn control_buffer(bytes: usize) -> Vec<usize> {
    vec![0; bytes.div_ceil(size_of::<usize>())]
}

/// The sender retains its own descriptor. The peer receives a separate owned
/// descriptor; closing either one does not revoke the other one's capability.
pub fn send(stream: &mut UnixStream, descriptor: &OwnedFd) -> io::Result<()> {
    send_envelope(stream, &[descriptor.as_raw_fd()], MARKER)
}

fn send_envelope(stream: &mut UnixStream, descriptors: &[libc::c_int], tag: u8) -> io::Result<()> {
    let mut marker = tag;
    let mut vector = libc::iovec {
        iov_base: (&mut marker as *mut u8).cast(),
        iov_len: 1,
    };
    // SAFETY: CMSG_SPACE computes storage size for one c_int, not a pointer.
    let bytes = unsafe { libc::CMSG_SPACE(std::mem::size_of_val(descriptors) as _) } as usize;
    let mut control = control_buffer(bytes);
    // SAFETY: all msghdr fields permit zero initialization; referenced buffers
    // remain live and aligned through sendmsg. Exactly one fd is copied.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = bytes as _;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        if header.is_null() {
            return Err(invalid());
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(descriptors) as _) as _;
        for (index, fd) in descriptors.iter().enumerate() {
            std::ptr::write_unaligned(
                libc::CMSG_DATA(header).cast::<libc::c_int>().add(index),
                *fd,
            );
        }
    }
    loop {
        // SAFETY: the message and its iovec/control storage are live above.
        let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
        if sent == 1 {
            return Ok(());
        }
        if sent >= 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "descriptor not sent",
            ));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Reject missing/extra descriptors, malformed marker, ancillary truncation and
/// unexpected ancillary types. Every received fd is immediately owned and is
/// closed on error. CLOEXEC is set before returning the descriptor to callers.
/// macOS receipt holds the shared core FD/spawn gate through CLOEXEC setup;
/// recvmsg has no MSG_CMSG_CLOEXEC atomic equivalent there.
pub fn receive(stream: &mut UnixStream) -> Result<OwnedFd, ReceiveError> {
    #[cfg(target_os = "macos")]
    let _fd_guard = ppvpn_core::capture_host::fd_spawn_guard();
    let mut marker = 0u8;
    let mut vector = libc::iovec {
        iov_base: (&mut marker as *mut u8).cast(),
        iov_len: 1,
    };
    let bytes =
        unsafe { libc::CMSG_SPACE((MAX_RECEIVED * size_of::<libc::c_int>()) as _) } as usize;
    let mut control = control_buffer(bytes);
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = bytes as _;
    let received = loop {
        let count = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, 0) };
        if count >= 0 {
            break count;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(ReceiveError::Transport(error));
        }
        message.msg_controllen = bytes as _;
    };
    let mut descriptors = Vec::new();
    let mut bad = received != 1
        || marker != MARKER
        || message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0;
    // Kernel-produced ancillary headers live in our aligned control buffer.
    // Take ownership of every delivered SCM_RIGHTS fd before validating count.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        let control_start = message.msg_control as usize;
        let control_end = control_start + (message.msg_controllen as usize).min(bytes);
        while !header.is_null() {
            let address = header as usize;
            if address < control_start
                || address.saturating_add(size_of::<libc::cmsghdr>()) > control_end
            {
                bad = true;
                break;
            }
            let remaining = control_end - address;
            let base = libc::CMSG_LEN(0) as usize;
            let declared = (*header).cmsg_len as usize;
            // Darwin can preserve the original cmsg_len on MSG_CTRUNC.
            // Never read descriptors beyond bytes actually copied by recvmsg.
            if declared > remaining {
                bad = true;
            }
            let length = declared.min(remaining);
            if length < base {
                bad = true;
                break;
            }
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let size = length - base;
                if !size.is_multiple_of(size_of::<libc::c_int>()) {
                    bad = true;
                }
                for index in 0..size / size_of::<libc::c_int>() {
                    let fd = std::ptr::read_unaligned(
                        libc::CMSG_DATA(header).cast::<libc::c_int>().add(index),
                    );
                    if fd < 0 {
                        bad = true;
                    } else {
                        descriptors.push(OwnedFd::from_raw_fd(fd));
                    }
                }
            } else {
                bad = true;
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    if message.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(ReceiveError::ContainmentRequired);
    }
    if bad || descriptors.len() != 1 {
        return Err(ReceiveError::InvalidEnvelope);
    }
    let descriptor = descriptors.pop().unwrap();
    let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
    if flags < 0
        || unsafe {
            libc::fcntl(
                descriptor.as_raw_fd(),
                libc::F_SETFD,
                flags | libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(ReceiveError::Transport(io::Error::last_os_error()));
    }
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn descriptor_is_live_owned_cloexec_and_independent_of_sender_copy() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        let (data, mut peer) = UnixStream::pair().unwrap();
        let descriptor: OwnedFd = data.into();
        send(&mut sender, &descriptor).unwrap();
        let copy = receive(&mut receiver).unwrap();
        assert_ne!(copy.as_raw_fd(), descriptor.as_raw_fd());
        assert_ne!(
            unsafe { libc::fcntl(copy.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(descriptor);
        let mut stream = UnixStream::from(copy);
        stream.write_all(b"alive").unwrap();
        let mut bytes = [0; 5];
        peer.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"alive");
        drop(stream);
        assert_eq!(peer.read(&mut bytes).unwrap(), 0);
    }
    #[test]
    fn invalid_envelopes_close_every_delivered_copy() {
        for (count, marker) in [(1, 0), (2, MARKER), (24, MARKER), (200, MARKER)] {
            let (mut sender, mut receiver) = UnixStream::pair().unwrap();
            let (data, mut peer) = UnixStream::pair().unwrap();
            peer.set_read_timeout(Some(std::time::Duration::from_millis(200)))
                .unwrap();
            let descriptor: OwnedFd = data.into();
            send_envelope(&mut sender, &vec![descriptor.as_raw_fd(); count], marker).unwrap();
            assert_eq!(
                receive(&mut receiver).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            drop(descriptor);
            // Any leaked received copy keeps this endpoint alive, causing a
            // timeout instead of EOF. This checks the specific resource.
            assert_eq!(peer.read(&mut [0u8; 1]).unwrap(), 0, "count={count}");
        }
    }
    #[test]
    fn control_messages_above_kernel_bound_are_rejected_before_delivery() {
        let (mut sender, _receiver) = UnixStream::pair().unwrap();
        let (data, _peer) = UnixStream::pair().unwrap();
        assert!(send_envelope(
            &mut sender,
            &vec![data.as_raw_fd(); MAX_RECEIVED + 8],
            MARKER
        )
        .is_err());
    }
    #[test]
    fn broken_receiver_is_a_transport_error() {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        let (data, _peer) = UnixStream::pair().unwrap();
        drop(receiver);
        assert!(send(&mut sender, &data.into()).is_err());
    }
    #[test]
    fn marker_without_descriptor_is_rejected() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_all(&[MARKER]).unwrap();
        assert_eq!(
            receive(&mut receiver).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn eof_does_not_create_a_descriptor() {
        let (sender, mut receiver) = UnixStream::pair().unwrap();
        drop(sender);
        assert_eq!(
            receive(&mut receiver).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
