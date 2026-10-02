//! Profile validation against Go 0.5.21's contract golden
//! (testdata/golden/contract): every validate-profile and apply-profile step
//! must reach the same outcome — accepted, or the same error code, field and
//! retryable. Steps where the Rust engine deliberately differs are listed in
//! `DEPARTURES` with their expected outcome (docs/rust-parity.md); steps that
//! exist only because Core API v1 is IPC are in `IPC_ONLY`.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use ppvpn_core::internal::validate_request;
use ppvpn_core::{ApplyRequest, RoutingMode};
use serde_json::Value;

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
