//! Go's internal/rulesets tests, ported. The rule sets are served over TLS
//! from 127.0.0.1 with a certificate made for the run; the .srs files are
//! testdata/gen.go's, written by the sing-box the Go core pins.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use btls::asn1::Asn1Time;
use btls::bn::BigNum;
use btls::ec::{EcGroup, EcKey};
use btls::hash::MessageDigest;
use btls::nid::Nid;
use btls::pkey::{PKey, Private};
use btls::ssl::{Ssl, SslAcceptor, SslMethod};
use btls::x509::extension::{BasicConstraints, SubjectAlternativeName};
use btls::x509::{X509Name, X509NameBuilder, X509};
use chrono::TimeZone;
use sail::transport::tls::BoringConnection;
use sail::transport::tls_stream::TlsStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::*;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/rulesets/testdata")
            .join(name),
    )
    .unwrap()
}

/// The certificate authority the downloads trust (PEM) and the server's
/// acceptor, whose certificate it issued for 127.0.0.1.
fn tls() -> &'static (String, SslAcceptor) {
    static TLS: OnceLock<(String, SslAcceptor)> = OnceLock::new();
    TLS.get_or_init(|| {
        fn key() -> PKey<Private> {
            let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
            PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap()
        }
        fn name(cn: &str) -> X509Name {
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", cn).unwrap();
            name.build()
        }
        fn cert(
            serial: u32,
            subject: &str,
            key: &PKey<Private>,
            issuer: Option<(&X509, &PKey<Private>)>,
        ) -> X509 {
            let mut builder = X509::builder().unwrap();
            builder.set_version(2).unwrap();
            let serial = BigNum::from_u32(serial).unwrap().to_asn1_integer().unwrap();
            builder.set_serial_number(&serial).unwrap();
            builder.set_subject_name(&name(subject)).unwrap();
            builder.set_pubkey(key).unwrap();
            builder
                .set_not_before(&Asn1Time::days_from_now(0).unwrap())
                .unwrap();
            builder
                .set_not_after(&Asn1Time::days_from_now(2).unwrap())
                .unwrap();
            let signer = match issuer {
                Some((ca, ca_key)) => {
                    builder.set_issuer_name(ca.subject_name()).unwrap();
                    let san = SubjectAlternativeName::new()
                        .ip("127.0.0.1")
                        .build(&builder.x509v3_context(Some(&**ca), None))
                        .unwrap();
                    builder.append_extension(&san).unwrap();
                    ca_key
                }
                None => {
                    builder.set_issuer_name(&name(subject)).unwrap();
                    let ca = BasicConstraints::new().critical().ca().build().unwrap();
                    builder.append_extension(&ca).unwrap();
                    key
                }
            };
            builder.sign(signer, MessageDigest::sha256()).unwrap();
            builder.build()
        }
        let ca_key = key();
        let ca = cert(1, "rulesets test ca", &ca_key, None);
        let leaf_key = key();
        let leaf = cert(2, "127.0.0.1", &leaf_key, Some((&ca, &ca_key)));
        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&leaf_key).unwrap();
        acceptor.set_certificate(&leaf).unwrap();
        let pem = String::from_utf8(ca.to_pem().unwrap()).unwrap();
        (pem, acceptor.build())
    })
}

struct Served {
    body: Vec<u8>,
    fail: bool,
}

/// Serves one .srs with ETag = sha256 and honours If-None-Match, or
/// redirects every request elsewhere.
struct RuleSetServer {
    host: String,
    served: Mutex<Served>,
    redirect: Option<String>,
    requests: AtomicU32,
    not_modified: AtomicU32,
}

impl RuleSetServer {
    async fn start(body: Vec<u8>) -> Arc<Self> {
        Self::listen(body, None).await
    }

    async fn redirecting(to: String) -> Arc<Self> {
        Self::listen(Vec::new(), Some(to)).await
    }

