//! Explicit administrator opt-in for a temporary physical lab. This is compiled
//! only into QA helpers and does not change the GitHub-hosted acceptance guard.
use anyhow::{bail, Result};
use serde::Deserialize;
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub const MARKER: &str = "/private/var/db/ppvpn-capture-lab-acceptance.json"; // sensitive-check: allow — administrator-owned temporary lab authorization
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Permit {
    version: u32,
    uid: u32,
    expires_at: u64,
    boot_session: String,
    helper_sha256: String,
}
fn validate(permit: &Permit, uid: u32, now: u64, boot: &str, hash: &str) -> Result<()> {
    if permit.version != 1
        || uid == 0
        || permit.uid != uid
        || permit.expires_at <= now
        || permit.expires_at - now > 1800
        || permit.boot_session != boot
        || permit.helper_sha256 != hash
    {
        bail!("PHYSICAL_LAB_AUTHORIZATION_MISMATCH");
    }
    Ok(())
}
fn boot_session() -> Result<String> {
    let mut bytes = [0u8; 128];
    let mut size = bytes.len();
    if unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size == 0
        || size > bytes.len()
    {
        bail!("PHYSICAL_LAB_BOOT_ID_UNAVAILABLE");
    }
    Ok(std::str::from_utf8(&bytes[..size])?
        .trim_end_matches('\0')
        .to_owned())
}
pub fn authorize() -> Result<()> {
    let image = Path::new(super::broker::IMAGE);
    super::mac_peer::secure_installed_image(image)?;
    if std::env::current_exe()?.canonicalize()? != image {
        bail!("PHYSICAL_LAB_INSTALLED_IMAGE_REQUIRED");
    }
    // Check trusted ancestry, then open without following a final symlink and
    // validate the actual opened inode. A user cannot replace this authorization.
    super::mac_peer::secure_installed_image(Path::new(MARKER))?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(MARKER)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != 0
        || meta.nlink() != 1
        || meta.mode() & 0o777 != 0o444
        || meta.len() > 4096
    {
        bail!("PHYSICAL_LAB_AUTHORIZATION_UNSAFE");
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let permit: Permit = serde_json::from_str(&text)?;
    let hash = hex::encode(ring::digest::digest(
        &ring::digest::SHA256,
        &std::fs::read(image)?,
    ));
    validate(
        &permit,
        unsafe { libc::getuid() },
        crate::protocol::now_epoch(),
        &boot_session()?,
        &hash,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lab_authorization_binds_boot_user_artifact_and_short_expiry() {
        let mut p = Permit {
            version: 1,
            uid: 501,
            expires_at: 1100,
            boot_session: "boot-a".into(),
            helper_sha256: "signed-image-hash".into(),
        };
        assert!(validate(&p, 501, 1000, "boot-a", "signed-image-hash").is_ok());
        for (uid, now, boot, hash) in [
            (0, 1000, "boot-a", "signed-image-hash"),
            (502, 1000, "boot-a", "signed-image-hash"),
            (501, 1100, "boot-a", "signed-image-hash"),
            (501, 1000, "boot-b", "signed-image-hash"),
            (501, 1000, "boot-a", "changed-image"),
        ] {
            assert!(validate(&p, uid, now, boot, hash).is_err());
        }
        p.expires_at = 2801;
        assert!(validate(&p, 501, 1000, "boot-a", "signed-image-hash").is_err());
    }
}
