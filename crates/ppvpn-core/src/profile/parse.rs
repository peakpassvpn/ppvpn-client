//! Strict decoding of the profile bytes, as Go's `profile.Parse`: unknown
//! fields are ignored, `null` reads as absent, a removed field is rejected
//! with a code, and `replica_ordinal` (whose zero is a valid value) must be
//! present.

use serde_json::Value;

use super::model::Profile;
use crate::error::{codes, Error};

/// Decodes a profile. Validation is separate ([`super::validate`]).
pub fn parse(data: &[u8]) -> Result<Profile, Error> {
    if data.iter().all(u8::is_ascii_whitespace) {
        return Err(Error::invalid(
            codes::PROFILE_REQUIRED,
            "",
            "profile is required",
        ));
    }
    let mut value: Value = serde_json::from_slice(data).map_err(malformed)?;
    reject_removed_fields(&value)?;
    require_presence(&value)?;
    // Go decodes a top-level `null` into the zero profile (which then fails
    // validation on its schema version).
    if value.is_null() {
        value = Value::Object(Default::default());
    }
    strip_nulls(&mut value);
    serde_json::from_value(value).map_err(malformed)
}

fn malformed(err: serde_json::Error) -> Error {
    Error::invalid(
        codes::PROFILE_MALFORMED,
        "",
        format!("decode profile: {err}"),
    )
}

fn ingresses(value: &Value) -> impl Iterator<Item = (usize, usize, &Value)> {
    value
        .get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .flat_map(|(i, node)| {
            node.get("ingresses")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
                .map(move |(j, ingress)| (i, j, ingress))
        })
}

/// `shadowsocks.server_key` was replaced by `identity_keys` + `user_key`; a
/// backend still sending it had the two roles swapped.
fn reject_removed_fields(value: &Value) -> Result<(), Error> {
    for (i, j, ingress) in ingresses(value) {
        let removed = ingress
            .pointer("/credentials/shadowsocks")
            .and_then(Value::as_object)
            .is_some_and(|shadowsocks| shadowsocks.contains_key("server_key"));
        if removed {
            return Err(Error::invalid(
                codes::SHADOWSOCKS_SERVER_KEY_REMOVED,
                format!("nodes[{i}].ingresses[{j}].credentials.shadowsocks.server_key"),
                "server_key was removed; send the server iPSKs as identity_keys and the user uPSK as user_key",
            ));
        }
    }
    Ok(())
}

/// `replica_ordinal` 0 is legal but must be sent.
fn require_presence(value: &Value) -> Result<(), Error> {
    for (i, j, ingress) in ingresses(value) {
        if ingress.get("replica_ordinal").is_none_or(Value::is_null) {
            return Err(Error::invalid(
                codes::FIELD_REQUIRED,
                format!("nodes[{i}].ingresses[{j}].replica_ordinal"),
                "replica ordinal is required",
            ));
        }
    }
    Ok(())
}

/// Go decodes `null` as "leave the zero value"; serde would fail on it for a
/// non-optional field.
fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}
