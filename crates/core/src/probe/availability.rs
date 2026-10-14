//! Availability probes: a whole GET of the target through the node, timed as
//! `total_ms` (the node's connection, the target's answer, its body up to
//! 64 KiB). Go fetched through the node's local proxy user; here the request
//! goes through the node's outbound itself ([`Runtime::dial_tcp`]), which is
//! where that user leads. Redirects are followed, as Go's client does; TLS
//! is sail's (BoringSSL), checked against the system's roots, with Chrome's
//! ClientHello: Google's front ends closed connections that came with
//! BoringSSL's own from some exits, though they answered curl there (#342).
//!
//! Each step that fails has its own result code: the dial (CONNECT_FAILED,
//! TIMEOUT, or PROXY_REQUEST_FAILED when the outbound could not be used at
//! all), the TLS handshake (TLS_HANDSHAKE_FAILED, TLS_CERTIFICATE_INVALID),
//! the answer (CONNECTION_CLOSED before a whole response head,
//! RESPONSE_INVALID), the redirects (REDIRECT_LOOP), and the target itself
//! (TARGET_INVALID).

use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use sail::transport::tls::{BoringConnection, Fingerprint};
use sail::transport::tls_stream::TlsStream;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;
use url::{Host, Url};

use super::result_codes as rc;
use super::{require_online, timeout_of, Cancel, DefaultInterface};
use crate::error::{codes, Error};
use crate::event::Event;
use crate::runtime::{Runtime, RuntimeError, Target};
use crate::types::{AvailabilityResult, ProbeAvailabilityRequest};

/// A request's timeout when it gives none.
pub(crate) const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(10);
/// Go's client gives up after 10 redirects.
const MAX_REDIRECTS: usize = 10;
/// The most of a response's head read.
const MAX_HEAD: usize = 64 << 10;
/// The most of the final body read (and timed).
const MAX_BODY: usize = 64 << 10;
/// BoringSSL's X509_V_ERR_INVALID_CALL: what the verify result reads as
/// before a certificate was verified at all.
const NOT_VERIFIED: i32 = 65;

/// Fetches `request.target` through the node's outbound (`node_tags`, the
/// translation's node id to outbound tag). Offline it fails at once and
/// dials nothing. A probe that ran is a result, whatever its outcome, with
/// the AvailabilityProbed the Engine emits.
pub(crate) async fn probe_availability(
    runtime: &dyn Runtime,
    node_tags: &BTreeMap<String, String>,
    request: &ProbeAvailabilityRequest,
    network: DefaultInterface,
    cancel: &Cancel,
) -> Result<(AvailabilityResult, Event), Error> {
    probe_trusting(runtime, node_tags, request, network, cancel, None).await
}

/// [`probe_availability`], trusting `trust_pem` in place of the system's
/// roots when given (tests).
async fn probe_trusting(
    runtime: &dyn Runtime,
    node_tags: &BTreeMap<String, String>,
    request: &ProbeAvailabilityRequest,
    network: DefaultInterface,
    cancel: &Cancel,
    trust_pem: Option<&str>,
) -> Result<(AvailabilityResult, Event), Error> {
    require_online(network)?;
    let outbound = node_tags
        .get(&request.node_id)
        .ok_or_else(|| Error::invalid(codes::NODE_NOT_FOUND, "node_id", "node not found"))?;
    let timeout = timeout_of(request.timeout_ms, AVAILABILITY_TIMEOUT);
    let started = Instant::now();
    let deadline = started + timeout;
    let fetching = fetch(runtime, outbound, &request.target, deadline, trust_pem);
    let outcome = if cancel.is_cancelled() {
        Err(rc::CANCELED)
    } else {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(rc::CANCELED),
            outcome = tokio::time::timeout_at(deadline, fetching) => {
                outcome.unwrap_or(Err(rc::TIMEOUT))
            }
        }
    };
    let total = started.elapsed();
    let measured_at = Utc::now();
    let mut result = AvailabilityResult {
        node_id: request.node_id.clone(),
        total_ms: i64::try_from(total.as_millis()).unwrap_or(i64::MAX),
        success: false,
        http_status: 0,
        error_code: String::new(),
        measured_at,
    };
    match outcome {
        Ok(status) => {
            result.http_status = status;
            result.success = (200..400).contains(&status);
            if !result.success {
                result.error_code = rc::HTTP_STATUS.to_owned();
            }
        }
        Err(code) => result.error_code = code.to_owned(),
    }
    let event = Event::AvailabilityProbed {
        at: measured_at,
        node_id: result.node_id.clone(),
        message: if result.success {
            "success".to_owned()
        } else {
            result.error_code.clone()
        },
    };
    Ok((result, event))
}

