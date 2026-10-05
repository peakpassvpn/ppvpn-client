//! One rule set download: a GET over HTTP/1.1 and sail's TLS (BoringSSL,
//! the system's roots) on a connection the engine opens. Never through an
//! environment proxy, and never following a redirect: it would leave the
//! pinned host.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::header::{ACCEPT, CONNECTION, HOST, IF_NONE_MATCH, USER_AGENT};
use hyper::{Request, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use sail::transport::tls::TlsClient;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

use super::{DOWNLOAD_FAILED, HTTP_STATUS, MAX_SIZE, TOO_LARGE};

/// How long [`DirectDial`] waits for a connection.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// A connection a download runs on.
pub(crate) trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Opens the connections of rule set downloads, which must bypass the
/// tunnel. The engine's goes through the running TUN instance's direct
/// outbound (bound to the physical interface), as Go's `dialRuleSet`;
/// without a TUN instance a plain socket is direct ([`DirectDial`]).
#[async_trait]
pub(crate) trait Dial: Send + Sync {
    /// A TCP connection to `host` (a name or an IP literal) and `port`.
    async fn dial(&self, host: &str, port: u16) -> io::Result<Box<dyn Stream>>;
}

/// A plain socket, its name resolved by the system.
pub(crate) struct DirectDial;

#[async_trait]
impl Dial for DirectDial {
    async fn dial(&self, host: &str, port: u16) -> io::Result<Box<dyn Stream>> {
        let stream = tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        Ok(Box::new(stream))
    }
}

pub(super) enum Fetched {
    Body(Vec<u8>),
    /// 304 to a request naming the cached copy.
    NotModified,
}

pub(super) struct Client {
    dial: Arc<dyn Dial>,
    /// None when no TLS client could be made (no roots): every download
    /// fails.
    tls: Option<TlsClient>,
}

impl Client {
    /// `trust_pem` replaces the system's roots (tests).
    pub(super) fn new(dial: Arc<dyn Dial>, trust_pem: Option<&str>) -> Self {
        let tls = crate::tls::client(&[], trust_pem)
            .inspect_err(|e| tracing::warn!("rule set downloads have no TLS client: {e}"))
            .ok();
        Client { dial, tls }
    }

    /// GETs `url`, conditionally on the cached copy's digest (the server's
    /// ETag) when there is one. The error is the status's code.
    pub(super) async fn get(
        &self,
        url: &str,
        cached_sha256: Option<&str>,
    ) -> Result<Fetched, &'static str> {
        let uri: Uri = url.parse().map_err(|_| DOWNLOAD_FAILED)?;
        let authority = uri.authority().ok_or(DOWNLOAD_FAILED)?;
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']');
        let port = authority.port_u16().unwrap_or(443);
        let path = uri.path_and_query().map_or("/", |p| p.as_str());
        let tls = self.tls.as_ref().ok_or(DOWNLOAD_FAILED)?;

        let stream = self
            .dial
            .dial(host, port)
            .await
            .map_err(|_| DOWNLOAD_FAILED)?;
        let stream = tls
            .connect(host, stream, None, None)
            .await
            .map_err(|_| DOWNLOAD_FAILED)?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|_| DOWNLOAD_FAILED)?;
        let _connection = AbortOnDrop(tokio::spawn(connection));

        let mut request = Request::get(path)
            .header(HOST, authority.as_str())
            .header(ACCEPT, "application/octet-stream")
            .header(
                USER_AGENT,
                concat!("ppvpn-core/", env!("CARGO_PKG_VERSION")),
            )
            .header(CONNECTION, "close");
        if let Some(sha256) = cached_sha256 {
            request = request.header(IF_NONE_MATCH, format!("\"{sha256}\""));
        }
        let request = request
            .body(Empty::<Bytes>::new())
            .map_err(|_| DOWNLOAD_FAILED)?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| DOWNLOAD_FAILED)?;
        match response.status() {
            StatusCode::NOT_MODIFIED if cached_sha256.is_some() => return Ok(Fetched::NotModified),
            StatusCode::OK => {}
            _ => return Err(HTTP_STATUS),
        }
        let mut body = response.into_body();
        let mut data = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| DOWNLOAD_FAILED)?;
            if let Ok(chunk) = frame.into_data() {
                if data.len() + chunk.len() > MAX_SIZE {
                    return Err(TOO_LARGE);
                }
                data.extend_from_slice(&chunk);
            }
        }
        Ok(Fetched::Body(data))
    }
}

/// The task driving a connection, stopped with the download.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
