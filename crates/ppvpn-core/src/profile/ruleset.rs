//! Rule set declarations and the host pinning of their URLs (Go's
//! `profile/ruleset.go`).

use std::collections::HashSet;
use std::time::Duration;

use super::model::{Profile, RuleSet, MAX_RULE_SETS};
use crate::error::{codes, Error};

/// The refresh interval when `update_interval_seconds` is absent or 0.
pub const DEFAULT_RULE_SET_UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// `update_interval_seconds` is clamped to this range.
pub const MIN_RULE_SET_UPDATE_INTERVAL: Duration = Duration::from_secs(3600);
pub const MAX_RULE_SET_UPDATE_INTERVAL: Duration = Duration::from_secs(7 * 24 * 3600);

impl RuleSet {
    /// `update_interval_seconds` clamped to [1 h, 7 d], or 24 h when absent.
    pub fn update_interval(&self) -> Duration {
        match u64::try_from(self.update_interval_seconds) {
            Ok(0) | Err(_) => DEFAULT_RULE_SET_UPDATE_INTERVAL,
            Ok(seconds) => Duration::from_secs(seconds)
                .clamp(MIN_RULE_SET_UPDATE_INTERVAL, MAX_RULE_SET_UPDATE_INTERVAL),
        }
    }
}

fn stable_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

pub(super) fn validate_rule_sets(sets: &[RuleSet]) -> Result<HashSet<&str>, Error> {
    if sets.len() > MAX_RULE_SETS {
        return Err(Error::invalid(
            codes::RULE_SET_COUNT_INVALID,
            "routing.rule_sets",
            format!("at most {MAX_RULE_SETS} rule sets are supported"),
        ));
    }
    let mut ids = HashSet::new();
    for (i, set) in sets.iter().enumerate() {
        let base = format!("routing.rule_sets[{i}]");
        if !stable_id(&set.id) {
            return Err(Error::invalid(
                codes::RULE_SET_ID_INVALID,
                format!("{base}.id"),
                "rule set id is not stable or valid",
            ));
        }
        if !ids.insert(set.id.as_str()) {
            return Err(Error::invalid(
                codes::RULE_SET_ID_DUPLICATE,
                format!("{base}.id"),
                "rule set id must be unique",
            ));
        }
        if let Err(message) = rule_set_host(&set.url) {
            return Err(Error::invalid(
                codes::RULE_SET_URL_INVALID,
                format!("{base}.url"),
                message,
            ));
        }
        if set.sha256.len() != 64 || !set.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::invalid(
                codes::RULE_SET_SHA256_INVALID,
                format!("{base}.sha256"),
                "sha256 must be 64 hexadecimal characters",
            ));
        }
        if set.update_interval_seconds < 0 {
            return Err(Error::invalid(
                codes::RULE_SET_INTERVAL_INVALID,
                format!("{base}.update_interval_seconds"),
                "update interval must not be negative",
            ));
        }
    }
    Ok(ids)
}

pub(super) fn validate_rule_set_refs(
    ids: &[String],
    known: &HashSet<&str>,
    field: &str,
) -> Result<(), Error> {
    let mut seen = HashSet::new();
    for (i, id) in ids.iter().enumerate() {
        if !known.contains(id.as_str()) {
            return Err(Error::invalid(
                codes::RULE_SET_NOT_FOUND,
                format!("{field}[{i}]"),
                "rule set does not exist",
            ));
        }
        if !seen.insert(id.as_str()) {
            return Err(Error::invalid(
                codes::RULE_SET_REF_DUPLICATE,
                format!("{field}[{i}]"),
                "rule set must be unique within a rule",
            ));
        }
    }
    Ok(())
}

/// The normalised host of an https rule set URL: the lowercase authority
/// with a default :443 removed. Userinfo, fragments, non-https schemes and
/// relative or opaque URLs are rejected.
pub fn rule_set_host(raw: &str) -> Result<String, &'static str> {
    let Some((scheme, rest)) = raw.split_once(':') else {
        return Err("url must be an absolute https URL");
    };
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
    {
        return Err("url must be an absolute https URL");
    }
    let Some(rest) = rest.strip_prefix("//") else {
        return Err("url must be an absolute https URL"); // opaque, as "https:host"
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return Err("url must use https");
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') || rest.contains('#') {
        return Err("url must not contain userinfo or a fragment");
    }
    normalize_rule_set_host(authority)
}

