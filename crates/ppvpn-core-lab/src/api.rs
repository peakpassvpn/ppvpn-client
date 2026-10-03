//! The part of Core API v1 the lab scripts use (Go: api/server.go), over the
//! engine: the same envelope, authentication, error codes and log lines, so
//! test/lab and the netns CI run unchanged against this binary.
//!
//! Every method is forwarded to the engine as it is: no state of its own,
//! the engine's errors as they come (CORE_OPERATION_FAILED folded, as the Go
//! core folds it). Methods the engine has no call for (reload,
//! get-local-proxy-endpoints, debug/goroutines) answer API_NOT_FOUND.

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use ppvpn_core::{
    codes, ApplyRequest, Engine, EventItem, EventKind, ProbeAvailabilityRequest,
    ProbeEntrancesRequest, ProbeMethod, RoutingMode,
};

use crate::corelog::Logger;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::value::RawValue;

/// A method's data, serialised as the engine orders its fields (a
/// `serde_json::Value` would sort them).
type Value = Box<RawValue>;

/// As the Go core: 4 MiB.
pub const MAX_REQUEST_BYTES: usize = 4 << 20;

/// The paths logged on success too, so the log shows what led to a failure.
const LIFECYCLE_PATHS: &[&str] = &[
    "/v1/apply-profile",
    "/v1/start",
    "/v1/stop",
    "/v1/reload",
    "/v1/set-system-proxy",
    "/v1/pin-ingress",
];

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

/// Core API v1's own code for a body it cannot read.
const REQUEST_INVALID: &str = "REQUEST_INVALID";

/// A rejected call: the engine's error, or one of Core API v1's own checks
/// (the engine's `Error` is built by the engine only).
#[derive(Debug)]
pub struct Failure {
    code: String,
    field: String,
    message: String,
    retryable: bool,
}

impl Failure {
    fn new(code: &str, field: &str, message: &str) -> Self {
        Failure {
            code: code.into(),
            field: field.into(),
            message: message.into(),
            retryable: false,
        }
    }
}

impl From<ppvpn_core::Error> for Failure {
    fn from(err: ppvpn_core::Error) -> Self {
        Failure {
            code: err.code.into(),
            field: err.field.unwrap_or_default(),
            message: err.message,
            retryable: err.retryable,
        }
    }
}

/// Core API v1's request body: one shape for every method, unknown fields
/// rejected (Go: rawRequest with DisallowUnknownFields).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct RawRequest {
    profile: Option<Box<serde_json::value::RawValue>>,
    node_id: String,
    node_ids: Vec<String>,
    timeout_ms: i64,
    concurrency: i64,
    target: String,
    method: String,
    enabled: Option<bool>,
    allowed_rule_set_hosts: Vec<String>,
    routing_mode: String,
    /// pin-ingress: the ingress, or null for automatic.
    endpoint_key: Option<String>,
    /// get-local-proxy-credential: "node" (default) or "routed".
    kind: String,
}

fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into()).boxed()
}

fn envelope(
    status: StatusCode,
    id: &str,
    data: Option<Value>,
    error: Option<ApiError>,
) -> Response<Body> {
    let mut body = serde_json::to_vec(&Envelope {
        request_id: id,
        ok: error.is_none(),
        data,
        error,
    })
    .expect("envelope");
    body.push(b'\n');
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(full(body))
        .expect("response")
}

fn api_error(code: &str, message: &str) -> ApiError {
    ApiError {
        code: code.into(),
        message: message.into(),
        field: String::new(),
        retryable: false,
    }
}