    async fn listen(body: Vec<u8>, redirect: Option<String>) -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = Arc::new(RuleSetServer {
            host: listener.local_addr().unwrap().to_string(),
            served: Mutex::new(Served { body, fail: false }),
            redirect,
            requests: AtomicU32::new(0),
            not_modified: AtomicU32::new(0),
        });
        let serving = server.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let server = serving.clone();
                tokio::spawn(async move {
                    let ssl = Ssl::new(tls().1.context()).unwrap();
                    let conn = BoringConnection::server(ssl).unwrap();
                    let mut stream = TlsStream::new(conn, stream, None);
                    if stream.handshake().await.is_ok() {
                        let _ = server.respond(&mut stream).await;
                    }
                });
            }
        });
        server
    }

    async fn respond<S>(&self, stream: &mut S) -> std::io::Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let mut head = Vec::new();
        let mut buf = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            head.extend_from_slice(&buf[..n]);
        }
        self.requests.fetch_add(1, Ordering::SeqCst);
        let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
        let if_none_match = head
            .lines()
            .find_map(|line| line.strip_prefix("if-none-match:"))
            .map(|v| v.trim().to_string());
        let (status, headers, body) = if let Some(to) = &self.redirect {
            ("302 Found", format!("Location: {to}\r\n"), Vec::new())
        } else {
            let (body, fail) = {
                let served = self.served.lock().unwrap();
                (served.body.clone(), served.fail)
            };
            let etag = format!("\"{}\"", sha256_hex(&body));
            if fail {
                (
                    "503 Service Unavailable",
                    String::new(),
                    b"unavailable".to_vec(),
                )
            } else if if_none_match.as_deref() == Some(etag.as_str()) {
                self.not_modified.fetch_add(1, Ordering::SeqCst);
                ("304 Not Modified", format!("ETag: {etag}\r\n"), Vec::new())
            } else {
                (
                    "200 OK",
                    format!("ETag: {etag}\r\nContent-Type: application/octet-stream\r\n"),
                    body,
                )
            }
        };
        let mut response = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            if status.starts_with("304") {
                0
            } else {
                body.len()
            }
        )
        .into_bytes();
        response.extend_from_slice(&body);
        stream.write_all(&response).await?;
        stream.flush().await?;
        stream.shutdown().await
    }

    fn set(&self, body: Vec<u8>, fail: bool) {
        *self.served.lock().unwrap() = Served { body, fail };
    }

    fn url(&self, path: &str) -> String {
        format!("https://{}{path}", self.host)
    }

    fn rule_set(&self, id: &str, body: &[u8]) -> RuleSet {
        RuleSet {
            id: id.into(),
            url: self.url(&format!("/api/v1/proxy-profile/rule-sets/{id}.srs")),
            sha256: sha256_hex(body),
            update_interval_seconds: 3600,
        }
    }

    fn requests(&self) -> u32 {
        self.requests.load(Ordering::SeqCst)
    }
}

/// A clock the test moves.
#[derive(Clone)]
struct TestClock(Arc<Mutex<DateTime<Utc>>>);

impl TestClock {
    fn new() -> Self {
        TestClock(Arc::new(Mutex::new(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        )))
    }
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
    fn add(&self, d: Duration) {
        let mut now = self.0.lock().unwrap();
        *now = after(*now, d);
    }
}

fn manager(dir: &Path, opts: Options) -> Manager {
    Manager::new(Options {
        dir: Some(dir.to_path_buf()),
        trust_pem: Some(tls().0.clone()),
        ..opts
    })
}

const DOWNLOAD: Option<Duration> = Some(PREPARE_TIMEOUT);

fn status_of(statuses: &[Status], id: &str) -> Status {
    statuses
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("no status for {id} in {statuses:?}"))
        .clone()
}

