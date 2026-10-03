//! Availability probes: a whole GET of the target through the node, timed as
//! `total_ms` (the node's connection, the target's answer, its body up to
//! 64 KiB). Go fetched through the node's local proxy user; here the request
//! goes through the node's outbound itself ([`Runtime::dial_tcp`]), which is
//! where that user leads. Redirects are followed, as Go's client does; TLS
//! is sail's (BoringSSL), checked against the system's roots.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;
use url::{Host, Url};

use super::result_codes as rc;
use super::{require_online, timeout_of, Cancel, DefaultInterface};
use crate::error::{codes, Error};
use crate::event::Event;
use crate::runtime::{Runtime, Target};
use crate::types::{AvailabilityResult, ProbeAvailabilityRequest};

/// A request's timeout when it gives none.
pub(crate) const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(10);
/// Go's client gives up after 10 redirects.
const MAX_REDIRECTS: usize = 10;
/// The most of a response's head read.
const MAX_HEAD: usize = 64 << 10;
/// The most of the final body read (and timed).
const MAX_BODY: usize = 64 << 10;

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
    require_online(network)?;
    let outbound = node_tags
        .get(&request.node_id)
        .ok_or_else(|| Error::invalid(codes::NODE_NOT_FOUND, "node_id", "node not found"))?;
    let timeout = timeout_of(request.timeout_ms, AVAILABILITY_TIMEOUT);
    let started = Instant::now();
    let deadline = started + timeout;
    let outcome = if cancel.is_cancelled() {
        Err(rc::CANCELED)
    } else {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(rc::CANCELED),
            outcome = tokio::time::timeout_at(deadline, fetch(runtime, outbound, &request.target, deadline)) => {
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
) -> Result<u16, &'static str> {
    let mut url = match Url::parse(target) {
        Ok(url) if url.has_host() && !url.scheme().is_empty() => url,
        _ => return Err(rc::TARGET_INVALID),
    };
    for _ in 0..=MAX_REDIRECTS {
        let (status, location) = get(runtime, outbound, &url, deadline).await?;
        match location {
            Some(location) if matches!(status, 301 | 302 | 303 | 307 | 308) => {
                url = url.join(&location).map_err(|_| rc::PROXY_REQUEST_FAILED)?;
            }
            _ => return Ok(status),
        }
    }
    Err(rc::PROXY_REQUEST_FAILED)
}

/// One GET: its status and, for a redirect, where to.
async fn get(
    runtime: &dyn Runtime,
    outbound: &str,
    url: &Url,
    deadline: Instant,
) -> Result<(u16, Option<String>), &'static str> {
    let tls = match url.scheme() {
        "http" => false,
        "https" => true,
        _ => return Err(rc::PROXY_REQUEST_FAILED),
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
        .map_err(|e| {
            if e.code == "timeout" {
                rc::TIMEOUT
            } else {
                rc::PROXY_REQUEST_FAILED
            }
        })?;
    if !tls {
        return exchange(stream, url, deadline).await;
    }
    use sail::config::model::CertificateStore;
    use sail::transport::tls::{roots::Roots, TlsClient};
    let roots = Roots::of(CertificateStore::System).map_err(|_| rc::PROXY_REQUEST_FAILED)?;
    let client = TlsClient::new(&["http/1.1".to_owned()], None, false, None, &roots)
        .map_err(|_| rc::PROXY_REQUEST_FAILED)?;
    let stream = client
        .connect(&server_name, stream, None, None)
        .await
        .map_err(|e| io_code(&e))?;
    exchange(stream, url, deadline).await
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
            return Err(rc::PROXY_REQUEST_FAILED);
        }
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.map_err(|e| io_code(&e))?;
        if n == 0 {
            return Err(rc::PROXY_REQUEST_FAILED);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| rc::PROXY_REQUEST_FAILED)?;
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
        .ok_or(rc::PROXY_REQUEST_FAILED)?;
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

fn io_code(error: &std::io::Error) -> &'static str {
    if error.kind() == std::io::ErrorKind::TimedOut {
        rc::TIMEOUT
    } else {
        rc::PROXY_REQUEST_FAILED
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::net::TcpListener;

    use super::*;
    use crate::runtime::fake::{Call, FakeRuntime, Op};
    use crate::runtime::RuntimeError;
    use crate::translate::node_tag;

    /// A server that answers each connection with the next of `responses`
    /// (none left: holds it open, silent), and keeps the requests.
    async fn serve(responses: Vec<&'static str>) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
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
                        let _ = stream.write_all(response.as_bytes()).await;
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
        for target in ["", "not a url", "example.com/path", "mailto:someone"] {
            let (got, _) = probe(&runtime, target, 1000).await.unwrap();
            assert_eq!(got.error_code, rc::TARGET_INVALID, "{target}");
        }
        let (got, _) = probe(&runtime, "ftp://target.invalid/", 1000)
            .await
            .unwrap();
        assert_eq!(got.error_code, rc::PROXY_REQUEST_FAILED);

        runtime.fail_next(Op::Dial, RuntimeError::new("failed", "refused"));
        let (got, _) = probe(&runtime, "http://target.invalid/", 1000)
            .await
            .unwrap();
        assert_eq!(got.error_code, rc::PROXY_REQUEST_FAILED);
        runtime.fail_next(Op::Dial, RuntimeError::new("timeout", "slow"));
        let (got, _) = probe(&runtime, "http://target.invalid/", 1000)
            .await
            .unwrap();
        assert_eq!(got.error_code, rc::TIMEOUT);

        // A server that never answers: the probe's own timeout.
        let (addr, _) = serve(vec![]).await;
        runtime.route_tcp_to(addr);
        let started = std::time::Instant::now();
        let (got, _) = probe(&runtime, "http://target.invalid/", 100)
            .await
            .unwrap();
        assert_eq!(got.error_code, rc::TIMEOUT);
        assert!(got.total_ms >= 90 && started.elapsed() < Duration::from_secs(5));

        // TLS spoken to a plain HTTP server fails as the request.
        let (addr, _) = serve(vec!["HTTP/1.1 200 OK\r\n\r\n"]).await;
        runtime.route_tcp_to(addr);
        let (got, _) = probe(&runtime, "https://target.invalid/", 2000)
            .await
            .unwrap();
        assert!(
            got.error_code == rc::PROXY_REQUEST_FAILED || got.error_code == rc::TIMEOUT,
            "{got:?}"
        );
        assert!(!got.success);
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
