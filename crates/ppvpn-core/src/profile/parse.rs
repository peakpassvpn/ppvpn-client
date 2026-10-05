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

/// Where the profile did not decode, without what serde quotes of its values
/// (a credential among them): the host may log or show the message. A
/// position exists only for a syntax error, not for a type error found after
/// the bytes were read.
fn malformed(err: serde_json::Error) -> Error {
    let message = if err.line() == 0 {
        format!("decode profile: {:?} error", err.classify())
    } else {
        format!(
            "decode profile: {:?} error at line {} column {}",
            err.classify(),
            err.line(),
            err.column()
        )
    };
    Error::invalid(codes::PROFILE_MALFORMED, "", message)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> String {
        let mut buf = [0u8; 12];
        getrandom::fill(&mut buf).unwrap();
        buf.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// serde quotes a mistyped value in its error text; the profile carries
    /// credentials, so the error the host gets names the kind of failure and
    /// where, never the value.
    #[test]
    fn a_malformed_profile_error_never_quotes_a_value() {
        let secret = secret();
        let ingress = |credentials: &str| {
            format!(
                r#"{{"schema_version": 1, "nodes": [{{"id": "n", "ingresses": [{{"replica_ordinal": 0, "credentials": {credentials}}}]}}]}}"#
            )
        };
        let cases = [
            (
                "string for a number",
                format!(r#"{{"schema_version": "{secret}"}}"#),
            ),
            ("string for an array", format!(r#"{{"nodes": "{secret}"}}"#)),
            (
                "string for an array of keys",
                ingress(&format!(
                    r#"{{"shadowsocks": {{"identity_keys": "{secret}"}}}}"#
                )),
            ),
            (
                "array for a key",
                ingress(&format!(
                    r#"{{"shadowsocks": {{"user_key": ["{secret}"]}}}}"#
                )),
            ),
            (
                "object for a password",
                ingress(&format!(
                    r#"{{"anytls": {{"password": {{"{secret}": 1}}}}}}"#
                )),
            ),
            ("broken JSON", format!(r#"{{"nodes": [{{"id": "{secret}" "#)),
            ("bare value", format!(r#"{{"nodes": {secret}}}"#)),
        ];
        for (case, json) in cases {
            let err = parse(json.as_bytes()).expect_err(case);
            assert_eq!(err.code, codes::PROFILE_MALFORMED, "{case}: {err:?}");
            let shown = format!("{err:?} {err}");
            assert!(!shown.contains(&secret), "{case}: {shown}");
            assert!(shown.contains("decode profile: "), "{case}: {shown}");
        }
    }

    #[test]
    fn a_syntax_error_says_where() {
        let err = parse(b"{\n  \"nodes\": [,]\n}").unwrap_err();
        assert!(
            err.message
                .starts_with("decode profile: Syntax error at line 2 column "),
            "{}",
            err.message
        );
    }

    /// Go: profile TestParseIgnoresUnknownFields. A backend that adds a
    /// field must not break the clients already out: unknown fields at the
    /// top level, in a node, its exit and an ingress are ignored, and the
    /// known ones beside them still read.
    #[test]
    fn unknown_fields_are_ignored_at_every_level() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/profiles/multi-ingress.json");
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut doc: Value = serde_json::from_slice(&data).unwrap();
        doc["future_top_level"] = serde_json::json!({"x": 1});
        let node = &mut doc["nodes"][0];
        node["future_node_field"] = "x".into();
        node["exit"]["country_code"] = "CN".into();
        node["exit"]["future_exit_field"] = true.into();
        node["ingresses"][0]["future_ingress_field"] = serde_json::json!([1]);

        let p = parse(&serde_json::to_vec(&doc).unwrap()).expect("unknown fields are ignored");
        assert_eq!(p.nodes[0].exit.country_code, "CN");
        crate::profile::validate(&p, chrono::Utc::now()).expect("and the profile validates");
    }
}