/// A host or host:port authority for pinning: lowercase, default port 443
/// removed, IPv6 literals bracketed.
pub fn normalize_rule_set_host(authority: &str) -> Result<String, &'static str> {
    if authority.is_empty() || authority.contains(['/', '?', '#', '@', ' ']) {
        return Err("url host is invalid");
    }
    let (host, port) = split_host_port(authority);
    if host.is_empty() {
        return Err("url host is invalid");
    }
    if !port.is_empty() {
        let valid = port.bytes().all(|b| b.is_ascii_digit())
            && !port.starts_with('0')
            && port.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n));
        if !valid {
            return Err("url port is invalid");
        }
    }
    let mut host = host.to_ascii_lowercase();
    if host.contains(':') {
        host = format!("[{host}]");
    }
    if port.is_empty() || port == "443" {
        Ok(host)
    } else {
        Ok(format!("{host}:{port}"))
    }
}

/// Go's `net.SplitHostPort`, falling back to the whole authority (unbracketed
/// when it is a bracketed IPv6 literal without a port) as the host.
fn split_host_port(authority: &str) -> (&str, &str) {
    if let Some(inner) = authority.strip_prefix('[') {
        if let Some((host, rest)) = inner.split_once(']') {
            if rest.is_empty() {
                return (host, "");
            }
            if let Some(port) = rest.strip_prefix(':') {
                if !host.contains(['[', ']']) && !port.contains(':') {
                    return (host, port);
                }
            }
        }
        return (authority, "");
    }
    match authority.matches(':').count() {
        1 => authority.split_once(':').expect("one colon"),
        _ => (authority, ""),
    }
}

/// Pins every rule set URL to one of `allowed` (the authorities of the API
/// the host fetched the profile from).
pub fn validate_rule_set_hosts(p: &Profile, allowed: &[String]) -> Result<(), Error> {
    let mut pinned = HashSet::new();
    for value in allowed {
        let host = normalize_rule_set_host(value).map_err(|_| {
            Error::invalid(
                codes::RULE_SET_HOSTS_INVALID,
                "allowed_rule_set_hosts",
                "allowed rule set host is invalid",
            )
        })?;
        pinned.insert(host);
    }
    for (i, set) in p.routing.rule_sets.iter().enumerate() {
        let host = rule_set_host(&set.url).map_err(|message| {
            Error::invalid(
                codes::RULE_SET_URL_INVALID,
                format!("routing.rule_sets[{i}].url"),
                message,
            )
        })?;
        if !pinned.contains(&host) {
            return Err(Error::invalid(
                codes::RULE_SET_HOST_NOT_ALLOWED,
                format!("routing.rule_sets[{i}].url"),
                "rule set host must be the host the profile came from",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_normalise_like_go() {
        assert_eq!(
            rule_set_host("https://Rules.Example.com/cn.srs"),
            Ok("rules.example.com".into())
        );
        assert_eq!(
            rule_set_host("https://rules.example.com:443/x"),
            Ok("rules.example.com".into())
        );
        assert_eq!(
            rule_set_host("https://rules.example.com:8443/x"),
            Ok("rules.example.com:8443".into())
        );
        assert_eq!(
            rule_set_host("https://[2001:db8::1]:443/x"),
            Ok("[2001:db8::1]".into())
        );
        assert!(rule_set_host("http://rules.example.com/x").is_err());
        assert!(rule_set_host("https://user@rules.example.com/x").is_err());
        assert!(rule_set_host("https://rules.example.com/x#frag").is_err());
        assert!(rule_set_host("https:rules.example.com").is_err());
        assert!(rule_set_host("https://rules.example.com:0443/x").is_err());
        assert!(normalize_rule_set_host("https://bad/").is_err());
    }
}