fn request_id<B>(req: &Request<B>) -> String {
    if let Some(id) = req
        .headers()
        .get("X-Request-ID")
        .and_then(|v| v.to_str().ok())
    {
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
    provided.len() == secret.len()
        && provided
            .bytes()
            .zip(secret.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

impl Api {
    pub async fn handle(
        self: Arc<Self>,
        req: Request<Incoming>,
    ) -> Result<Response<Body>, Infallible> {
        let id = request_id(&req);
        let auth = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let provided = auth.strip_prefix("Bearer ").unwrap_or(auth);
        if !secret_matches(provided, &self.secret) {
            return Ok(envelope(
                StatusCode::UNAUTHORIZED,
                &id,
                None,
                Some(api_error(
                    "UNAUTHENTICATED",
                    "valid session authentication is required",
                )),
            ));
        }
        if let Some(v) = req.headers().get("X-Core-API-Version") {
            if v.as_bytes() != b"1" && !v.is_empty() {
                return Ok(envelope(
                    StatusCode::BAD_REQUEST,
                    &id,
                    None,
                    Some(api_error(
                        "CORE_API_UNSUPPORTED",
                        "requested Core API version is unsupported",
                    )),
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
            // Methods that read no body (Go: simple).
            "/v1/get-version" => Ok(version()),
            "/v1/start" => self
                .engine
                .start()
                .await
                .map(|()| to_value(json!({})))
                .map_err(Failure::from),
            "/v1/stop" => self
                .engine
                .stop()
                .await
                .map(|()| to_value(json!({})))
                .map_err(Failure::from),
            "/v1/get-status" => Ok(to_value(self.engine.status())),
            "/v1/list-nodes" => Ok(to_value(self.engine.nodes())),
            "/v1/get-selected-node" => {
                self.engine.selected_node().map(to_value).ok_or_else(|| {
                    Failure::new(codes::NODE_NOT_FOUND, "", "selected node not found")
                })
            }
            "/v1/get-local-proxy-metadata" => self
                .engine
                .local_proxy_metadata()
                .map(to_value)
                .map_err(Failure::from),
            "/v1/get-system-proxy-endpoints" => {
                let status = self.engine.status().system_proxy;
                if status.available {
                    Ok(to_value(status))
                } else {
                    Err(Failure::new(
                        codes::SYSTEM_PROXY_UNAVAILABLE,
                        "",
                        "this core cannot host the system proxy (TUN core or no state directory)",
                    ))
                }
            }
            "/v1/get-traffic" => Ok(to_value(self.engine.traffic())),
            "/v1/get-connections" => Ok(to_value(self.engine.connections())),
            // Methods with a body.
            "/v1/validate-profile"
            | "/v1/apply-profile"
            | "/v1/select-node"
            | "/v1/pin-ingress"
            | "/v1/probe-entrances"
            | "/v1/probe-availability"
            | "/v1/get-local-proxy-credential"
            | "/v1/set-system-proxy" => match decode(req).await {
                Ok(request) => self.with_body(&path, request).await,
                Err(response) => return Ok(response(&id)),
            },
            _ => return Ok(not_found(&id)),
        };
        Ok(self.respond(&path, &id, result))
    }

    async fn with_body(&self, path: &str, request: RawRequest) -> Result<Value, Failure> {
        let engine = &self.engine;
        match path {
            "/v1/validate-profile" => {
                Engine::validate(&apply_request(request)?)?;
                Ok(to_value(json!({ "valid": true })))
            }
            "/v1/apply-profile" => {
                let result = engine.apply(apply_request(request)?).await?;
                Ok(to_value(json!({ "applied": result.applied })))
            }
            "/v1/select-node" => {
                engine.select_node(&request.node_id).await?;
                Ok(to_value(json!({ "node_id": request.node_id })))
            }
            "/v1/pin-ingress" => {
                if request.endpoint_key.as_deref() == Some("") {
                    return Err(Failure::new(
                        codes::INGRESS_NOT_FOUND,
                        "endpoint_key",
                        "endpoint_key must be an ingress endpoint_key or null",
                    ));
                }
                engine
                    .pin_ingress(&request.node_id, request.endpoint_key.as_deref())
                    .await?;
                Ok(to_value(Pinned {
                    node_id: request.node_id,
                    endpoint_key: request.endpoint_key,
                }))
            }
            "/v1/probe-entrances" => {
                let method = match request.method.as_str() {
                    "" | "tcp" => ProbeMethod::Tcp,
                    "icmp" => ProbeMethod::Icmp,
                    _ => {
                        return Err(Failure::new(
                            codes::PROBE_METHOD_UNSUPPORTED,
                            "method",
                            "probe method must be tcp or icmp",
                        ))
                    }
                };
                let probe = ProbeEntrancesRequest::new(
                    method,
                    timeout_ms(request.timeout_ms, 5000),
                    request.concurrency.clamp(0, u32::MAX.into()) as u32,
                )
                .with_node_ids(request.node_ids);
                Ok(to_value(engine.probe_entrances(probe).await?))
            }
            "/v1/probe-availability" => {
                let probe = ProbeAvailabilityRequest::new(
                    request.node_id,
                    request.target,
                    timeout_ms(request.timeout_ms, 10000),
                );
                Ok(to_value(engine.probe_availability(probe).await?))
            }
            "/v1/get-local-proxy-credential" => match request.kind.as_str() {
                "" | "node" => Ok(to_value(engine.local_proxy_credential(&request.node_id)?)),
                "routed" if request.node_id.is_empty() => {
                    Ok(to_value(engine.local_proxy_routed_credential()?))
                }
                "routed" => Err(Failure::new(
                    REQUEST_INVALID,
                    "node_id",
                    "node_id must be empty for kind routed",
                )),
                _ => Err(Failure::new(
                    REQUEST_INVALID,
                    "kind",
                    "kind must be node or routed",
                )),
            },
            "/v1/set-system-proxy" => match request.enabled {
                Some(enabled) => Ok(to_value(engine.set_system_proxy_listener(enabled).await?)),
                None => Err(Failure::new(
                    REQUEST_INVALID,
                    "enabled",
                    "enabled is required",
                )),
            },
            _ => unreachable!("routed by handle"),
        }
    }

    fn respond(&self, path: &str, id: &str, result: Result<Value, Failure>) -> Response<Body> {
        match result {
            Ok(data) => {
                if LIFECYCLE_PATHS.contains(&path) {
                    self.log
                        .info("request ok", &[("path", &path), ("request_id", &id)]);
                }
                envelope(StatusCode::OK, id, Some(data), None)
            }
            Err(err) if err.code == codes::CORE_OPERATION_FAILED => {
                // The response stays folded; the cause goes to the log only.
                self.log.error(
                    "CORE_OPERATION_FAILED",
                    &[
                        ("path", &path),
                        ("request_id", &id),
                        ("error", &err.message),
                    ],
                );
                envelope(
                    StatusCode::BAD_REQUEST,
                    id,
                    None,
                    Some(api_error("CORE_OPERATION_FAILED", "core operation failed")),
                )
            }
            Err(err) => {
                // As Go: the field only for errors about one.
                if err.field.is_empty() {
                    self.log.info(
                        "request rejected",
                        &[("path", &path), ("request_id", &id), ("code", &err.code)],
                    );
                } else {
                    self.log.info(
                        "request rejected",
                        &[
                            ("path", &path),
                            ("request_id", &id),
                            ("code", &err.code),
                            ("field", &err.field),
                        ],
                    );
                }
                let error = ApiError {
                    code: err.code,
                    message: err.message,
                    field: err.field,
                    retryable: err.retryable,
                };
                envelope(StatusCode::BAD_REQUEST, id, None, Some(error))
            }
        }
    }

    /// NDJSON: one envelope per event, as the Go core streams them.
    fn watch_events(&self, id: &str) -> Response<Body> {
        let mut events = self.engine.subscribe(EventKind::ALL);
        let id = id.to_string();
        let stream = async_stream(move |tx| async move {
            while let Some(item) = events.recv().await {
                let EventItem::Event { event } = item else {
                    continue;
                };
                let data = to_value(&event);
                let mut line = serde_json::to_vec(&Envelope {
                    request_id: &id,
                    ok: true,
                    data: Some(data),
                    error: None,
                })
                .expect("envelope");
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
    envelope(
        StatusCode::NOT_FOUND,
        id,
        None,
        Some(api_error("API_NOT_FOUND", "Core API method was not found")),
    )
}

type Rejection = Box<dyn FnOnce(&str) -> Response<Body> + Send>;

/// One JSON value, at most MAX_REQUEST_BYTES, no unknown fields.
async fn decode(req: Request<Incoming>) -> Result<RawRequest, Rejection> {
    let invalid = |message: &'static str| -> Rejection {
        Box::new(move |id: &str| {
            envelope(
                StatusCode::BAD_REQUEST,
                id,
                None,
                Some(api_error("REQUEST_INVALID", message)),
            )
        })
    };
    let body = match Limited::new(req.into_body(), MAX_REQUEST_BYTES)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(_) => return Err(invalid("request body is invalid")),
    };
    let mut values =
        serde_json::Deserializer::from_slice(&body).into_iter::<Box<serde_json::value::RawValue>>();
    let Some(Ok(first)) = values.next() else {
        return Err(invalid("request body is invalid"));
    };
    if values.next().is_some() {
        return Err(invalid("request body must contain exactly one JSON value"));
    }
    serde_json::from_str(first.get()).map_err(|_| invalid("request body is invalid"))
}

fn to_value(value: impl Serialize) -> Value {
    serde_json::value::to_raw_value(&value).expect("serialisable")
}

/// pin-ingress's data, in Go's field order.
#[derive(Serialize)]
struct Pinned {
    node_id: String,
    endpoint_key: Option<String>,
}

/// get-version's data: the engine's, with Core API v1's version.
#[derive(Serialize)]
struct Version {
    #[serde(flatten)]
    engine: ppvpn_core::VersionInfo,
    core_api_version: u32,
}

/// Go's duration(): 0 or less is the default; at most 120 s.
fn timeout_ms(ms: i64, default: u64) -> u64 {
    if ms <= 0 {
        default
    } else {
        ms.min(120_000) as u64
    }
}

/// apply-profile's and validate-profile's request.
fn apply_request(request: RawRequest) -> Result<ApplyRequest, Failure> {
    let mode = RoutingMode::parse(&request.routing_mode)?;
    let profile = request
        .profile
        .map(|p| p.get().as_bytes().to_vec())
        .unwrap_or_default();
    Ok(ApplyRequest::new(profile)
        .with_routing_mode(mode)
        .with_allowed_rule_set_hosts(request.allowed_rule_set_hosts))
}

/// The engine's version, with Core API v1's `core_api_version`.
fn version() -> Value {
    to_value(Version {
        engine: Engine::version(),
        core_api_version: 1,
    })
}
