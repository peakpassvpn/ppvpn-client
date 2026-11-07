//! macOS audit-token identity with no ad-hoc/debug bypass. Expected image and
//! signing id are trusted installation policy, never values from an IPC frame.
use crate::peer_identity::Peer;
use anyhow::{anyhow, Result};
use std::os::fd::AsRawFd;
use std::os::unix::{fs::MetadataExt, net::UnixStream};
use std::path::Path;
pub fn audit_peer(stream: &UnixStream) -> Result<(Peer, [u32; 8])> {
    let mut token = [0u32; 8];
    let mut length = std::mem::size_of_val(&token) as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0
        || length as usize != std::mem::size_of_val(&token)
        || token[5] == 0
        || token[7] == 0
    {
        return Err(anyhow!("PEER_AUDIT_UNAVAILABLE"));
    }
    // Darwin audit_token_t: auid,euid,egid,ruid,rgid,pid,asid,pidversion.
    if token[1] != token[3] {
        return Err(anyhow!("PEER_SETUID_REJECTED"));
    }
    Ok((
        Peer {
            uid: token[1],
            pid: token[5],
            pid_version: token[7],
            audit_session: token[6],
        },
        token,
    ))
}
pub fn secure_installed_image(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.canonicalize()? != path {
        return Err(anyhow!("INSTALL_IMAGE_NOT_CANONICAL"));
    }
    for entry in path.ancestors() {
        let meta = std::fs::symlink_metadata(entry)?;
        if meta.file_type().is_symlink()
            || meta.uid() != 0
            || meta.mode() & 0o002 != 0
            || (meta.mode() & 0o020 != 0
                && !(entry == Path::new("/Applications") && meta.is_dir() && meta.gid() == 80))
        {
            return Err(anyhow!("INSTALL_IMAGE_WRITABLE"));
        }
    }
    if !path.is_file() {
        return Err(anyhow!("INSTALL_IMAGE_NOT_FILE"));
    }
    Ok(())
}
pub fn verify(stream: &UnixStream, uid: u32, identifier: &str, image: &Path) -> Result<Peer> {
    let (peer, token) = audit_peer(stream)?;
    if uid == 0 || peer.uid != uid {
        return Err(anyhow!("PEER_USER_REJECTED"));
    }
    secure_installed_image(image)?;
    let requirement = crate::mac_identity::requirement(identifier, crate::mac_identity::team_id())?;
    let bytes: Vec<u8> = token.iter().flat_map(|word| word.to_ne_bytes()).collect();
    crate::mac_identity::check_process(&bytes, &requirement)?;
    crate::mac_identity::check_file(image, &requirement)?;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length =
        unsafe { libc::proc_pidpath(peer.pid as _, buffer.as_mut_ptr().cast(), buffer.len() as _) };
    if length <= 0 {
        return Err(anyhow!("PEER_IMAGE_UNAVAILABLE"));
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    if Path::new(std::str::from_utf8(&buffer[..end])?) != image {
        return Err(anyhow!("PEER_INSTALL_IMAGE_MISMATCH"));
    }
    // Check the audit-token signature again after the PID path lookup, preventing
    // a recycled PID from substituting its executable during verification.
    crate::mac_identity::check_process(&bytes, &requirement)?;
    Ok(peer)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_socket_audit_names_current_process() {
        let (left, _right) = UnixStream::pair().unwrap();
        let (peer, _) = audit_peer(&left).unwrap();
        assert_eq!(peer.pid, std::process::id());
        assert_eq!(peer.uid, unsafe { libc::geteuid() });
        assert_ne!(peer.pid_version, 0);
    }
    #[test]
    fn root_and_wrong_uid_cannot_use_user_endpoint() {
        let (left, _right) = UnixStream::pair().unwrap();
        for uid in [0, unsafe { libc::geteuid() }.saturating_add(1)] {
            assert!(verify(
                &left,
                uid,
                crate::mac_identity::CLIENT_IDENTIFIER,
                Path::new("/not/installed")
            )
            .is_err());
        }
    }
}