/// The final response's status, redirects followed.
async fn fetch(
    runtime: &dyn Runtime,
    outbound: &str,
    target: &str,
    deadline: Instant,
    trust_pem: Option<&str>,
) -> Result<u16, &'static str> {
    let mut url = match Url::parse(target) {
        Ok(url) if url.has_host() && !url.scheme().is_empty() => url,
        _ => return Err(rc::TARGET_INVALID),
    };
    for _ in 0..=MAX_REDIRECTS {
        let (status, location) = get(runtime, outbound, &url, deadline, trust_pem).await?;
        match location {
            Some(location) if matches!(status, 301 | 302 | 303 | 307 | 308) => {
                url = url.join(&location).map_err(|_| rc::RESPONSE_INVALID)?;
            }
            _ => return Ok(status),
        }
    }
    Err(rc::REDIRECT_LOOP)
}

/// One GET: its status and, for a redirect, where to.
async fn get(
    runtime: &dyn Runtime,
    outbound: &str,
    url: &Url,
    deadline: Instant,
    trust_pem: Option<&str>,
) -> Result<(u16, Option<String>), &'static str> {
    let tls = match url.scheme() {
        "http" => false,
        "https" => true,
        // Where a redirect leads too: nothing this probe can fetch.
        _ => return Err(rc::TARGET_INVALID),
    };
    let port = url.port_or_known_default().ok_or(rc::TARGET_INVALID)?;
    let (to, server_name) = match url.host().ok_or(rc::TARGET_INVALID)? {
        Host::Domain(domain) => (Target::Domain(domain.to_owned(), port), domain.to_owned()),
        Host::Ipv4(ip) => (
            Target::Addr(SocketAddr::new(ip.into(), port)),
            ip.to_string(),
        ),
        Host::Ipv6(ip) => (
            Target::Addr(SocketAddr::new(ip.into(), port)),
            ip.to_string(),
        ),
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    let stream = runtime
        .dial_tcp(outbound, to, remaining)
        .await
        .map_err(|e| dial_code(&e))?;
    if !tls {
        return exchange(stream, url, deadline).await;
    }
    let client = crate::tls::client(&["http/1.1"], trust_pem, Some(Fingerprint::Chrome))
        .map_err(|_| rc::PROXY_REQUEST_FAILED)?;
    let conn = client
        .connection(&server_name, None)
        .map_err(|e| match e.kind() {
            io::ErrorKind::InvalidInput => rc::TARGET_INVALID,
            _ => rc::TLS_HANDSHAKE_FAILED,
        })?;
    let mut stream = TlsStream::new(conn, stream, None);
    if let Err(e) = stream.handshake().await {
        return Err(handshake_code(&e, stream.conn()));
    }
    exchange(stream, url, deadline).await
}

/// A failed dial through the outbound, by sail's error kind
/// (`sail::embed::ErrorKind::code`). A dial that was made and failed is
/// "failed", whether the name did not resolve or the target refused: sail
/// keeps only the text of why, so DNS_FAILED cannot be told apart here.
fn dial_code(error: &RuntimeError) -> &'static str {
    match error.code.as_str() {
        "timeout" => rc::TIMEOUT,
        "failed" | "io" => rc::CONNECT_FAILED,
        // No such outbound, the instance not running or stopping: the node
        // could not take the request at all.
        _ => rc::PROXY_REQUEST_FAILED,
    }
}

