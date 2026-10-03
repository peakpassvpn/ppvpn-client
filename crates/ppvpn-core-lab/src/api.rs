//! The part of Core API v1 the lab scripts use (Go: api/server.go), over the
//! engine: the same envelope, authentication, error codes and log lines, so
//! test/lab and the netns CI run unchanged against this binary.
//!
//! Served: get-version, apply-profile, start, stop, get-status and
//! watch-events. Every other method answers API_NOT_FOUND until the engine
//! has it.

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use ppvpn_core::{codes, ApplyRequest, Engine, EventItem, RoutingMode};

use crate::corelog::Logger;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// As the Go core: 4 MiB.
pub const MAX_REQUEST_BYTES: usize = 4 << 20;

/// The paths logged on success too, so the log shows what led to a failure.
const LIFECYCLE_PATHS: &[&str] = &["/v1/apply-profile", "/v1/start", "/v1/stop", "/v1/reload", "/v1/set-system-proxy", "/v1/pin-ingress"];

pub type Body = BoxBody<Bytes, Infallible>;

pub struct Api {
    pub engine: Engine,
    pub secret: String,
    pub log: Logger,
}

#[derive(Serialize)]
struct Envelope<'a> {
    request_id: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ApiError>,
}

#[derive(Serialize)]
struct ApiError {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    field: String,
    retryable: bool,
}

/// Core API v1's request body (the fields of the methods served).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    #[serde(default)]
    profile: Option<Box<serde_json::value::RawValue>>,
    #[serde(default)]
    routing_mode: String,
    #[serde(default)]
    allowed_rule_set_hosts: Vec<String>,
}

fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into()).boxed()
}

fn envelope(status: StatusCode, id: &str, data: Option<Value>, error: Option<ApiError>) -> Response<Body> {
    let mut body = serde_json::to_vec(&Envelope { request_id: id, ok: error.is_none(), data, error }).expect("envelope");
    body.push(b'\n');
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(full(body))
        .expect("response")
}

fn api_error(code: &str, message: &str) -> ApiError {
    ApiError { code: code.into(), message: message.into(), field: String::new(), retryable: false }
}

