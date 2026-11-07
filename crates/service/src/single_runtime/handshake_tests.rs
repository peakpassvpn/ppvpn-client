//! Isolated protocol regressions through the production host request, broker
//! transact and audit-token verification flow. Only OS signing/path backends are
//! controlled here; signed native acceptance separately exercises those APIs.
use super::{broker, host, image_verification::TestCache, mac_peer};
use anyhow::{bail, Result};
use security_framework::os::macos::code_signing::SecRequirement;
use std::{
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "pv-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        std::fs::create_dir(&p).unwrap();
        std::fs::write(p.join("image"), b"signature fixture").unwrap();
        Self(p)
    }
    fn image(&self) -> PathBuf {
        self.0.join("image")
    }
    fn listener(&self) -> (UnixListener, PathBuf) {
        let p = self.0.join("s");
        (UnixListener::bind(&p).unwrap(), p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Gate {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
struct Checks {
    path: PathBuf,
    cache: TestCache,
    legacy: bool,
    full: AtomicUsize,
    live: AtomicUsize,
    protected: AtomicUsize,
    paths: AtomicUsize,
    gate: parking_lot::Mutex<Option<Gate>>,
    invalid: AtomicBool,
    reject_live: AtomicUsize,
}
impl Checks {
    fn new(path: PathBuf, legacy: bool) -> Arc<Self> {
        Arc::new(Self {
            path,
            cache: TestCache::default(),
            legacy,
            full: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
            protected: AtomicUsize::new(0),
            paths: AtomicUsize::new(0),
            gate: parking_lot::Mutex::new(None),
            invalid: AtomicBool::new(false),
            reject_live: AtomicUsize::new(0),
        })
    }
    fn full(&self) -> Result<()> {
        self.full.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = self.gate.lock().take() {
            gate.entered.send(()).unwrap();
            gate.release.recv().unwrap();
        }
        if self.invalid.load(Ordering::SeqCst) {
            bail!("CONTROLLED_INVALID_STATIC_SIGNATURE");
        }
        Ok(())
    }
    fn prevalidate(&self) -> Result<()> {
        self.cache
            .verify(&self.path, crate::mac_identity::SERVICE_IDENTIFIER, || {
                self.full()
            })
    }
    fn arm(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (tx, entered) = mpsc::channel();
        let (release, rx) = mpsc::channel();
        *self.gate.lock() = Some(Gate {
            entered: tx,
            release: rx,
        });
        (entered, release)
    }
    fn verify(&self, stream: &UnixStream) -> Result<()> {
        mac_peer::verify_test(
            stream,
            unsafe { libc::geteuid() },
            crate::mac_identity::SERVICE_IDENTIFIER,
            &self.path,
            self,
        )
        .map(|_| ())
    }
}
impl mac_peer::IdentityChecks for Checks {
    fn image(&self, image: &Path) -> Result<()> {
        self.protected.fetch_add(1, Ordering::SeqCst);
        assert_eq!(image, self.path);
        Ok(())
    }
    fn process(&self, token: &[u8], _: &SecRequirement) -> Result<()> {
        assert_eq!(token.len(), 32);
        assert_eq!(&token[20..24], &std::process::id().to_ne_bytes());
        let number = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        if self.reject_live.load(Ordering::SeqCst) == number {
            bail!("CONTROLLED_INVALID_LIVE_PROCESS");
        }
        Ok(())
    }
    fn file(&self, path: &Path, id: &str, _: &SecRequirement) -> Result<()> {
        if self.legacy {
            self.full()
        } else {
            self.cache.verify(path, id, || self.full())
        }
    }
    fn process_path(&self, pid: u32) -> Result<PathBuf> {
        assert_eq!(pid, std::process::id());
        self.paths.fetch_add(1, Ordering::SeqCst);
        Ok(self.path.clone())
    }
}
fn grant() -> broker::Grant {
    broker::Grant {
        token: uuid::Uuid::new_v4().to_string(),
        lease: ppvpn_core_wire::CaptureLease {
            owner: ppvpn_core_wire::ForwarderIdentity {
                instance: "handshake-regression".into(),
                generation: 1,
            },
            attachment_epoch: 1,
        },
        settings: None,
    }
}
fn serve(
    listener: UnixListener,
    checks: Arc<Checks>,
) -> std::thread::JoinHandle<(Result<()>, bool)> {
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        broker::transact_test(&mut stream, |stream, peer| {
            assert_eq!(
                stream.read_timeout().unwrap(),
                Some(Duration::from_millis(600))
            );
            assert_eq!(
                stream.write_timeout().unwrap(),
                Some(Duration::from_millis(600))
            );
            checks.verify(stream)?;
            Ok(peer)
        })
    })
}
fn request(path: PathBuf, checks: Arc<Checks>) -> (Result<()>, bool, Duration) {
    let mut attempted = false;
    let started = Instant::now();
    let result = host::request_test(
        &grant(),
        &mut attempted,
        || checks.prevalidate(),
        || Ok(UnixStream::connect(path)?),
        |stream| {
            assert_eq!(
                stream.read_timeout().unwrap(),
                Some(Duration::from_millis(2400))
            );
            assert_eq!(
                stream.write_timeout().unwrap(),
                Some(Duration::from_millis(600))
            );
            checks.verify(stream)
        },
    );
    (result, attempted, started.elapsed())
}
// The broker's production root-role assertion checks the actual process-global
// forwarding registry. Run this suite alone: no unrelated Engine may coexist.
#[test]
#[ignore = "isolated root-role registry: cargo test -p ppvpn-service handshake_regression -- --ignored --test-threads=1"]
fn handshake_regression_legacy_static_barrier_times_out_but_warm_generation_passes() {
    for legacy in [true, false] {
        let fixture = Fixture::new();
        let (listener, path) = fixture.listener();
        let root = Checks::new(fixture.image(), legacy);
        let client = Checks::new(fixture.image(), false);
        root.prevalidate().unwrap();
        client.prevalidate().unwrap();
        let (entered, release) = root.arm();
        let server = serve(listener, root.clone());
        let client_checks = client.clone();
        let call = std::thread::spawn(move || request(path, client_checks));
        if legacy {
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            let (result, attempted, elapsed) = call.join().unwrap();
            let error = result.unwrap_err();
            let io = error.downcast_ref::<std::io::Error>().unwrap();
            assert!(matches!(
                io.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ));
            assert!(!attempted);
            assert!(elapsed >= Duration::from_millis(2300));
            release.send(()).unwrap();
            let (result, occupied) = server.join().unwrap();
            assert!(result.is_err());
            assert!(!occupied);
            assert_eq!(root.full.load(Ordering::SeqCst), 2);
        } else {
            let (result, attempted, elapsed) = call.join().unwrap();
            result.unwrap();
            assert!(attempted);
            assert!(elapsed < Duration::from_millis(2400));
            assert!(matches!(entered.try_recv(), Err(mpsc::TryRecvError::Empty)));
            let (result, occupied) = server.join().unwrap();
            result.unwrap();
            assert!(!occupied);
            assert_eq!(root.full.load(Ordering::SeqCst), 1);
            assert_eq!(root.live.load(Ordering::SeqCst), 2);
            assert_eq!(root.paths.load(Ordering::SeqCst), 1);
            assert_eq!(root.protected.load(Ordering::SeqCst), 1);
        }
        assert_eq!(client.live.load(Ordering::SeqCst), 2);
        assert_eq!(client.paths.load(Ordering::SeqCst), 1);
        assert_eq!(client.protected.load(Ordering::SeqCst), 1);
    }
}
#[test]
#[ignore = "isolated root-role registry: cargo test -p ppvpn-service handshake_regression -- --ignored --test-threads=1"]
fn handshake_regression_cold_client_finishes_validation_before_warm_broker_accepts() {
    let fixture = Fixture::new();
    let (listener, path) = fixture.listener();
    listener.set_nonblocking(true).unwrap();
    let root = Checks::new(fixture.image(), false);
    root.prevalidate().unwrap();
    let client = Checks::new(fixture.image(), false);
    let (entered, release) = client.arm();
    let client_checks = client.clone();
    let call = std::thread::spawn(move || request(path, client_checks));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    listener.set_nonblocking(false).unwrap();
    let server = serve(listener, root.clone());
    release.send(()).unwrap();
    let (result, attempted, _) = call.join().unwrap();
    result.unwrap();
    assert!(attempted);
    let (result, occupied) = server.join().unwrap();
    result.unwrap();
    assert!(!occupied);
    assert_eq!(client.full.load(Ordering::SeqCst), 1);
    assert_eq!(client.live.load(Ordering::SeqCst), 2);
    assert_eq!(root.live.load(Ordering::SeqCst), 2);
}
#[test]
#[ignore = "isolated root-role registry: cargo test -p ppvpn-service handshake_regression -- --ignored --test-threads=1"]
fn handshake_regression_tampered_generation_cannot_send_hello_or_allocate_grant() {
    let fixture = Fixture::new();
    let root = Checks::new(fixture.image(), false);
    let client = Checks::new(fixture.image(), false);
    root.prevalidate().unwrap();
    client.prevalidate().unwrap();
    std::fs::write(fixture.image(), b"tampered fixture!").unwrap();
    root.invalid.store(true, Ordering::SeqCst);
    // Local client validation is independently successful so the broker's
    // changed-generation check, not a client early error, is the negative path.
    client.prevalidate().unwrap();
    for _ in 0..2 {
        let (listener, path) = fixture.listener();
        let server = serve(listener, root.clone());
        let (result, attempted, _) = request(path.clone(), client.clone());
        assert!(result.is_err());
        assert!(!attempted);
        let (result, occupied) = server.join().unwrap();
        assert!(result.is_err());
        assert!(!occupied);
        std::fs::remove_file(path).unwrap();
    }
    assert_eq!(root.full.load(Ordering::SeqCst), 3);
    assert_eq!(root.live.load(Ordering::SeqCst), 2);
    assert_eq!(root.paths.load(Ordering::SeqCst), 0);
}

#[test]
#[ignore = "isolated root-role registry: cargo test -p ppvpn-service handshake_regression -- --ignored --test-threads=1"]
fn handshake_regression_warm_file_does_not_bypass_either_live_process_check() {
    for rejected in [1, 2] {
        let fixture = Fixture::new();
        let root = Checks::new(fixture.image(), false);
        let client = Checks::new(fixture.image(), false);
        root.prevalidate().unwrap();
        client.prevalidate().unwrap();
        root.reject_live.store(rejected, Ordering::SeqCst);
        let (listener, path) = fixture.listener();
        let server = serve(listener, root.clone());
        let (result, attempted, _) = request(path, client);
        assert!(result.is_err());
        assert!(!attempted);
        let (result, occupied) = server.join().unwrap();
        assert!(result.is_err());
        assert!(!occupied);
        assert_eq!(root.live.load(Ordering::SeqCst), rejected);
        assert_eq!(root.full.load(Ordering::SeqCst), 1);
        assert_eq!(root.paths.load(Ordering::SeqCst), rejected - 1);
    }
}