/// A failed handshake: the certificate's fault when BoringSSL verified one
/// and it failed (an unknown issuer, another name, expired), the
/// handshake's otherwise.
fn handshake_code(error: &io::Error, conn: &BoringConnection) -> &'static str {
    if error.kind() == io::ErrorKind::TimedOut {
        return rc::TIMEOUT;
    }
    match conn.ssl().verify_result() {
        Err(e) if e.as_raw() != NOT_VERIFIED => rc::TLS_CERTIFICATE_INVALID,
        _ => rc::TLS_HANDSHAKE_FAILED,
    }
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    url: &Url,
    deadline: Instant,
) -> Result<(u16, Option<String>), &'static str> {
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_owned(),
    };
    let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: ppvpn-core\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| io_code(&e))?;
    stream.flush().await.map_err(|e| io_code(&e))?;

    let mut buf = Vec::with_capacity(4096);
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() >= MAX_HEAD {
            return Err(rc::RESPONSE_INVALID);
        }
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.map_err(|e| io_code(&e))?;
        if n == 0 {
            tracing::info!("availability probe: closed before a whole response head");
            return Err(rc::CONNECTION_CLOSED);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| rc::RESPONSE_INVALID)?;
    let status = head
        .split("\r\n")
        .next()
        .and_then(|line| {
            let mut parts = line.splitn(3, ' ');
            let version = parts.next()?;
            let code = parts.next()?;
            (version.starts_with("HTTP/1.") && code.len() == 3)
                .then(|| code.parse::<u16>().ok())
                .flatten()
        })
        .ok_or(rc::RESPONSE_INVALID)?;
    let header = |name: &str| {
        head.split("\r\n").skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    };
    let location = header("location");
    if location.is_none() && !matches!(status, 100..=199 | 204 | 304) {
        // The body, bounded, is part of the time; how it ends is not.
        let length = header("content-length").and_then(|v| v.parse::<usize>().ok());
        let want = length.unwrap_or(MAX_BODY).min(MAX_BODY);
        let mut got = buf.len() - head_end;
        let _ = tokio::time::timeout_at(deadline, async {
            let mut chunk = [0u8; 8192];
            while got < want {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got += n,
                }
            }
        })
        .await;
    }
    Ok((status, location))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Writing the request or reading the answer failed: the connection broke