fn request_id<B>(req: &Request<B>) -> String {
    if let Some(id) = req.headers().get("X-Request-ID").and_then(|v| v.to_str().ok()) {
        if !id.is_empty() && id.len() <= 128 {
            return id.to_string();
        }
    }
    let mut bytes = [0u8; 12];
    if getrandom::getrandom(&mut bytes).is_err() {
        return "unknown".into();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant time over the secret's length.
fn secret_matches(provided: &str, secret: &str) -> bool {
    provided.len() == secret.len() && provided.bytes().zip(secret.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

impl Api {
    pub async fn handle(self: Arc<Self>, req: Request<Incoming>) -> Result<Response<Body>, Infallible> {
        let id = request_id(&req);
        let auth = req.headers().get("Authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
        let provided = auth.strip_prefix("Bearer ").unwrap_or(auth);
        if !secret_matches(provided, &self.secret) {
            return Ok(envelope(
                StatusCode::UNAUTHORIZED,
                &id,
                None,
                Some(api_error("UNAUTHENTICATED", "valid session authentication is required")),
            ));
        }
        if let Some(v) = req.headers().get("X-Core-API-Version") {
            if v.as_bytes() != b"1" && !v.is_empty() {
                return Ok(envelope(
                    StatusCode::BAD_REQUEST,
                    &id,
                    None,
                    Some(api_error("CORE_API_UNSUPPORTED", "requested Core API version is unsupported")),
                ));
            }
        }
        let path = req.uri().path().to_string();
        let method = req.method().clone();
        if method == Method::GET && path == "/v1/watch-events" {
            return Ok(self.watch_events(&id));
        }
        if method != Method::POST {
            return Ok(not_found(&id));
        }
        let result = match path.as_str() {
            "/v1/get-version" => Ok(version()),
            "/v1/apply-profile" => match decode(req).await {
                Ok(request) => self.apply(request).await,
                Err(response) => return Ok(response(&id)),
            },
            "/v1/start" => self.engine.start().await.map(|()| json!({})),
            "/v1/stop" => self.engine.stop().await.map(|()| json!({})),
            "/v1/get-status" => Ok(serde_json::to_value(self.engine.status()).expect("status")),
            _ => return Ok(not_found(&id)),
        };
        Ok(self.respond(&path, &id, result))
    }

    async fn apply(&self, request: RawRequest) -> Result<Value, ppvpn_core::Error> {
        let mode = RoutingMode::parse(&request.routing_mode)?;
        let profile = request.profile.map(|p| p.get().as_bytes().to_vec()).unwrap_or_default();
        let apply = ApplyRequest::new(profile)
            .with_routing_mode(mode)
            .with_allowed_rule_set_hosts(request.allowed_rule_set_hosts);
        let result = self.engine.apply(apply).await?;
        Ok(json!({ "applied": result.applied }))
    }

    fn respond(&self, path: &str, id: &str, result: Result<Value, ppvpn_core::Error>) -> Response<Body> {
        match result {
            Ok(data) => {
                if LIFECYCLE_PATHS.contains(&path) {
                    self.log.info("request ok", &[("path", &path), ("request_id", &id)]);
                }
                envelope(StatusCode::OK, id, Some(data), None)
            }
            Err(err) if err.code == codes::CORE_OPERATION_FAILED => {
                // The response stays folded; the cause goes to the log only.
                self.log.error("CORE_OPERATION_FAILED", &[("path", &path), ("request_id", &id), ("error", &err.message)]);
                envelope(StatusCode::BAD_REQUEST, id, None, Some(api_error("CORE_OPERATION_FAILED", "core operation failed")))
            }
            Err(err) => {
                let field = err.field.clone().unwrap_or_default();
                self.log.info("request rejected", &[("path", &path), ("request_id", &id), ("code", &err.code), ("field", &field)]);
                let error = ApiError { code: err.code.into(), message: err.message, field, retryable: err.retryable };
                envelope(StatusCode::BAD_REQUEST, id, None, Some(error))
            }
        }
    }

    /// NDJSON: one envelope per event, as the Go core streams them.
    fn watch_events(&self, id: &str) -> Response<Body> {
        let mut events = self.engine.subscribe(&[]);
        let id = id.to_string();
        let stream = async_stream(move |tx| async move {
            while let Some(item) = events.recv().await {
                let EventItem::Event { event } = item else { continue };
                let data = serde_json::to_value(&event).expect("event");
                let mut line = serde_json::to_vec(&Envelope { request_id: &id, ok: true, data: Some(data), error: None }).expect("envelope");
                line.push(b'\n');
                if tx.send(Ok(Frame::data(Bytes::from(line)))).await.is_err() {
                    return;
                }
            }
        });
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/x-ndjson")
            .body(StreamBody::new(stream).boxed())
            .expect("response")
    }
}

/// A body stream fed by a task.
fn async_stream<F, Fut>(f: F) -> tokio_stream_shim::Receiver
where
    F: FnOnce(tokio::sync::mpsc::Sender<Result<Frame<Bytes>, Infallible>>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(f(tx));
    tokio_stream_shim::Receiver(rx)
}

mod tokio_stream_shim {
    use std::convert::Infallible;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use hyper::body::Frame;

    pub struct Receiver(pub tokio::sync::mpsc::Receiver<Result<Frame<Bytes>, Infallible>>);

    impl futures_core::Stream for Receiver {
        type Item = Result<Frame<Bytes>, Infallible>;
        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.0.poll_recv(cx)
        }
    }
}

fn not_found(id: &str) -> Response<Body> {
    envelope(StatusCode::NOT_FOUND, id, None, Some(api_error("API_NOT_FOUND", "Core API method was not found")))
}

type Rejection = Box<dyn FnOnce(&str) -> Response<Body> + Send>;

/// One JSON value, at most MAX_REQUEST_BYTES, no unknown fields.
async fn decode(req: Request<Incoming>) -> Result<RawRequest, Rejection> {
    let invalid = |message: &'static str| -> Rejection {
        Box::new(move |id: &str| envelope(StatusCode::BAD_REQUEST, id, None, Some(api_error("REQUEST_INVALID", message))))
    };
    let body = match Limited::new(req.into_body(), MAX_REQUEST_BYTES).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return Err(invalid("request body is invalid")),
    };
    let mut values = serde_json::Deserializer::from_slice(&body).into_iter::<Box<serde_json::value::RawValue>>();
    let Some(Ok(first)) = values.next() else { return Err(invalid("request body is invalid")) };
    if values.next().is_some() {
        return Err(invalid("request body must contain exactly one JSON value"));
    }
    serde_json::from_str(first.get()).map_err(|_| invalid("request body is invalid"))
}

/// The engine's version, with Core API v1's `core_api_version`.
fn version() -> Value {
    let mut info = serde_json::to_value(Engine::version()).expect("version");
    info["core_api_version"] = json!(1);
    info
}
