//! Tunnel mode's transport (#448): Core API v1 calls to the tunnel host in
//! the phone's VPN process (Android's `VpnService`, iOS's packet tunnel
//! extension), over its Unix socket.
//!
//! Wire format (#448, the tunnel socket): frames of `[u32 BE length][UTF-8
//! JSON]`, at most [`MAX_FRAME`]. One call per connection: the request
//! `{"path", "body"}`, then the answer `{"ok": true, "data"}` or
//! `{"ok": false, "error": {"code", "message", "retryable"}}`. `/v1/watch`
//! keeps its connection open for the host's events, in order; the host
//! closes it when it stops.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::core_ipc::{parse_status, BoxFuture, CoreCallError, CoreStatus, CoreTransport};

/// The phones' socket. Desktop builds have the type too, so the client
/// compiles everywhere; there a call fails (tunnel mode is only used on
/// Android and iOS).
#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(not(unix))]
type Stream = tokio::io::DuplexStream;

/// The socket's file name in the app's directory: Android's `filesDir`, the
/// App Group container on iOS.
pub(crate) const SOCKET_FILE: &str = "tunnel.sock";

/// The largest frame either side sends.
const MAX_FRAME: usize = 16 << 20;

/// Calls the tunnel host at `socket`.
pub(crate) struct TunnelTransport {
    socket: PathBuf,
}

impl TunnelTransport {
    /// The host's socket in the app's directory `dir`.
    pub(crate) fn in_dir(dir: &Path) -> Self {
        Self {
            socket: dir.join(SOCKET_FILE),
        }
    }

    #[cfg(unix)]
    async fn connect(&self) -> Result<Stream, CoreCallError> {
        Stream::connect(&self.socket).await.map_err(|error| {
            CoreCallError::Transport(format!("connect {}: {error}", self.socket.display()))
        })
    }

    #[cfg(not(unix))]
    async fn connect(&self) -> Result<Stream, CoreCallError> {
        Err(CoreCallError::Transport(format!(
            "connect {}: no Unix sockets on this platform",
            self.socket.display()
        )))
    }

    /// The host's events until it stops: the first is the current status.
    pub(crate) async fn watch(&self) -> Result<Watch, CoreCallError> {
        let mut stream = self.connect().await?;
        write_frame(&mut stream, &request("/v1/watch", json!({}))).await?;
        Ok(Watch { stream })
    }
}

impl CoreTransport for TunnelTransport {
    fn call<'a>(
        &'a self,
        path: &'static str,
        body: Value,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Value, CoreCallError>> {
        Box::pin(async move {
            tokio::time::timeout(timeout, async {
                let mut stream = self.connect().await?;
                write_frame(&mut stream, &request(path, body)).await?;
                match read_frame(&mut stream).await? {
                    Some(frame) => answer(&frame),
                    None => Err(CoreCallError::Transport(
                        "the tunnel host closed the connection".into(),
                    )),
                }
            })
            .await
            .map_err(|_| CoreCallError::Transport(format!("{path}: timed out")))?
        })
    }
}

/// An open `/v1/watch`.
pub(crate) struct Watch {
    stream: Stream,
}

impl Watch {
    /// The next status, or `None` once the host closed the connection (it
    /// stopped). Kinds this client does not know are skipped.
    pub(crate) async fn next(&mut self) -> Result<Option<CoreStatus>, CoreCallError> {
        loop {
            let Some(frame) = read_frame(&mut self.stream).await? else {
                return Ok(None);
            };
            let data = answer(&frame)?;
            if data.get("kind").and_then(Value::as_str) == Some("status") {
                return Ok(Some(parse_status(
                    data.get("status").unwrap_or(&Value::Null),
                )));
            }
        }
    }
}

fn request(path: &str, body: Value) -> Vec<u8> {
    json!({ "path": path, "body": body })
        .to_string()
        .into_bytes()
}