/// Waits up to five seconds for `done`.
async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn prepare_downloads_verifies_and_reuses_cache() {
    let body = fixture("domains.srs");
    let server = RuleSetServer::start(body.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let set = server.rule_set("cn-site", &body);
    let hosts = [server.host.clone()];

    let manager = manager(dir.path(), Options::default());
    let snapshot = manager
        .prepare(std::slice::from_ref(&set), &hosts, DOWNLOAD)
        .await;
    let files = snapshot.files();
    let want = dir
        .path()
        .join("cn-site.srs")
        .to_string_lossy()
        .into_owned();
    match files.get("cn-site") {
        Some(file) if file.path == want && file.mirror_dns => {}
        other => panic!("files: {other:?}"),
    }
    assert_eq!(server.requests(), 1);
    assert_eq!(std::fs::read(dir.path().join("cn-site.srs")).unwrap(), body);
    manager.activate(snapshot);
    let status = status_of(&manager.statuses(), "cn-site");
    assert!(
        status.state == State::Ready && status.updated_at.is_some() && status.error.is_empty(),
        "status: {status:?}"
    );

    // A restarted core uses the matching cached copy without the network.
    server.set(Vec::new(), true);
    let restarted = self::manager(dir.path(), Options::default());
    let snapshot = restarted.prepare(&[set], &hosts, DOWNLOAD).await;
    assert!(snapshot.files().contains_key("cn-site"), "cache not reused");
    assert_eq!(server.requests(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn prepare_rejects_digest_mismatch_and_foreign_hosts() {
    let body = fixture("domains.srs");
    let server = RuleSetServer::start(fixture("domains-other.srs")).await;
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path(), Options::default());
    let set = server.rule_set("cn-site", &body);

    let snapshot = manager
        .prepare(
            std::slice::from_ref(&set),
            std::slice::from_ref(&server.host),
            DOWNLOAD,
        )
        .await;
    manager.activate(snapshot);
    let status = status_of(&manager.statuses(), "cn-site");
    assert!(
        status.state == State::Unavailable && status.error == SHA256_MISMATCH,
        "status: {status:?}"
    );
    assert!(
        !dir.path().join("cn-site.srs").exists(),
        "mismatched download was written"
    );

    let requests = server.requests();
    let snapshot = manager.prepare(&[set], &[], DOWNLOAD).await;
    manager.activate(snapshot);
    let status = status_of(&manager.statuses(), "cn-site");
    assert!(
        status.state == State::Unavailable && status.error == HOST_NOT_PINNED,
        "status: {status:?}"
    );
    assert_eq!(
        server.requests(),
        requests,
        "an unpinned host was contacted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prepare_rejects_invalid_rule_set() {
    let body = b"not a rule set".to_vec();
    let server = RuleSetServer::start(body.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path(), Options::default());
    let snapshot = manager
        .prepare(
            &[server.rule_set("bad", &body)],
            std::slice::from_ref(&server.host),
            DOWNLOAD,
        )
        .await;
    manager.activate(snapshot);
    let status = status_of(&manager.statuses(), "bad");
    assert!(
        status.state == State::Unavailable && status.error == INVALID,
        "status: {status:?}"
    );
}

/// A new profile version that cannot be fetched keeps the last good copy.
#[tokio::test(flavor = "multi_thread")]
async fn failed_update_keeps_last_good_copy() {
    let old_body = fixture("domains.srs");
    let new_body = fixture("domains-more.srs");
    let server = RuleSetServer::start(old_body.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let hosts = [server.host.clone()];
    let manager = manager(dir.path(), Options::default());
    let snapshot = manager
        .prepare(&[server.rule_set("cn-site", &old_body)], &hosts, DOWNLOAD)
        .await;
    manager.activate(snapshot);

    server.set(Vec::new(), true);
    let snapshot = manager
        .prepare(&[server.rule_set("cn-site", &new_body)], &hosts, DOWNLOAD)
        .await;
    let want = dir
        .path()
        .join("cn-site.srs")
        .to_string_lossy()
        .into_owned();
    assert!(
        snapshot
            .files()
            .get("cn-site")
            .is_some_and(|f| f.path == want),
        "stale copy not used: {:?}",
        snapshot.files()
    );
    manager.activate(snapshot);
    let status = status_of(&manager.statuses(), "cn-site");
    assert!(
        status.state == State::Stale && status.error == HTTP_STATUS,
        "status: {status:?}"
    );
    assert_eq!(
        std::fs::read(dir.path().join("cn-site.srs")).unwrap(),
        old_body,
        "last good copy was replaced"
    );
}

/// A ready set is refreshed on its interval with If-None-Match and stays
/// ready on 304.
#[tokio::test(flavor = "multi_thread")]
async fn refresh_uses_etag() {
    let body = fixture("domains.srs");
    let server = RuleSetServer::start(body.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let now = clock.clone();
    let manager = manager(
        dir.path(),
        Options {
            now: Arc::new(move || now.now()),
            ..Options::default()
        },
    );
    let snapshot = manager
        .prepare(
            &[server.rule_set("cn-site", &body)],
            std::slice::from_ref(&server.host),
            DOWNLOAD,
        )
        .await;
    clock.add(Duration::from_secs(2 * 3600));
    manager.activate(snapshot);
    eventually("a conditional refresh", || {
        server.not_modified.load(Ordering::SeqCst) > 0
    })
    .await;
    eventually("ready after 304", || {
        let status = status_of(&manager.statuses(), "cn-site");
        status.state == State::Ready && status.updated_at == Some(clock.now())
    })
    .await;
}

/// A set that was never downloaded is retried; once it arrives the manager
/// asks for a rebuild so the skipped rules take effect.
#[tokio::test(flavor = "multi_thread")]
async fn recovery_triggers_rebuild() {
    let body = fixture("cidrs.srs");
    let server = RuleSetServer::start(body.clone()).await;
    server.set(body.clone(), true);
    let dir = tempfile::tempdir().unwrap();
    let (rebuild_tx, mut rebuilds) = mpsc::unbounded_channel();
    let (state_tx, mut states) = mpsc::unbounded_channel();
    let manager = manager(
        dir.path(),
        Options {
            retry_min: Duration::from_millis(10),
            on_rebuild: Some(Arc::new(move || {
                let _ = rebuild_tx.send(());
            })),
            on_state: Some(Arc::new(move |s| {
                let _ = state_tx.send(s);
            })),
            ..Options::default()
        },
    );
    let set = server.rule_set("cn-ip", &body);
    let hosts = [server.host.clone()];
    let snapshot = manager
        .prepare(std::slice::from_ref(&set), &hosts, DOWNLOAD)
        .await;
    assert!(snapshot.files().is_empty(), "files: {:?}", snapshot.files());
    manager.activate(snapshot);
    let first = states.recv().await.unwrap();
    assert!(
        first.state == State::Unavailable && first.error == HTTP_STATUS,
        "first state: {first:?}"
    );
    server.set(body, false);
    tokio::time::timeout(Duration::from_secs(5), rebuilds.recv())
        .await
        .expect("no rebuild after recovery");
    let recovered = states.recv().await.unwrap();
    assert_eq!(
        recovered.state,
        State::Ready,
        "recovered state: {recovered:?}"
    );
    // A rebuild prepares again without the network and finds the copy.
    let snapshot = manager.prepare(&[set], &hosts, None).await;
    assert!(
        snapshot.files().get("cn-ip").is_some_and(|f| !f.mirror_dns),
        "files after recovery: {:?}",
        snapshot.files()
    );
}

#[test]
fn inspect_classifies_dns_mirroring() {
    for (name, want) in [
        ("domains.srs", true),
        ("cidrs.srs", false),
        ("mixed.srs", false),
    ] {
        assert_eq!(srs::inspect(&fixture(name)), Ok(want), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_refuse_redirects() {
    let body = fixture("domains.srs");
    let target = RuleSetServer::start(body.clone()).await;
    let redirect = RuleSetServer::redirecting(target.url("/x.srs")).await;
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path(), Options::default());
    let set = RuleSet {
        id: "cn-site".into(),
        url: redirect.url("/cn-site.srs"),
        sha256: sha256_hex(&body),
        update_interval_seconds: 0,
    };
    let snapshot = manager
        .prepare(&[set], std::slice::from_ref(&redirect.host), DOWNLOAD)
        .await;
    manager.activate(snapshot);
    let status = status_of(&manager.statuses(), "cn-site");
    assert!(
        status.state == State::Unavailable && status.error == HTTP_STATUS,
        "status: {status:?}"
    );
    assert_eq!(target.requests(), 0);
}

/// When one set recovers, every other set that is not ready is retried at
/// once (not on its own, possibly long, backoff), the downloads run
/// concurrently, and the configuration is rebuilt once for the whole round.
#[tokio::test(flavor = "multi_thread")]
async fn recovery_sweeps_all_sets_and_rebuilds_once() {
    let body = fixture("cidrs.srs");
    let server = RuleSetServer::start(body.clone()).await;
    server.set(body.clone(), true);
    let dir = tempfile::tempdir().unwrap();
    let rebuilds = Arc::new(AtomicU32::new(0));
    let counted = rebuilds.clone();
    let manager = manager(
        dir.path(),
        Options {
            retry_min: Duration::from_secs(3600), // nothing retries on its own here
            on_rebuild: Some(Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            })),
            ..Options::default()
        },
    );
    let sets: Vec<RuleSet> = (0..15)
        .map(|i| server.rule_set(&format!("set-{i:02}"), &body))
        .collect();
    let mut snapshot = manager
        .prepare(&sets, std::slice::from_ref(&server.host), DOWNLOAD)
        .await;
    for e in &snapshot.entries {
        let status = e.status();
        assert!(
            status.state == State::Unavailable
                && status.failures == 1
                && status.next_retry_at.is_some(),
            "failed set status: {status:?}"
        );
    }
    // Staggered backoffs, as after a long outage: one set is due now, the
    // others an hour from now.
    let now = Utc::now();
    for (i, e) in snapshot.entries.iter_mut().enumerate() {
        e.due = Some(if i == 0 {
            now
        } else {
            after(now, Duration::from_secs(3600))
        });
    }
    server.set(body, false);
    manager.activate(snapshot);
    eventually("every set ready and a rebuild", || {
        let ready = manager
            .statuses()
            .iter()
            .filter(|s| s.state == State::Ready)
            .count();
        ready == sets.len() && rebuilds.load(Ordering::SeqCst) > 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        rebuilds.load(Ordering::SeqCst),
        1,
        "rebuilds for one recovery"
    );
    for status in manager.statuses() {
        assert!(
            status.failures == 0 && status.next_retry_at.is_none(),
            "ready set status: {status:?}"
        );
    }
}

#[test]
fn path_stays_inside_dir() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(Options {
        dir: Some(dir.path().to_path_buf()),
        ..Options::default()
    });
    assert_eq!(
        manager.inner.path("geosite-cn"),
        Some(dir.path().join("geosite-cn.srs"))
    );
    for id in ["../escape", "a/b", "..", "/abs", "a\\b"] {
        if let Some(path) = manager.inner.path(id) {
            assert_eq!(path.parent(), Some(dir.path()), "{id} escapes");
        }
    }
    for id in ["../escape", "a/b"] {
        assert_eq!(manager.inner.path(id), None, "{id} accepted");
    }
}

/// Activation reports each set's state once, the cached files of sets the
/// profile dropped go, and the public status and event carry the state.
#[tokio::test(flavor = "multi_thread")]
async fn activation_reports_changes_and_prunes() {
    let body = fixture("domains.srs");
    let server = RuleSetServer::start(body.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (state_tx, mut states) = mpsc::unbounded_channel();
    let manager = manager(
        dir.path(),
        Options {
            on_state: Some(Arc::new(move |s| {
                let _ = state_tx.send(s);
            })),
            ..Options::default()
        },
    );
    let hosts = [server.host.clone()];
    let a = server.rule_set("a", &body);
    let b = server.rule_set("b", &body);
    let snapshot = manager.prepare(&[a.clone(), b], &hosts, DOWNLOAD).await;
    assert_eq!(snapshot.counts(), (2, 0, 0));
    std::fs::write(dir.path().join(".tmp-left-over"), b"x").unwrap();
    manager.activate(snapshot);
    assert_eq!(states.recv().await.unwrap().id, "a");
    assert_eq!(states.recv().await.unwrap().id, "b");

    let snapshot = manager.prepare(&[a], &hosts, DOWNLOAD).await;
    manager.activate(snapshot);
    assert!(
        states.try_recv().is_err(),
        "an unchanged state was reported"
    );
    assert!(!dir.path().join("b.srs").exists(), "a dropped set was kept");
    assert!(!dir.path().join(".tmp-left-over").exists());
    assert!(dir.path().join("a.srs").exists());

    let status = status_of(&manager.statuses(), "a");
    let public = status.public();
    assert_eq!((public.id.as_str(), public.state.as_str()), ("a", "ready"));
    let at = Utc::now();
    match status.event(at) {
        Event::RuleSetChanged {
            rule_set_id,
            message,
            code,
            ..
        } => assert_eq!(
            (rule_set_id.as_str(), message.as_str(), code.as_str()),
            ("a", "ready", "")
        ),
        other => panic!("event: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn without_storage_every_set_is_unavailable() {
    let manager = Manager::new(Options::default());
    let set = RuleSet {
        id: "cn-site".into(),
        url: "https://rules.example.com/cn-site.srs".into(),
        sha256: "0".repeat(64),
        update_interval_seconds: 0,
    };
    let snapshot = manager.prepare(&[set], &[], DOWNLOAD).await;
    assert!(snapshot.files().is_empty());
    manager.activate(snapshot);
    let status = status_of(&manager.statuses(), "cn-site");
    assert!(
        status.state == State::Unavailable
            && status.error == STORAGE_UNAVAILABLE
            && status.next_retry_at.is_none(),
        "status: {status:?}"
    );
}

#[test]
fn backoff_doubles_up_to_its_bounds() {
    let manager = Manager::new(Options {
        retry_min: Duration::from_secs(5),
        retry_max: Duration::from_secs(60),
        ..Options::default()
    });
    let now = Utc::now();
    let mut e = Entry::new(RuleSet {
        update_interval_seconds: 3600,
        ..RuleSet::default()
    });
    e.err = HTTP_STATUS;
    for (failures, want) in [(1, 5), (2, 10), (3, 20), (4, 40), (5, 60), (9, 60)] {
        e.failures = failures;
        assert_eq!(
            manager.inner.next_due(&e, now),
            after(now, Duration::from_secs(want)),
            "{failures} failures"
        );
    }
    e.state = State::Ready;
    assert_eq!(
        manager.inner.next_due(&e, now),
        after(now, Duration::from_secs(3600))
    );
}