/// (reset, closed without TLS's close_notify, an alert) before a whole
/// response head came.
/// A failed read or write of the exchange. The error itself is logged
/// (never the target): the code alone does not tell a close from a reset
/// or a TLS alert.
fn io_code(error: &io::Error) -> &'static str {
    if error.kind() == io::ErrorKind::TimedOut {
        rc::TIMEOUT
    } else {
        tracing::info!("availability probe: the connection failed: {error}");
        rc::CONNECTION_CLOSED
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use btls::ssl::{
        Ssl, SslAcceptor, SslConnector, SslMethod, SslSessionCacheMode, SslVerifyMode, SslVersion,
    };
    use tokio::net::{TcpListener, TcpStream};

    use super::*;
    use crate::runtime::fake::{Call, FakeRuntime, Op};
    use crate::translate::node_tag;

    /// No step of a test waits longer.
    const BOUND: Duration = Duration::from_secs(10);

    /// A server that answers each connection with the next of `responses`
    /// (none left: holds it open, silent), and keeps the requests.
    async fn serve<R>(responses: Vec<R>) -> (SocketAddr, Arc<Mutex<Vec<String>>>)
    where
        R: AsRef<[u8]> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let requests = seen.clone();
        tokio::spawn(async move {
            let mut responses = responses.into_iter();
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                requests
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                match responses.next() {
                    Some(response) => {
                        let _ = stream.write_all(response.as_ref()).await;
                    }
                    None => {
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            drop(stream);
                        });
                    }
                }
            }
        });
        (addr, seen)
    }

    /// What the TLS server does once the handshake is done and the request
    /// read.
    #[derive(Clone, Copy)]
    enum Then {
        /// Waits, then answers this (its session tickets long sent).
        Answer(Duration, &'static str),
        /// Ends the TLS session (close_notify), answering nothing.
        CloseNotify,
        /// Drops the connection, answering nothing.
        Drop,
    }

    /// A TLS 1.3 server (sail's BoringSSL, as the rule set tests') with
    /// `acceptor`'s certificate. BoringSSL sends its session tickets at the
    /// end of its handshake, before it reads the request.
    async fn serve_tls(acceptor: SslAcceptor, then: Then) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = Arc::new(acceptor);
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(tokio::time::timeout(BOUND, async move {
                    let mut ssl = Ssl::new(acceptor.context()).unwrap();
                    ssl.set_min_proto_version(Some(SslVersion::TLS1_3)).unwrap();
                    let conn = BoringConnection::server(ssl).unwrap();
                    let mut stream = TlsStream::new(conn, tcp, None);
                    if stream.handshake().await.is_err() {
                        return;
                    }
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while find(&head, b"\r\n\r\n").is_none() {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    match then {
                        Then::Answer(delay, response) => {
                            tokio::time::sleep(delay).await;
                            let _ = stream.write_all(response.as_bytes()).await;
                            let _ = stream.shutdown().await;
                        }
                        Then::CloseNotify => {
                            let _ = stream.shutdown().await;
                        }
                        Then::Drop => {}
                    }
                }));
            }
        });
        addr
    }

    async fn runtime(to: Option<SocketAddr>) -> FakeRuntime {
        let runtime = FakeRuntime::default();
        runtime.start("{}").await.unwrap();
        if let Some(to) = to {
            runtime.route_tcp_to(to);
        }
        runtime
    }

    fn tags() -> BTreeMap<String, String> {
        BTreeMap::from([("stable".to_owned(), node_tag("stable"))])
    }

    async fn probe(
        runtime: &FakeRuntime,
        target: &str,
        timeout_ms: u64,
    ) -> Result<(AvailabilityResult, Event), Error> {
        let request = ProbeAvailabilityRequest::new("stable", target, timeout_ms);
        probe_availability(
            runtime,
            &tags(),
            &request,
            DefaultInterface::Present,
            &Cancel::never(),
        )
        .await
    }

    /// The error code of a probe of `target` trusting `trust_pem`.
    async fn code_trusting(runtime: &FakeRuntime, target: &str, trust_pem: &str) -> String {
        let request = ProbeAvailabilityRequest::new("stable", target, 5000);
        let (got, _) = probe_trusting(
            runtime,
            &tags(),
            &request,
            DefaultInterface::Present,
            &Cancel::never(),
            Some(trust_pem),
        )
        .await
        .unwrap();
        got.error_code
    }

    /// The error code of a probe of `target` through `runtime`.
    async fn code(runtime: &FakeRuntime, target: &str) -> String {
        probe(runtime, target, 5000).await.unwrap().0.error_code
    }

    // Go: TestAvailabilityUsesAuthenticatedNodeProxy. Here: through the
    // node's outbound, to the target named in the URL.
    #[tokio::test]
    async fn availability_goes_through_the_node_outbound() {
        let (addr, seen) = serve(vec!["HTTP/1.1 204 No Content\r\n\r\n"]).await;
        let runtime = runtime(Some(addr)).await;
        let (got, event) = probe(&runtime, "http://target.invalid/check?a=1", 1000)
            .await
            .unwrap();
        assert!(got.success, "{got:?}");
        assert_eq!(
            (
                got.node_id.as_str(),
                got.http_status,
                got.error_code.as_str()
            ),
            ("stable", 204, "")
        );
        assert!(runtime.calls().contains(&Call::DialTcp(
            node_tag("stable"),
            Target::Domain("target.invalid".into(), 80)
        )));
        let request = seen.lock().unwrap()[0].clone();
        assert!(
            request.starts_with("GET /check?a=1 HTTP/1.1\r\nHost: target.invalid\r\n"),
            "{request}"
        );
        match event {
            Event::AvailabilityProbed {
                at,
                node_id,
                message,
            } => {
                assert_eq!(at, got.measured_at);
                assert_eq!((node_id.as_str(), message.as_str()), ("stable", "success"));
            }
            other => panic!("{other:?}"),
        }
    }

    // Go: TestAvailabilityCancellation.
    #[tokio::test]
    async fn availability_cancellation() {
        let runtime = runtime(None).await;
        let (canceller, cancel) = Cancel::new();
        canceller.cancel();
        let request = ProbeAvailabilityRequest::new("stable", "https://example.invalid", 1000);
        let (got, event) = probe_availability(
            &runtime,
            &tags(),
            &request,
            DefaultInterface::Present,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(got.error_code, rc::CANCELED);
        assert!(!got.success);
        assert!(
            matches!(event, Event::AvailabilityProbed { message, .. } if message == rc::CANCELED)
        );
        assert!(!runtime
            .calls()
            .iter()
            .any(|c| matches!(c, Call::DialTcp(..))));
    }

    #[tokio::test]
    async fn availability_status_and_redirects() {
        let (addr, seen) = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n",
        ])
        .await;
        let runtime = runtime(Some(addr)).await;
        let (got, _) = probe(&runtime, "http://192.0.2.7:8080/start", 1000)
            .await
            .unwrap();
        assert_eq!((got.success, got.http_status), (true, 200), "{got:?}");
        let requests = seen.lock().unwrap().clone();
        assert!(requests[0].starts_with("GET /start HTTP/1.1\r\nHost: 192.0.2.7:8080\r\n"));
        assert!(
            requests[1].starts_with("GET /next HTTP/1.1\r\n"),
            "{requests:?}"
        );
        assert!(runtime.calls().contains(&Call::DialTcp(
            node_tag("stable"),
            Target::Addr("192.0.2.7:8080".parse().unwrap())
        )));
        let (got, event) = probe(&runtime, "http://target.invalid/", 1000)
            .await
            .unwrap();
        assert_eq!(
            (got.success, got.http_status, got.error_code.as_str()),
            (false, 503, rc::HTTP_STATUS)
        );
        assert!(
            matches!(event, Event::AvailabilityProbed { message, .. } if message == rc::HTTP_STATUS)
        );
    }

    #[tokio::test]
    async fn availability_failures() {
        let runtime = runtime(None).await;
        for target in [
            "",
            "not a url",
            "example.com/path",
            "mailto:someone",
            "ftp://target.invalid/",
        ] {
            let (got, _) = probe(&runtime, target, 1000).await.unwrap();
            assert_eq!(got.error_code, rc::TARGET_INVALID, "{target}");
        }
        assert!(
            !runtime
                .calls()
                .iter()
                .any(|c| matches!(c, Call::DialTcp(..))),
            "dialled an invalid target"
        );

        // The dial, by sail's error kind.
        for (kind, want) in [
            ("failed", rc::CONNECT_FAILED),
            ("io", rc::CONNECT_FAILED),
            ("timeout", rc::TIMEOUT),
            ("not_found", rc::PROXY_REQUEST_FAILED),
            ("not_running", rc::PROXY_REQUEST_FAILED),
        ] {
            runtime.fail_next(Op::Dial, RuntimeError::new(kind, "why"));
            let (got, _) = probe(&runtime, "http://target.invalid/", 1000)
                .await
                .unwrap();
            assert_eq!(got.error_code, want, "{kind}");
        }

        // A server that never answers: the probe's own timeout.
        let (addr, _) = serve(Vec::<&str>::new()).await;
        runtime.route_tcp_to(addr);
        let started = std::time::Instant::now();
        let (got, _) = probe(&runtime, "http://target.invalid/", 100)
            .await
            .unwrap();
        assert_eq!(got.error_code, rc::TIMEOUT);
        assert!(got.total_ms >= 90 && started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn availability_answers_cut_short_or_invalid() {
        let runtime = runtime(None).await;
        let mut long = b"HTTP/1.1 200 OK\r\nX-Long: ".to_vec();
        long.resize(MAX_HEAD + 1024, b'a');
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (b"".to_vec(), rc::CONNECTION_CLOSED),
            (
                b"HTTP/1.1 200 OK\r\nContent-Le".to_vec(),
                rc::CONNECTION_CLOSED,
            ),
            (b"garbage\r\n\r\n".to_vec(), rc::RESPONSE_INVALID),
            (b"HTTP/1.1 2000 OK\r\n\r\n".to_vec(), rc::RESPONSE_INVALID),
            (
                b"HTTP/1.1 200 OK\r\nX: \xff\r\n\r\n".to_vec(),
                rc::RESPONSE_INVALID,
            ),
            (long, rc::RESPONSE_INVALID),
            (
                b"HTTP/1.1 302 Found\r\nLocation: http://[::1\r\n\r\n".to_vec(),
                rc::RESPONSE_INVALID,
            ),
            (
                b"HTTP/1.1 302 Found\r\nLocation: ftp://target.invalid/\r\n\r\n".to_vec(),
                rc::TARGET_INVALID,
            ),
        ];
        for (response, want) in cases {
            let shown = String::from_utf8_lossy(&response[..response.len().min(40)]).into_owned();
            let (addr, _) = serve(vec![response]).await;
            runtime.route_tcp_to(addr);
            assert_eq!(
                code(&runtime, "http://target.invalid/").await,
                want,
                "{shown}"
            );
        }
    }

    #[tokio::test]
    async fn availability_redirect_loop() {
        let redirect = "HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\n\r\n";
        let (addr, seen) = serve(vec![redirect; MAX_REDIRECTS + 1]).await;
        let runtime = runtime(Some(addr)).await;
        assert_eq!(
            code(&runtime, "http://target.invalid/").await,
            rc::REDIRECT_LOOP
        );
        assert_eq!(seen.lock().unwrap().len(), MAX_REDIRECTS + 1);
    }

    #[tokio::test]
    async fn availability_tls_failures() {
        let runtime = runtime(None).await;
        let (pem, acceptor) = crate::tls::testing::authority("probe test ca");
        let (other_pem, _) = crate::tls::testing::authority("another test ca");

        // TLS spoken to a plain HTTP server, or to one that closes at once.
        let (addr, _) = serve(vec!["HTTP/1.1 200 OK\r\n\r\n", ""]).await;
        runtime.route_tcp_to(addr);
        for _ in 0..2 {
            assert_eq!(
                code_trusting(&runtime, "https://127.0.0.1/", &pem).await,
                rc::TLS_HANDSHAKE_FAILED
            );
        }

        // A certificate from an authority not trusted, or for another name.
        let addr = serve_tls(acceptor.clone(), Then::Answer(Duration::ZERO, "")).await;
        runtime.route_tcp_to(addr);
        assert_eq!(
            code_trusting(&runtime, "https://127.0.0.1/", &other_pem).await,
            rc::TLS_CERTIFICATE_INVALID
        );
        assert_eq!(
            code_trusting(&runtime, "https://localhost.invalid/", &pem).await,
            rc::TLS_CERTIFICATE_INVALID
        );

        // The handshake done, the server ends the session, or drops the
        // connection, without an answer.
        for then in [Then::CloseNotify, Then::Drop] {
            let addr = serve_tls(acceptor.clone(), then).await;
            runtime.route_tcp_to(addr);
            assert_eq!(
                code_trusting(&runtime, "https://127.0.0.1/", &pem).await,
                rc::CONNECTION_CLOSED
            );
        }
    }

    // Google's front ends closed, from some exits, connections that came
    // with BoringSSL's own ClientHello (#342): the probe sends Chrome's,
    // which offers X25519MLKEM768 (0x11ec) as Chrome does.
    #[tokio::test]
    async fn the_client_hello_is_chromes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hello = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut hello = vec![0u8; 4096];
            let n = tcp.read(&mut hello).await.unwrap();
            hello.truncate(n);
            hello
        });
        let runtime = runtime(Some(addr)).await;
        let (got, _) = probe(&runtime, "https://127.0.0.1/", 2000).await.unwrap();
        assert!(!got.success, "{got:?}");
        let hello = tokio::time::timeout(BOUND, hello).await.unwrap().unwrap();
        assert_eq!(hello.first(), Some(&0x16), "a TLS handshake record");
        assert!(
            hello.windows(2).any(|w| w == [0x11, 0xec]),
            "no X25519MLKEM768 in the ClientHello"
        );
        assert!(
            hello.windows(8).any(|w| w == b"http/1.1"),
            "the ALPN is http/1.1"
        );
    }

    // #342: Google's front end speaks TLS 1.3 and sends session tickets
    // after its handshake. A read that takes in only a ticket record (no
    // application data) must not read as the end of the stream.
    #[tokio::test]
    async fn tls13_session_tickets_before_the_answer() {
        let (pem, acceptor) = crate::tls::testing::authority("probe test ca");
        let answer = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
        let addr = serve_tls(acceptor, Then::Answer(Duration::from_millis(200), answer)).await;

        // Through the probe, as a host runs it, a few times.
        let runtime = runtime(Some(addr)).await;
        for _ in 0..4 {
            let request = ProbeAvailabilityRequest::new("stable", "https://127.0.0.1/", 5000);
            let (got, _) = probe_trusting(
                &runtime,
                &tags(),
                &request,
                DefaultInterface::Present,
                &Cancel::never(),
                Some(&pem),
            )
            .await
            .unwrap();
            assert!(got.success && got.http_status == 204, "{got:?}");
        }

        // The same exchange with a client that counts the tickets: they did
        // come, over TLS 1.3, before the answer.
        let tickets = Arc::new(AtomicUsize::new(0));
        let mut builder = SslConnector::bare_builder(SslMethod::tls()).unwrap();
        builder.set_verify(SslVerifyMode::NONE);
        builder.set_session_cache_mode(SslSessionCacheMode::CLIENT);
        let counted = tickets.clone();
        builder.set_new_session_callback(move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let ssl = builder
            .build()
            .configure()
            .unwrap()
            .into_ssl("127.0.0.1")
            .unwrap();
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut stream = TlsStream::new(BoringConnection::client(ssl).unwrap(), tcp, None);
        tokio::time::timeout(BOUND, stream.handshake())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stream.conn().ssl().version_str(), "TLSv1.3");
        let url = Url::parse("https://127.0.0.1/").unwrap();
        let got = exchange(&mut stream, &url, Instant::now() + BOUND).await;
        assert_eq!(got, Ok((204, None)));
        assert!(tickets.load(Ordering::SeqCst) > 0, "no session ticket came");
    }

    #[allow(dead_code)]
    fn _assert_send(runtime: Arc<dyn Runtime>, request: ProbeAvailabilityRequest, c: Cancel) {
        fn send<T: Send>(_: T) {}
        send(async move {
            let tags = tags();
            probe_availability(&*runtime, &tags, &request, DefaultInterface::Present, &c).await
        });
    }

    #[tokio::test]
    async fn availability_unknown_node() {
        let runtime = runtime(None).await;
        let request = ProbeAvailabilityRequest::new("missing", "http://target.invalid/", 1000);
        let error = probe_availability(
            &runtime,
            &tags(),
            &request,
            DefaultInterface::Present,
            &Cancel::never(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (error.code, error.field.as_deref()),
            (codes::NODE_NOT_FOUND, Some("node_id"))
        );
    }

    // Go: internal/runtime TestProbesFailFastWithoutDefaultInterface (the
    // availability half).
    #[tokio::test]
    async fn availability_offline_fails_fast_and_dials_nothing() {
        let (addr, _) = serve(vec!["HTTP/1.1 204 No Content\r\n\r\n"]).await;
        let runtime = runtime(Some(addr)).await;
        let request = ProbeAvailabilityRequest::new("stable", "http://example.com/", 5000);
        let started = std::time::Instant::now();
        let error = probe_availability(
            &runtime,
            &tags(),
            &request,
            DefaultInterface::Absent,
            &Cancel::never(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (error.code, error.retryable),
            (codes::NO_DEFAULT_INTERFACE, true)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            !runtime
                .calls()
                .iter()
                .any(|c| matches!(c, Call::DialTcp(..))),
            "dialled while offline"
        );
        let (got, _) = probe_availability(
            &runtime,
            &tags(),
            &request,
            DefaultInterface::Unknown,
            &Cancel::never(),
        )
        .await
        .unwrap();
        assert!(got.success, "{got:?}");
    }
}