/// The answer's data, or its error as the core's.
fn answer(frame: &[u8]) -> Result<Value, CoreCallError> {
    let mut answer: Value = serde_json::from_slice(frame)
        .map_err(|error| CoreCallError::Transport(format!("not JSON: {error}")))?;
    if answer.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(answer
            .get_mut("data")
            .map(Value::take)
            .unwrap_or(Value::Null));
    }
    let error = answer
        .get("error")
        .ok_or_else(|| CoreCallError::Transport("an answer with neither ok nor error".into()))?;
    let text = |key: &str| {
        error
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Err(CoreCallError::Api {
        code: text("code"),
        message: text("message"),
        retryable: error
            .get("retryable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

pub(crate) async fn write_frame<W: AsyncWrite + Unpin>(
    stream: &mut W,
    frame: &[u8],
) -> Result<(), CoreCallError> {
    let lost = |error: std::io::Error| CoreCallError::Transport(format!("write: {error}"));
    let length = u32::try_from(frame.len())
        .ok()
        .filter(|&length| length as usize <= MAX_FRAME)
        .ok_or_else(|| CoreCallError::Transport(format!("a request of {} bytes", frame.len())))?;
    stream
        .write_all(&length.to_be_bytes())
        .await
        .map_err(lost)?;
    stream.write_all(frame).await.map_err(lost)?;
    stream.flush().await.map_err(lost)
}

/// The next frame, or `None` when the stream ended before one began.
pub(crate) async fn read_frame<R: AsyncRead + Unpin>(
    stream: &mut R,
) -> Result<Option<Vec<u8>>, CoreCallError> {
    let lost = |error: std::io::Error| CoreCallError::Transport(format!("read: {error}"));
    let mut length = [0u8; 4];
    let mut read = 0;
    while read < length.len() {
        match stream.read(&mut length[read..]).await.map_err(lost)? {
            0 if read == 0 => return Ok(None),
            0 => {
                return Err(CoreCallError::Transport(
                    "the frame's length was cut off".into(),
                ))
            }
            n => read += n,
        }
    }
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err(CoreCallError::Transport(format!(
            "a frame of {length} bytes"
        )));
    }
    let mut frame = vec![0u8; length];
    stream.read_exact(&mut frame).await.map_err(lost)?;
    Ok(Some(frame))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    /// A tunnel host that answers each connection's request with `reply`
    /// (the frames to write back), recording the requests.
    fn host(
        dir: &Path,
        reply: impl Fn(Value) -> Vec<Value> + Send + 'static,
    ) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
        let listener = UnixListener::bind(dir.join(SOCKET_FILE)).unwrap();
        let (seen, requests) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let frame = read_frame(&mut stream).await.unwrap().unwrap();
                let request: Value = serde_json::from_slice(&frame).unwrap();
                seen.send(request.clone()).unwrap();
                for answer in reply(request) {
                    write_frame(&mut stream, answer.to_string().as_bytes())
                        .await
                        .unwrap();
                }
            }
        });
        requests
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ppvpn-tunnel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_call_sends_path_and_body_and_returns_the_data() {
        let dir = temp_dir();
        let mut requests = host(&dir, |_| {
            vec![json!({ "ok": true, "data": { "node_count": 3 } })]
        });
        let transport = TunnelTransport::in_dir(&dir);
        let data = transport
            .call(
                "/v1/select-node",
                json!({ "node_id": "hk" }),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(data, json!({ "node_count": 3 }));
        assert_eq!(
            requests.recv().await.unwrap(),
            json!({ "path": "/v1/select-node", "body": { "node_id": "hk" } })
        );
    }

    #[tokio::test]
    async fn an_error_answer_is_the_cores_error() {
        let dir = temp_dir();
        let _requests = host(&dir, |_| {
            vec![json!({ "ok": false, "error": {
                "code": "TUNNEL_NOT_READY", "message": "starting", "retryable": true } })]
        });
        let error = TunnelTransport::in_dir(&dir)
            .call("/v1/get-status", json!({}), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            CoreCallError::Api {
                code: "TUNNEL_NOT_READY".into(),
                message: "starting".into(),
                retryable: true,
            }
        );
    }

    #[tokio::test]
    async fn no_host_is_a_transport_error() {
        let error = TunnelTransport::in_dir(&temp_dir())
            .call("/v1/get-status", json!({}), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(matches!(error, CoreCallError::Transport(_)), "{error:?}");
    }

    #[tokio::test]
    async fn a_watch_yields_statuses_skips_other_kinds_and_ends_when_the_host_closes() {
        let dir = temp_dir();
        let mut requests = host(&dir, |_| {
            vec![
                json!({ "ok": true, "data": { "kind": "status", "status": { "state": "starting" } } }),
                json!({ "ok": true, "data": { "kind": "health", "round": 1 } }),
                json!({ "ok": true, "data": { "kind": "status", "status": {
                    "state": "running", "selected_node_id": "hk" } } }),
            ]
        });
        let mut watch = TunnelTransport::in_dir(&dir).watch().await.unwrap();
        assert_eq!(watch.next().await.unwrap().unwrap().state, "starting");
        let running = watch.next().await.unwrap().unwrap();
        assert_eq!(running.state, "running");
        assert_eq!(running.selected_node_id.as_deref(), Some("hk"));
        assert!(watch.next().await.unwrap().is_none());
        assert_eq!(
            requests.recv().await.unwrap(),
            json!({ "path": "/v1/watch", "body": {} })
        );
    }

    #[tokio::test]
    async fn an_oversized_frame_is_refused() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server
            .write_all(&((MAX_FRAME as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(read_frame(&mut client).await.is_err());
    }
}
