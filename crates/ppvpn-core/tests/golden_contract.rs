//! The Rust engine against Go 0.5.21's contract golden
//! (testdata/golden/contract):
//!
//! - validation: every validate-profile and apply-profile step must reach
//!   the same outcome — accepted, or the same error code, field and
//!   retryable;
//! - scenarios (`SCENARIOS`): every step of the file, in order, on one
//!   Engine over the fake runtime: the response and the events it sent.
//!
//! Steps where the Rust engine deliberately differs are listed with their
//! expected outcome (docs/rust-parity.md, D1–D3, D5); steps that exist only
//! because Core API v1 is IPC are in `IPC_ONLY`.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use futures_util::FutureExt;
use ppvpn_core::internal::{engine_on_fake_runtime, validate_request};
use ppvpn_core::{
    ApplyRequest, Engine, EngineConfig, Error, EventItem, EventKind, Platform,
    ProbeAvailabilityRequest, Role, RoutingMode,
};
use serde_json::{json, Value};

/// (file, step, expected code and field): D3 — apply validates the profile
/// as given, as validate-profile does.
const DEPARTURES: &[(&str, &str, &str, &str)] = &[(
    "apply_dedupe",
    "apply_unknown_default_node_keeps_selection",
    "DEFAULT_NODE_NOT_FOUND",
    "selection.default_node_id",
)];

/// (file, step, why): the request is a function call with typed arguments,
/// not a JSON body, so these cannot be expressed.
const IPC_ONLY: &[(&str, &str, &str)] = &[(
    "validation",
    "request_invalid_unknown_field",
    "an unknown request body field (REQUEST_INVALID) has no library equivalent",
)];

/// (file, step, expected code): steps whose Go outcome came from the IPC
/// layer folding a decode error into CORE_OPERATION_FAILED; the library
/// reports the profile itself.
const STRUCTURED: &[(&str, &str, &str)] = &[("validation", "profile_missing", "PROFILE_REQUIRED")];

fn contract_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/golden/contract")
}

