//! Core API v1 served from an in-process [`ppvpn_core::Engine`]
//! (docs/host-integration.md): the request paths and JSON shapes the desktop
//! client speaks, answered by engine calls. The client's standard instance
//! calls [`dispatch`] directly; the privileged service answers the client's
//! forwarded calls for the TUN instance with it.

use ppvpn_core::{
    ApplyRequest, Engine, EngineState, Event, EventItem, EventReceiver, ProbeAvailabilityRequest,
    ProbeEntrancesRequest, RoutingMode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The `error` object of a failed call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl ApiError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    fn request_invalid(message: impl Into<String>) -> Self {
        Self::new("REQUEST_INVALID", message)
    }
}

impl From<ppvpn_core::Error> for ApiError {
    fn from(error: ppvpn_core::Error) -> Self {
        Self {
            code: error.code.to_string(),
            message: error.message,
            retryable: error.retryable,
        }
    }
}

/// Answers one Core API v1 call. Paths it does not serve (the event stream:
/// hosts use [`Engine::subscribe`]) fail with `CORE_API_UNSUPPORTED`.
pub async fn dispatch(engine: &Engine, path: &str, body: Value) -> Result<Value, ApiError> {
    match path {
        "/v1/get-version" => {
            let mut version = data(Engine::version())?;
            version["core_api_version"] = json!(1);
            Ok(version)
        }
        "/v1/validate-profile" => {
            reply(Engine::validate(&apply_request(&body)?).map(|()| json!({ "valid": true })))
        }
        "/v1/apply-profile" => reply(engine.apply(apply_request(&body)?).await),
        "/v1/start" => reply(engine.start().await.map(|()| json!({}))),
        "/v1/stop" => reply(engine.stop().await.map(|()| json!({}))),
        "/v1/get-status" => status(engine),
        "/v1/list-nodes" => data(engine.nodes()),
        "/v1/get-selected-node" => data(engine.selected_node()),
        "/v1/select-node" => {
            let node_id = string(&body, "node_id")?;
            reply(engine.select_node(&node_id).await.map(|()| json!({})))
        }
        "/v1/pin-ingress" => {
            let node_id = string(&body, "node_id")?;
            let endpoint_key = body.get("endpoint_key").and_then(Value::as_str);
            reply(
                engine
                    .pin_ingress(&node_id, endpoint_key)
                    .await
                    .map(|()| json!({})),
            )
        }
        "/v1/probe-entrances" => {
            let request: ProbeEntrancesRequest = decode(body)?;
            reply(engine.probe_entrances(request).await)
        }
        "/v1/probe-availability" => {
            let request: ProbeAvailabilityRequest = decode(body)?;
            reply(engine.probe_availability(request).await)
        }
        "/v1/get-local-proxy-metadata" => reply(engine.local_proxy_metadata()),
        "/v1/get-local-proxy-credential" => {
            if body.get("kind").and_then(Value::as_str) == Some("routed") {
                reply(engine.local_proxy_routed_credential())
            } else {
                reply(engine.local_proxy_credential(&string(&body, "node_id")?))
            }
        }
        "/v1/set-system-proxy" => {
            let enabled = body
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| ApiError::request_invalid("enabled is required"))?;
            reply(engine.set_system_proxy_listener(enabled).await)
        }
        "/v1/get-traffic" => data(engine.traffic()),
        "/v1/get-connections" => data(engine.connections()),
        _ => Err(ApiError::new(
            "CORE_API_UNSUPPORTED",
            format!("{path} is not served by the in-process engine"),
        )),
    }
}

/// Waits for the engine to reach `Fatal` and returns the reason as JSON.
/// Logs `Degraded` reasons on the way: the engine heals those itself. Never
/// returns once the subscription closes (the engine shut down), so the
/// host's own stop decides then.
pub async fn until_fatal(engine: &Engine, mut states: EventReceiver) -> String {
    // A Fatal between Engine::new and the subscription has no event.
    if let Some(reason) = fatal_reason(&engine.status().state) {
        return reason;
    }
    loop {
        let state = match states.recv().await {
            Some(EventItem::Event {
                event: Event::StateChanged { state, .. },
            }) => state,
            // Fell behind: the current state is what counts.
            Some(EventItem::Lagged { .. }) => engine.status().state,
            Some(_) => continue,
            None => std::future::pending().await,
        };
        if let Some(reason) = fatal_reason(&state) {
            return reason;
        }
    }
}

/// The reason of a `Fatal` state as JSON; logs the reasons of a `Degraded`.
pub fn fatal_reason(state: &EngineState) -> Option<String> {
    match state {
        EngineState::Fatal { reason } => {
            let reason = serde_json::to_string(reason).unwrap_or_default();
            tracing::warn!(%reason, "engine fatal");
            Some(reason)
        }
        EngineState::Degraded { reasons } => {
            let reasons = serde_json::to_string(reasons).unwrap_or_default();
            tracing::info!(%reasons, "engine degraded");
            None
        }
        _ => None,
    }
}

/// `get-status`. A degraded engine still forwards, so it reads `running`;
/// `fatal` stays as is.
fn status(engine: &Engine) -> Result<Value, ApiError> {
    let mut status = data(engine.status())?;
    if status["state"] == "degraded" {
        status["state"] = json!("running");
    }
    Ok(status)
}

/// The `apply-profile` / `validate-profile` body as an [`ApplyRequest`].
fn apply_request(body: &Value) -> Result<ApplyRequest, ApiError> {
    // A missing profile stays empty: the engine reports PROFILE_REQUIRED.
    let profile = match body.get("profile") {
        None | Some(Value::Null) => Vec::new(),
        Some(profile) => serde_json::to_vec(profile)
            .map_err(|error| ApiError::request_invalid(format!("encode profile: {error}")))?,
    };
    let mode = body
        .get("routing_mode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mode = RoutingMode::parse(mode)?;
    let hosts = body
        .get("allowed_rule_set_hosts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    Ok(ApplyRequest::new(profile)
        .with_routing_mode(mode)
        .with_allowed_rule_set_hosts(hosts))
}

fn reply<T: Serialize>(result: Result<T, ppvpn_core::Error>) -> Result<Value, ApiError> {
    data(result?)
}

fn data<T: Serialize>(value: T) -> Result<Value, ApiError> {
    serde_json::to_value(value)
        .map_err(|error| ApiError::new("INTERNAL", format!("encode engine reply: {error}")))
}

fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T, ApiError> {
    serde_json::from_value(body).map_err(|error| ApiError::request_invalid(error.to_string()))
}

fn string(body: &Value, key: &str) -> Result<String, ApiError> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ApiError::request_invalid(format!("{key} is required")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_errors_keep_their_code() {
        let error = Engine::validate(&ApplyRequest::new(Vec::new())).unwrap_err();
        assert_eq!(
            ApiError::from(error),
            ApiError {
                code: "PROFILE_REQUIRED".to_string(),
                message: "profile is required".to_string(),
                retryable: false,
            }
        );
    }

    #[test]
    fn only_fatal_ends_the_engine() {
        use ppvpn_core::{DegradedReason, FatalReason};
        let fatal = fatal_reason(&EngineState::Fatal {
            reason: FatalReason::TunDeviceLost,
        })
        .unwrap();
        assert!(fatal.contains("tun_device_lost"), "{fatal}");
        let degraded = EngineState::Degraded {
            reasons: vec![DegradedReason::NoDefaultInterface],
        };
        assert_eq!(fatal_reason(&degraded), None);
        assert_eq!(fatal_reason(&EngineState::Running), None);
        assert_eq!(fatal_reason(&EngineState::Stopped), None);
    }
}
