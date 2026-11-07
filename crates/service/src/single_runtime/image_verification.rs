//! Reuse a full static signature validation only for the exact installed,
//! standalone helper generation. Every hit rehashes every byte and rechecks
//! the entire protected ancestry. Live audit-token validation remains per RPC.
use anyhow::{bail, Result};
use security_framework::os::macos::code_signing::SecRequirement;
use std::{
    fs::{Metadata, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl From<Metadata> for Identity {
    fn from(m: Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            // Directory contents can change for unrelated root-owned siblings.
            // Bind directory identity and current protection, not its contents.
            links: if m.is_dir() { 0 } else { m.nlink() },
            len: if m.is_dir() { 0 } else { m.len() },
            modified: if m.is_dir() {
                (0, 0)
            } else {
                (m.mtime(), m.mtime_nsec())
            },
            changed: if m.is_dir() {
                (0, 0)
            } else {
                (m.ctime(), m.ctime_nsec())
            },
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Generation {
    ancestry: Vec<(PathBuf, Identity)>,
    sha256: [u8; 32],
}
fn ancestry(path: &Path) -> Result<Vec<(PathBuf, Identity)>> {
    path.ancestors()
        .map(|p| Ok((p.to_path_buf(), std::fs::symlink_metadata(p)?.into())))
        .collect()
}
impl Generation {
    fn read(path: &Path) -> Result<Self> {
        let before = ancestry(path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let opened = Identity::from(file.metadata()?);
        if before[0].1 != opened || !file.metadata()?.is_file() || opened.links != 1 {
            bail!("PEER_IMAGE_GENERATION_CHANGED");
        }
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        let mut bytes = [0u8; 32768];
        let mut length = 0;
        loop {
            let count = file.read(&mut bytes)?;
            if count == 0 {
                break;
            }
            digest.update(&bytes[..count]);
            length += count as u64;
        }
        if length != opened.len
            || opened != Identity::from(file.metadata()?)
            || before != ancestry(path)?
        {
            bail!("PEER_IMAGE_GENERATION_CHANGED");
        }
        let sha256 = digest.finish().as_ref().try_into().expect("SHA256 width");
        Ok(Self {
            ancestry: before,
            sha256,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    generation: Generation,
    identifier: String,
    team: String,
}
#[derive(Default)]
struct Cache {
    validated: Option<Key>,
}
impl Cache {
    fn verify(&mut self, key: Key, validate: impl FnOnce() -> Result<Generation>) -> Result<()> {
        if self.validated.as_ref() == Some(&key) {
            if Generation::read(&key.generation.ancestry[0].0)? != key.generation {
                self.validated = None;
                bail!("PEER_IMAGE_GENERATION_CHANGED");
            }
            return Ok(());
        }
        // Failed or racing generations never become an authenticated cache hit.
        self.validated = None;
        if validate()? != key.generation
            || Generation::read(&key.generation.ancestry[0].0)? != key.generation
        {
            bail!("PEER_IMAGE_GENERATION_CHANGED");
        }
        self.validated = Some(key);
        Ok(())
    }
}
static HELPER: OnceLock<parking_lot::Mutex<Cache>> = OnceLock::new();
fn standalone(path: &Path) -> Result<bool> {
    let macos = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("PEER_IMAGE_LAYOUT_INVALID"))?;
    let contents = macos
        .parent()
        .ok_or_else(|| anyhow::anyhow!("PEER_IMAGE_LAYOUT_INVALID"))?;
    fn names(path: &Path) -> Result<Vec<std::ffi::OsString>> {
        let mut names = std::fs::read_dir(path)?
            .map(|e| Ok(e?.file_name()))
            .collect::<Result<Vec<_>>>()?;
        names.sort();
        Ok(names)
    }
    // A resource-bearing App/bundle cannot be identified by its executable hash.
    // The installed helper contract has exactly this standalone Mach-O layout.
    Ok(names(contents)? == [std::ffi::OsString::from("MacOS")]
        && names(macos)? == [std::ffi::OsString::from("ppvpn-service")])
}
pub fn verify(path: &Path, identifier: &str, requirement: &SecRequirement) -> Result<()> {
    if path != Path::new(super::broker::IMAGE) || !standalone(path)? {
        return crate::mac_identity::check_file(path, requirement);
    }
    super::mac_peer::secure_installed_image(path)?;
    let key = Key {
        generation: Generation::read(path)?,
        identifier: identifier.into(),
        team: crate::mac_identity::team_id().into(),
    };
    HELPER.get_or_init(Default::default).lock().verify(key, || {
        super::native::timed("image-full-static-validation", || {
            crate::mac_identity::check_file(path, requirement)
        })?;
        super::mac_peer::secure_installed_image(path)?;
        if !standalone(path)? {
            bail!("PEER_IMAGE_GENERATION_CHANGED");
        }
        Generation::read(path)
    })
}
pub fn prevalidate_helper() -> Result<()> {
    let path = Path::new(super::broker::IMAGE);
    super::mac_peer::secure_installed_image(path)?;
    let requirement = crate::mac_identity::requirement(
        crate::mac_identity::SERVICE_IDENTIFIER,
        crate::mac_identity::team_id(),
    )?;
    verify(path, crate::mac_identity::SERVICE_IDENTIFIER, &requirement)
}
fn after_validation<T>(
    validate: impl FnOnce() -> Result<()>,
    expose: impl FnOnce() -> Result<T>,
) -> Result<T> {
    validate()?;
    expose()
}
pub fn before_exposure<T>(expose: impl FnOnce() -> Result<T>) -> Result<T> {
    after_validation(prevalidate_helper, expose)
}
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestCache(parking_lot::Mutex<Cache>);
#[cfg(test)]
impl TestCache {
    pub(crate) fn verify(
        &self,
        path: &Path,
        identifier: &str,
        full: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let key = Key {
            generation: Generation::read(path)?,
            identifier: identifier.into(),
            team: crate::mac_identity::team_id().into(),
        };
        self.0.lock().verify(key, || {
            full()?;
            Generation::read(path)
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn temporary_directory() -> TestDirectory {
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "pv-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        std::fs::create_dir(&path).unwrap();
        TestDirectory(path)
    }
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    };
    fn key(path: &Path) -> Key {
        Key {
            generation: Generation::read(path).unwrap(),
            identifier: "service".into(),
            team: "TEAM".into(),
        }
    }
    #[test]
    fn exact_bytes_policy_and_protected_generation_are_required() {
        let dir = temporary_directory();
        let path = dir.path().join("image");
        std::fs::write(&path, b"signed image").unwrap();
        let original = key(&path);
        let mut cache = Cache::default();
        let checks = AtomicUsize::new(0);
        cache
            .verify(original.clone(), || {
                checks.fetch_add(1, Ordering::SeqCst);
                Ok(original.generation.clone())
            })
            .unwrap();
        cache
            .verify(original.clone(), || {
                panic!("unchanged generation was already fully validated")
            })
            .unwrap();
        let mut wrong_id = original.clone();
        wrong_id.identifier.push('x');
        let mut wrong_team = original.clone();
        wrong_team.team.push('x');
        for changed in [wrong_id, wrong_team] {
            cache
                .verify(original.clone(), || Ok(original.generation.clone()))
                .unwrap();
            assert!(cache
                .verify(changed, || bail!("INVALID_REQUIREMENT"))
                .is_err());
            assert!(cache.validated.is_none());
        }
        std::fs::write(&path, b"tamper image").unwrap();
        let changed = key(&path);
        assert_ne!(original, changed);
        assert!(cache
            .verify(changed, || bail!("INVALID_SIGNATURE"))
            .is_err());
        assert!(cache.validated.is_none());
        assert_eq!(checks.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn replacement_permissions_and_ancestry_invalidate_generation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temporary_directory();
        let path = dir.path().join("image");
        std::fs::write(&path, b"same bytes").unwrap();
        let original = key(&path);
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, b"same bytes").unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_ne!(original, key(&path));
        let previous = key(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_ne!(previous, key(&path));
        let previous = key(&path);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        assert_ne!(previous, key(&path));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[test]
    fn cold_client_validation_cannot_consume_a_warm_brokers_hello_budget() {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::time::Duration;
        let dir = temporary_directory();
        let socket = dir.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let client = std::thread::spawn(move || {
            after_validation(
                || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                },
                || {
                    let mut stream = UnixStream::connect(socket)?;
                    stream.write_all(b"hello")?;
                    let mut ack = [0];
                    stream.read_exact(&mut ack)?;
                    Ok(())
                },
            )
        });
        entered_rx.recv().unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        release_tx.send(()).unwrap();
        listener.set_nonblocking(false).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(600)))
            .unwrap();
        let mut hello = [0; 5];
        stream.read_exact(&mut hello).unwrap();
        assert_eq!(&hello, b"hello");
        stream.write_all(b"!").unwrap();
        client.join().unwrap().unwrap();
    }
    #[test]
    fn resource_bearing_layouts_cannot_reuse_an_executable_only_signature() {
        let dir = temporary_directory();
        let contents = dir.path().join("Contents");
        let macos = contents.join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let image = macos.join("ppvpn-service");
        std::fs::write(&image, b"signed").unwrap();
        assert!(standalone(&image).unwrap());
        let info = contents.join("Info.plist");
        std::fs::write(&info, b"bundle metadata").unwrap();
        assert!(!standalone(&image).unwrap());
        std::fs::remove_file(info).unwrap();
        std::fs::write(macos.join("resource"), b"extra bytes").unwrap();
        assert!(!standalone(&image).unwrap());
    }
    #[test]
    fn unrelated_siblings_do_not_expire_a_verified_protected_image() {
        let dir = temporary_directory();
        let path = dir.path().join("image");
        std::fs::write(&path, b"signed").unwrap();
        let before = key(&path);
        let mut cache = Cache::default();
        cache
            .verify(before.clone(), || Ok(before.generation.clone()))
            .unwrap();
        std::fs::write(dir.path().join("unrelated-helper"), b"other image").unwrap();
        assert_eq!(before, key(&path));
        cache
            .verify(key(&path), || {
                panic!("unrelated sibling must not restart slow validation")
            })
            .unwrap();
    }
    #[test]
    fn full_file_hash_detects_same_length_tampering_and_symlinks_are_refused() {
        let dir = temporary_directory();
        let path = dir.path().join("image");
        std::fs::write(&path, b"signed").unwrap();
        let before = key(&path);
        std::fs::write(&path, b"tamper").unwrap();
        let after = key(&path);
        assert_ne!(before.generation.sha256, after.generation.sha256);
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Generation::read(&link).is_err());
        let hard = dir.path().join("hard");
        std::fs::hard_link(&path, &hard).unwrap();
        assert!(Generation::read(&path).is_err());
    }
    #[test]
    fn failed_or_changed_validation_is_never_published() {
        let dir = temporary_directory();
        let path = dir.path().join("image");
        std::fs::write(&path, b"signed").unwrap();
        let before = key(&path);
        let mut cache = Cache::default();
        assert!(cache
            .verify(before, || {
                std::fs::write(&path, b"changed")?;
                Generation::read(&path)
            })
            .is_err());
        assert!(cache.validated.is_none());
    }
    #[test]
    fn cold_validation_barrier_prevents_exposure_and_failure_stays_closed() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let exposed = Arc::new(AtomicUsize::new(0));
        let result = exposed.clone();
        let task = std::thread::spawn(move || {
            after_validation(
                || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                },
                || {
                    result.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            )
        });
        entered_rx.recv().unwrap();
        assert_eq!(exposed.load(Ordering::SeqCst), 0);
        release_tx.send(()).unwrap();
        task.join().unwrap().unwrap();
        assert_eq!(exposed.load(Ordering::SeqCst), 1);
        assert!(after_validation(
            || bail!("INVALID_SIGNATURE"),
            || {
                exposed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        )
        .is_err());
        assert_eq!(exposed.load(Ordering::SeqCst), 1);
    }
}
