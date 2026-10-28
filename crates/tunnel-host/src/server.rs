//! Core API v1 on the Unix socket, for the UI process (#448 comment
//! 6393069163):
//! - a frame is `[u32 big-endian length][UTF-8 JSON]`, at most 16 MiB;
//! - one call per connection: `{"path", "body"}`, answered by
//!   `engine-host::dispatch` as `{"ok": true, "data"}` or
//!   `{"ok": false, "error": {"code", "message", "retryable"}}`;
//! - `/v1/watch` keeps its connection: a `status` frame at once and at each
//!   state change, and the health loop's `health` frames, until the host
//!   stops.
//!
//! No authentication: the socket lives where only the app (Android's
//! filesDir) or its team (iOS's App Group) can reach.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use ppvpn_core::{EventItem, EventKind};
use ppvpn_engine_host::ApiError;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;

use crate::host::{Shared, Signal};
use crate::{api_error, request_invalid};

pub(crate) const MAX_FRAME: usize = 16 << 20;
/// The longest socket path `bind` takes (`sun_path`, its NUL included).
const MAX_SOCKET_PATH: usize = 104;
/// How long a connection may take to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Binds `path`, replacing a stale socket file. A path too long for
/// `sun_path` is bound relative to its directory, the process's working
/// directory from then on (the UI process connects the same way).
pub(crate) fn bind(path: &Path) -> Result<UnixListener, ApiError> {
    let failed = |error: std::io::Error| {
        api_error(
            "TUNNEL_SOCKET_FAILED",
            format!("{}: {error}", path.display()),
            false,
        )
    };
    let _ = std::fs::remove_file(path);
    let listener = if path.as_os_str().as_bytes().len() < MAX_SOCKET_PATH {
        std::os::unix::net::UnixListener::bind(path)
    } else {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(request_invalid(format!("socket_path {}", path.display())));
        };
        std::env::set_current_dir(dir).and_then(|()| std::os::unix::net::UnixListener::bind(name))
    }
    .map_err(failed)?;
    listener.set_nonblocking(true).map_err(failed)?;
    UnixListener::from_std(listener).map_err(failed)
}

pub(crate) async fn serve(listener: UnixListener, shared: Shared) {
    let mut shutdown = shared.shutdown.clone();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(connection(stream, shared.clone()));
                }
                Err(error) => {
                    shared.log.line(&format!("accept: {error}"));
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            _ = shutdown.changed() => return,
        }
    }
}

async fn connection(mut stream: UnixStream, shared: Shared) {
    let request = match tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut stream)).await {
        Ok(Ok(Some(request))) => request,
        // Closed, too large, or too slow: nothing to answer.
        _ => return,
    };
    let (path, body) = match parse(&request) {
        Ok(call) => call,
        Err(error) => {
            let _ = write_frame(&mut stream, &failure(&error)).await;
            return;
        }
    };
    if path == "/v1/watch" {
        watch(stream, shared).await;
        return;
    }
    let answer = match ppvpn_engine_host::dispatch(&shared.engine, &path, body).await {
        Ok(data) => {
            // A connect from the UI: a new recovery episode.
            if path == "/v1/start" {
                let _ = shared.signals.send(Signal::UserStart);
            }
            json!({ "ok": true, "data": data })
        }
        Err(error) => failure(&error),
    };
    let _ = write_frame(&mut stream, &answer).await;
}

fn parse(request: &[u8]) -> Result<(String, Value), ApiError> {
    let request: Value = serde_json::from_slice(request)
        .map_err(|error| request_invalid(format!("request: {error}")))?;
    let path = request
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| request_invalid("path is required"))?
        .to_string();
    let body = match request.get("body") {
        None | Some(Value::Null) => json!({}),
        Some(body) => body.clone(),
    };
    Ok((path, body))
}

fn failure(error: &ApiError) -> Value {
    json!({ "ok": false, "error": error })
}

/// `/v1/watch`: status frames, then health frames, until the host stops or
/// the client goes away.
async fn watch(stream: UnixStream, shared: Shared) {
    let (mut reader, mut writer) = stream.into_split();
    let mut states = shared.engine.subscribe(&[EventKind::StateChanged]);
    let mut health = shared.health.subscribe();
    let mut shutdown = shared.shutdown.clone();
    if write_frame(&mut writer, &status_frame(&shared).await)
        .await
        .is_err()
    {
        return;
    }
    let mut ignored = [0u8; 64];
    loop {
        let frame = tokio::select! {
            item = states.recv() => match item {
                Some(EventItem::Event { .. } | EventItem::Lagged { .. }) => status_frame(&shared).await,
                Some(_) => continue,
                // The engine shut down.
                None => return,
            },
            item = health.recv() => match item {
                Ok(frame) => json!({ "ok": true, "data": frame }),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            },
            // The client closed its end (or wrote, which it should not).
            read = reader.read(&mut ignored) => match read {
                Ok(0) | Err(_) => return,
                Ok(_) => continue,
            },
            _ = shutdown.changed() => return,
        };
        if write_frame(&mut writer, &frame).await.is_err() {
            return;
        }
    }
}

async fn status_frame(shared: &Shared) -> Value {
    match ppvpn_engine_host::dispatch(&shared.engine, "/v1/get-status", json!({})).await {
        Ok(status) => json!({ "ok": true, "data": { "kind": "status", "status": status } }),
        Err(error) => failure(&error),
    }
}

/// One frame; `None` when the peer closed before its first byte.
pub(crate) async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame of {length} bytes"),
        ));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

pub(crate) async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    writer.write_all(&(body.len() as u32).to_be_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse() {
        let (path, body) = parse(br#"{"path": "/v1/get-status"}"#).unwrap();
        assert_eq!((path.as_str(), body), ("/v1/get-status", json!({})));
        let (_, body) = parse(br#"{"path": "/v1/select-node", "body": {"node_id": "n"}}"#).unwrap();
        assert_eq!(body, json!({"node_id": "n"}));
        assert_eq!(parse(b"{").unwrap_err().code, "REQUEST_INVALID");
        assert_eq!(
            parse(br#"{"body": {}}"#).unwrap_err().code,
            "REQUEST_INVALID"
        );
    }

    #[tokio::test]
    async fn frames_round_trip_and_oversize_is_refused() {
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        write_frame(&mut a, &json!({"x": 1})).await.unwrap();
        let frame = read_frame(&mut b).await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&frame).unwrap(),
            json!({"x": 1})
        );

        a.write_all(&((MAX_FRAME as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        assert!(read_frame(&mut b).await.is_err());

        drop(a);
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }
}