#[test]
fn validation_matches_the_go_golden() {
    let mut checked = 0;
    let mut failures = Vec::new();
    let mut files: Vec<_> = fs::read_dir(contract_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    for path in files {
        let file: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let name = file["name"].as_str().unwrap().to_owned();
        let steps = file["steps"].as_array().unwrap();
        let expects = file["expect"].as_array().unwrap();
        for (step, expect) in steps.iter().zip(expects) {
            let method = step["method"].as_str().unwrap();
            if method != "validate-profile" && method != "apply-profile" {
                continue;
            }
            let step_name = step["name"].as_str().unwrap_or("");
            if IPC_ONLY
                .iter()
                .any(|(f, s, _)| *f == name && *s == step_name)
            {
                continue;
            }
            let want = expected(&name, step_name, &expect["response"]);
            let got = outcome(step);
            checked += 1;
            if got != want {
                failures.push(format!("{name}/{step_name}: got {got:?}, want {want:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} steps differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(checked > 70, "only {checked} steps checked");
}

type Outcome = Option<(String, Option<String>, bool)>;

fn expected(file: &str, step: &str, response: &Value) -> Outcome {
    if let Some((_, _, code, field)) = DEPARTURES
        .iter()
        .find(|(f, s, _, _)| *f == file && *s == step)
    {
        return Some(((*code).into(), Some((*field).into()), false));
    }
    if let Some((_, _, code)) = STRUCTURED.iter().find(|(f, s, _)| *f == file && *s == step) {
        return Some(((*code).into(), None, false));
    }
    if response["ok"].as_bool() == Some(true) {
        return None;
    }
    let error = &response["error"];
    Some((
        error["code"].as_str().unwrap().into(),
        error["field"]
            .as_str()
            .filter(|f| !f.is_empty())
            .map(Into::into),
        error["retryable"].as_bool().unwrap_or(false),
    ))
}

/// The library's outcome for a step: the routing mode is parsed first (as
/// the API did), then the request is validated.
fn outcome(step: &Value) -> Outcome {
    let body = step
        .get("body")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let result = (|| {
        let routing_mode = RoutingMode::parse(body["routing_mode"].as_str().unwrap_or(""))?;
        let profile = match step.get("profile_ref") {
            Some(reference) => serde_json::to_vec(&resolve(reference)).unwrap(),
            None => body
                .get("profile")
                .map(|p| serde_json::to_vec(p).unwrap())
                .unwrap_or_default(),
        };
        let mut request = ApplyRequest::new(profile);
        request.routing_mode = routing_mode;
        request.allowed_rule_set_hosts = body["allowed_rule_set_hosts"]
            .as_array()
            .map(|hosts| {
                hosts
                    .iter()
                    .map(|h| h.as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        validate_request(&request, Utc::now()).map(|_| ())
    })();
    result
        .err()
        .map(|e| (e.code.to_owned(), e.field, e.retryable))
}

fn resolve(reference: &Value) -> Value {
    let base = reference["base"].as_str().unwrap();
    let mut doc: Value =
        serde_json::from_slice(&fs::read(contract_dir().join(base)).unwrap()).unwrap();
    for op in reference["patch"].as_array().into_iter().flatten() {
        apply_patch(&mut doc, op);
    }
    doc
}

/// RFC 6902 add, remove and replace (all the golden files use).
fn apply_patch(doc: &mut Value, op: &Value) {
    let path = op["path"].as_str().unwrap();
    let kind = op["op"].as_str().unwrap();
    if path.is_empty() {
        *doc = op["value"].clone();
        return;
    }
    let tokens: Vec<String> = path[1..]
        .split('/')
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect();
    let (last, parents) = tokens.split_last().unwrap();
    let mut target = doc;
    for token in parents {
        target = match target {
            Value::Object(map) => map.get_mut(token).unwrap(),
            Value::Array(items) => &mut items[token.parse::<usize>().unwrap()],
            _ => panic!("path through a scalar: {path}"),
        };
    }
    match (target, kind) {
        (Value::Object(map), "remove") => {
            map.remove(last).unwrap();
        }
        (Value::Object(map), _) => {
            map.insert(last.clone(), op["value"].clone());
        }
        (Value::Array(items), "remove") => {
            items.remove(last.parse::<usize>().unwrap());
        }
        (Value::Array(items), "replace") => {
            items[last.parse::<usize>().unwrap()] = op["value"].clone()
        }
        (Value::Array(items), _) if last == "-" => items.push(op["value"].clone()),
        (Value::Array(items), _) => {
            items.insert(last.parse::<usize>().unwrap(), op["value"].clone())
        }
        _ => panic!("bad patch target: {path}"),
    }
}

/// The scenario files run step by step.
const SCENARIOS: &[&str] = &["lifecycle", "apply_dedupe"];

/// (file, step, Rust's response, Rust's events): D1–D3. A response is the
/// golden's envelope; events are listed by their golden fields.
fn scenario_departure(file: &str, step: &str) -> Option<(Value, Value)> {
    let error = |code: &str, field: Option<&str>| {
        let mut error = json!({ "code": code, "retryable": false });
        if let Some(field) = field {
            error["field"] = field.into();
        }
        json!({ "ok": false, "error": error })
    };
    match (file, step) {
        // D1: no profile is PROFILE_NOT_APPLIED, and nothing is sent.
        ("lifecycle", "start_without_profile") => {
            Some((error("PROFILE_NOT_APPLIED", None), json!([])))
        }
        // D3: the profile as given; the applied one stays.
        ("apply_dedupe", "apply_unknown_default_node_keeps_selection") => Some((
            error("DEFAULT_NODE_NOT_FOUND", Some("selection.default_node_id")),
            json!([{ "type": "ReloadFailed" }]),
        )),
        // D2: an unknown node is NODE_NOT_FOUND, before a profile
        // PROFILE_NOT_APPLIED.
        ("selection", "select_unknown") => {
            Some((error("NODE_NOT_FOUND", Some("node_id")), json!([])))
        }
        ("selection", "select_before_profile") => {
            Some((error("PROFILE_NOT_APPLIED", None), json!([])))
        }
        _ => None,
    }
}

/// (file, step): D3's knock-on — r3 was refused, so r2 is still applied.
const STILL_R2: &[(&str, &str)] = &[
    ("apply_dedupe", "status_r3"),
    ("apply_dedupe", "status_still_r3"),
];

/// Fields the Rust engine adds to a status (section 5): not in Go's.
const RUST_ONLY_STATUS_FIELDS: &[&str] = &["dropped_log_lines"];

#[tokio::test]
async fn scenarios_match_the_go_golden() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for name in SCENARIOS {
        let file: Value =
            serde_json::from_slice(&fs::read(contract_dir().join(format!("{name}.json"))).unwrap())
                .unwrap();
        let state_dir =
            std::env::temp_dir().join(format!("ppvpn-core-golden-{name}-{}", std::process::id()));
        // No local proxy, no system proxy, no TUN: the golden's platform.
        let engine = engine_on_fake_runtime(EngineConfig::new(
            Role::Standard,
            Platform::Linux,
            state_dir,
        ));
        // StateChanged is new with the library; its tests are the engine's.
        let kinds: Vec<EventKind> = EventKind::ALL
            .iter()
            .copied()
            .filter(|k| *k != EventKind::StateChanged)
            .collect();
        let mut events = engine.subscribe(&kinds);
        let steps = file["steps"].as_array().unwrap();
        let expects = file["expect"].as_array().unwrap();
        for (step, expect) in steps.iter().zip(expects) {
            let step_name = step["name"].as_str().unwrap();
            let (mut want, want_events) = match scenario_departure(name, step_name) {
                Some(departure) => departure,
                None => (expect["response"].clone(), expect["events"].clone()),
            };
            if STILL_R2.contains(&(*name, step_name)) {
                want["data"]["revision"] = "2026-09-29T00:00:00Z#2".into();
            }
            let got = run_step(&engine, step).await;
            let mut got_events = Vec::new();
            while let Some(Some(item)) = events.recv().now_or_never() {
                match item {
                    EventItem::Event { event } => {
                        got_events.push(serde_json::to_value(event).unwrap())
                    }
                    other => panic!("{name}/{step_name}: {other:?}"),
                }
            }
            checked += 1;
            let method = step["method"].as_str().unwrap();
            if let Err(why) = same_response(method, &want, &got) {
                failures.push(format!(
                    "{name}/{step_name}: {why}\n  got  {got}\n  want {want}"
                ));
            }
            if let Err(why) = same_events(&want_events, &got_events) {
                failures.push(format!(
                    "{name}/{step_name}: events: {why}\n  got  {got_events:?}\n  want {want_events}"
                ));
            }
        }
        engine.shutdown().await.unwrap();
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} steps differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// One step through the library, as the golden's envelope.
async fn run_step(engine: &Engine, step: &Value) -> Value {
    let body = &step["body"];
    let text = |key: &str| body[key].as_str().unwrap_or("").to_owned();
    let result: Result<Value, Error> = match step["method"].as_str().unwrap() {
        "apply-profile" => match RoutingMode::parse(body["routing_mode"].as_str().unwrap_or("")) {
            Err(e) => Err(e),
            Ok(mode) => {
                let profile = match step.get("profile_ref") {
                    Some(reference) => serde_json::to_vec(&resolve(reference)).unwrap(),
                    None => Vec::new(),
                };
                engine
                    .apply(ApplyRequest::new(profile).with_routing_mode(mode))
                    .await
                    .map(|r| serde_json::to_value(r).unwrap())
            }
        },
        "start" => engine.start().await.map(|()| json!({})),
        "stop" => engine.stop().await.map(|()| json!({})),
        "get-status" => {
            let mut status = serde_json::to_value(engine.status()).unwrap();
            for field in RUST_ONLY_STATUS_FIELDS {
                status.as_object_mut().unwrap().remove(*field);
            }
            Ok(status)
        }
        "select-node" => engine
            .select_node(&text("node_id"))
            .await
            .map(|()| json!({ "node_id": text("node_id") })),
        "get-selected-node" => Ok(serde_json::to_value(engine.selected_node()).unwrap()),
        "list-nodes" => Ok(serde_json::to_value(engine.nodes()).unwrap()),
        "pin-ingress" => {
            let key = body["endpoint_key"].as_str();
            engine
                .pin_ingress(&text("node_id"), key)
                .await
                .map(|()| json!({ "node_id": text("node_id"), "endpoint_key": key }))
        }
        "probe-availability" => engine
            .probe_availability(ProbeAvailabilityRequest::new(
                text("node_id"),
                text("target"),
                body["timeout_ms"].as_u64().unwrap_or(0),
            ))
            .await
            .map(|r| serde_json::to_value(r).unwrap()),
        other => panic!("no library call for {other}"),
    };
    match result {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => {
            let mut error = json!({ "code": e.code, "retryable": e.retryable });
            if let Some(field) = e.field {
                error["field"] = field.into();
            }
            json!({ "ok": false, "error": error })
        }
    }
}

/// apply-profile's golden data holds `applied` only (Go's response); every
/// other response is compared whole.
fn same_response(method: &str, want: &Value, got: &Value) -> Result<(), String> {
    if want["ok"] != got["ok"] {
        return Err("ok differs".into());
    }
    if want["ok"] == false {
        return (want["error"] == got["error"])
            .then_some(())
            .ok_or_else(|| "error differs".into());
    }
    let same = if method == "apply-profile" {
        contains(&got["data"], &want["data"])
    } else {
        want["data"] == got["data"]
    };
    same.then_some(()).ok_or_else(|| "data differs".into())
}

/// Same kinds in the same order; each with the golden's fields (Rust may
/// add fields: `ReloadFailed.code`); `at` is not compared.
fn same_events(want: &Value, got: &[Value]) -> Result<(), String> {
    let want = want.as_array().unwrap();
    if want.len() != got.len() {
        return Err(format!("{} events, want {}", got.len(), want.len()));
    }
    for (w, g) in want.iter().zip(got) {
        if !contains(g, w) {
            return Err(format!("{g} lacks {w}"));
        }
    }
    Ok(())
}

/// Every field of `want` is in `got` with the same value (objects only at
/// the top; below that values compare whole).
fn contains(got: &Value, want: &Value) -> bool {
    match want.as_object() {
        Some(fields) => fields.iter().all(|(k, v)| got.get(k) == Some(v)),
        None => got == want,
    }
}
