//! Semantic validation, a port of Go's `profile.Validate`: the same checks in
//! the same order, so the first error (code and field) is the same as Go
//! 0.5.21's (testdata/golden/contract/validation.json).

use std::collections::HashSet;
use std::net::IpAddr;

use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use chrono::{DateTime, Utc};

use super::addr::{is_public_unicast, parse_addr, parse_prefix};
use super::model::*;
use crate::error::{codes, Error};

type Result<T = ()> = std::result::Result<T, Error>;

fn invalid(code: &'static str, field: impl Into<String>, message: impl Into<String>) -> Error {
    Error::invalid(code, field, message)
}

/// Validates a decoded profile at `now`.
pub fn validate(p: &Profile, now: DateTime<Utc>) -> Result {
    if p.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(invalid(
            codes::SCHEMA_UNSUPPORTED,
            "schema_version",
            "only profile schema_version 1 is supported",
        ));
    }
    if p.revision.trim().is_empty() {
        return Err(invalid(
            codes::FIELD_REQUIRED,
            "revision",
            "revision is required",
        ));
    }
    if let (Some(generated), Some(expires)) = (p.generated_at, p.expires_at) {
        if expires <= generated {
            return Err(invalid(
                codes::TIME_RANGE_INVALID,
                "expires_at",
                "expiration must be after generation",
            ));
        }
    }
    if let Some(expires) = p.expires_at {
        if now >= expires {
            return Err(invalid(
                codes::PROFILE_EXPIRED,
                "expires_at",
                "profile has expired",
            ));
        }
    }
    if p.nodes.is_empty() {
        return Err(invalid(
            codes::FIELD_REQUIRED,
            "nodes",
            "at least one node is required",
        ));
    }
    let mut node_ids = HashSet::new();
    let mut endpoint_keys = HashSet::new();
    for (i, n) in p.nodes.iter().enumerate() {
        let base = format!("nodes[{i}]");
        if !stable_id(&n.id, 128, false) {
            return Err(invalid(
                codes::NODE_ID_INVALID,
                format!("{base}.id"),
                "node id is not stable or valid",
            ));
        }
        if !node_ids.insert(n.id.as_str()) {
            return Err(invalid(
                codes::NODE_ID_DUPLICATE,
                format!("{base}.id"),
                "node id must be unique",
            ));
        }
        if !stable_id(&n.entry_key, 64, false) {
            return Err(invalid(
                codes::ENTRY_KEY_INVALID,
                format!("{base}.entry_key"),
                "entry key is required and must be a stable identifier of at most 64 characters",
            ));
        }
        if !n.exit.ip.is_empty() && parse_addr(&n.exit.ip).is_none() {
            return Err(invalid(
                codes::EXIT_IP_INVALID,
                format!("{base}.exit.ip"),
                "exit IP must be a valid IP address",
            ));
        }
        if !n.capabilities.tcp && !n.capabilities.udp {
            return Err(invalid(
                codes::CAPABILITIES_INVALID,
                format!("{base}.capabilities"),
                "at least one network capability is required",
            ));
        }
        validate_ingresses(n, &base, &mut endpoint_keys)?;
    }
    if !node_ids.contains(p.selection.default_node_id.as_str()) {
        return Err(invalid(
            codes::DEFAULT_NODE_NOT_FOUND,
            "selection.default_node_id",
            "default node does not exist",
        ));
    }
    if p.selection.mode != "manual" {
        return Err(invalid(
            codes::SELECTION_MODE_UNSUPPORTED,
            "selection.mode",
            "only manual selection is supported",
        ));
    }
    validate_routing(&p.routing, &node_ids)
}

/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,max-1}$`; `colon` also allows `:` after the
/// first character (endpoint keys).
fn stable_id(value: &str, max: usize, colon: bool) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= max
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|&b| {
            b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' || (colon && b == b':')
        })
}

fn validate_ingresses<'a>(n: &'a Node, base: &str, endpoint_keys: &mut HashSet<&'a str>) -> Result {
    if n.ingresses.is_empty() {
        return Err(invalid(
            codes::FIELD_REQUIRED,
            format!("{base}.ingresses"),
            "at least one ingress is required",
        ));
    }
    if n.ingresses.len() > MAX_INGRESSES_PER_NODE {
        return Err(invalid(
            codes::INGRESS_COUNT_INVALID,
            format!("{base}.ingresses"),
            format!("at most {MAX_INGRESSES_PER_NODE} ingresses are supported"),
        ));
    }
    for (i, ingress) in n.ingresses.iter().enumerate() {
        let field = format!("{base}.ingresses[{i}]");
        match ingress.role.as_str() {
            "primary" if i != 0 => {
                return Err(invalid(
                    codes::INGRESS_ROLE_INVALID,
                    format!("{field}.role"),
                    "exactly one primary ingress is required and it must be listed first",
                ));
            }
            "backup" if i == 0 => {
                return Err(invalid(
                    codes::INGRESS_ROLE_INVALID,
                    format!("{field}.role"),
                    "the first ingress must be the primary",
                ));
            }
            "primary" | "backup" => {}
            _ => {
                return Err(invalid(
                    codes::INGRESS_ROLE_INVALID,
                    format!("{field}.role"),
                    "ingress role must be primary or backup",
                ))
            }
        }
        if !stable_id(&ingress.endpoint_key, 128, true) {
            return Err(invalid(
                codes::ENDPOINT_KEY_INVALID,
                format!("{field}.endpoint_key"),
                "endpoint key is required and must be a stable identifier of at most 128 characters",
            ));
        }
        if !endpoint_keys.insert(ingress.endpoint_key.as_str()) {
            return Err(invalid(
                codes::ENDPOINT_KEY_DUPLICATE,
                format!("{field}.endpoint_key"),
                "endpoint key must be unique within the profile",
            ));
        }
        if let Some(label) = &ingress.label {
            if !valid_label(label) {
                return Err(invalid(
                    codes::INGRESS_LABEL_INVALID,
                    format!("{field}.label"),
                    "ingress label must be 1-32 characters, trimmed, without control characters",
                ));
            }
        }
        if ingress.replica_ordinal < 0 {
            return Err(invalid(
                codes::REPLICA_ORDINAL_INVALID,
                format!("{field}.replica_ordinal"),
                "replica ordinal must be non-negative",
            ));
        }
        if i > 0 && ingress.replica_ordinal <= n.ingresses[i - 1].replica_ordinal {
            return Err(invalid(
                codes::REPLICA_ORDINAL_INVALID,
                format!("{field}.replica_ordinal"),
                "replica ordinals must be unique and strictly increasing in failover order",
            ));
        }
        validate_ingress(ingress, &field)?;
    }
    let primary = n.ingresses[0].capabilities;
    if (n.capabilities.tcp && !primary.tcp) || (n.capabilities.udp && !primary.udp) {
        return Err(invalid(
            codes::CAPABILITIES_INVALID,
            format!("{base}.capabilities"),
            "node capabilities must be supported by the primary (first) ingress",
        ));
    }
    Ok(())
}

fn validate_ingress(ingress: &Ingress, base: &str) -> Result {
    if !valid_domain(&ingress.endpoint.domain) {
        return Err(invalid(
            codes::FIELD_REQUIRED,
            format!("{base}.endpoint.domain"),
            "connection domain is required",
        ));
    }
    if ingress.endpoint.port == 0 {
        return Err(invalid(
            codes::PORT_INVALID,
            format!("{base}.endpoint.port"),
            "port must be non-zero",
        ));
    }
    if !ingress.endpoint.ip.is_empty()
        && !parse_addr(&ingress.endpoint.ip).is_some_and(|ip| is_public_unicast(&ip))
    {
        return Err(invalid(
            codes::ENTRY_IP_NOT_PUBLIC,
            format!("{base}.endpoint.ip"),
            "entry IP must be a public unicast address",
        ));
    }
    validate_credentials(ingress, base)?;
    if ingress
        .transport
        .as_ref()
        .is_some_and(|t| !t.kind.is_empty())
    {
        return Err(invalid(
            codes::TRANSPORT_UNSUPPORTED,
            format!("{base}.transport.type"),
            "transport is not supported",
        ));
    }
    if !ingress.capabilities.tcp && !ingress.capabilities.udp {
        return Err(invalid(
            codes::CAPABILITIES_INVALID,
            format!("{base}.capabilities"),
            "at least one network capability is required",
        ));
    }
    Ok(())
}

fn validate_credentials(ingress: &Ingress, base: &str) -> Result {
    let c = &ingress.credentials;
    let count = usize::from(c.shadowsocks.is_some())
        + usize::from(c.vless.is_some())
        + usize::from(c.anytls.is_some());
    if count != 1 {
        return Err(invalid(
            codes::CREDENTIALS_INVALID,
            format!("{base}.credentials"),
            "exactly one protocol credential object is required",
        ));
    }
    match ingress.protocol.as_str() {
        "shadowsocks" => {
            let Some(ss) = c
                .shadowsocks
                .as_ref()
                .filter(|ss| !ss.method.is_empty() && !ss.user_key.is_empty())
            else {
                return Err(invalid(
                    codes::CREDENTIALS_INVALID,
                    format!("{base}.credentials.shadowsocks"),
                    "shadowsocks credentials are incomplete",
                ));
            };
            let key_length = match ss.method.as_str() {
                "2022-blake3-aes-128-gcm" => 16,
                "2022-blake3-aes-256-gcm" | "2022-blake3-chacha20-poly1305" => 32,
                _ => {
                    return Err(invalid(
                        codes::SHADOWSOCKS_METHOD_UNSUPPORTED,
                        format!("{base}.credentials.shadowsocks.method"),
                        "only Shadowsocks 2022 methods are supported",
                    ));
                }
            };
            if !valid_key(&ss.user_key, key_length) {
                return Err(invalid(
                    codes::SHADOWSOCKS_KEY_INVALID,
                    format!("{base}.credentials.shadowsocks.user_key"),
                    "user key has invalid encoding or length",
                ));
            }
            for (i, key) in ss.identity_keys.iter().enumerate() {
                if !valid_key(key, key_length) {
                    return Err(invalid(
                        codes::CREDENTIALS_INVALID,
                        format!("{base}.credentials.shadowsocks.identity_keys[{i}]"),
                        "identity key cannot be empty",
                    ));
                }
            }
        }
        "vless" => {
            if !c.vless.as_ref().is_some_and(|v| valid_uuid(&v.uuid)) {
                return Err(invalid(
                    codes::CREDENTIALS_INVALID,
                    format!("{base}.credentials.vless"),
                    "VLESS credentials are incomplete",
                ));
            }
            let reality = ingress.tls.as_ref().and_then(|tls| {
                let reality = tls.reality.as_ref()?;
                (!tls.server_name.is_empty() && !reality.public_key.is_empty())
                    .then_some((tls, reality))
            });
            let Some((tls, reality)) = reality else {
                return Err(invalid(
                    codes::REALITY_REQUIRED,
                    format!("{base}.tls.reality"),
                    "VLESS REALITY settings are required",
                ));
            };
            // REALITY borrows a third-party site's SNI: it only has to be a
            // valid host name, not the endpoint domain.
            if !valid_domain(&tls.server_name) {
                return Err(invalid(
                    codes::TLS_SERVER_NAME_INVALID,
                    format!("{base}.tls.server_name"),
                    "REALITY server name must be a valid domain",
                ));
            }
            if !valid_reality_public_key(&reality.public_key) {
                return Err(invalid(
                    codes::REALITY_PUBLIC_KEY_INVALID,
                    format!("{base}.tls.reality.public_key"),
                    "REALITY public key must be a base64url X25519 key",
                ));
            }
            let short_id = &reality.short_id;
            if !short_id.bytes().all(|b| b.is_ascii_hexdigit())
                || short_id.len() % 2 != 0
                || short_id.len() > 16
            {
                return Err(invalid(
                    codes::REALITY_SHORT_ID_INVALID,
                    format!("{base}.tls.reality.short_id"),
                    "REALITY short ID must be even-length hexadecimal up to 16 characters",
                ));
            }
        }
        "anytls" => {
            if !c.anytls.as_ref().is_some_and(|a| !a.password.is_empty()) {
                return Err(invalid(
                    codes::CREDENTIALS_INVALID,
                    format!("{base}.credentials.anytls"),
                    "AnyTLS credentials are incomplete",
                ));
            }
            let Some(tls) = ingress
                .tls
                .as_ref()
                .filter(|tls| !tls.server_name.is_empty())
            else {
                return Err(invalid(
                    codes::TLS_REQUIRED,
                    format!("{base}.tls"),
                    "AnyTLS TLS settings are required",
                ));
            };
            if tls.server_name != ingress.endpoint.domain {
                return Err(invalid(
                    codes::TLS_SERVER_NAME_MISMATCH,
                    format!("{base}.tls.server_name"),
                    "TLS server name must equal endpoint domain",
                ));
            }
        }
        _ => {
            return Err(invalid(
                codes::PROTOCOL_UNSUPPORTED,
                format!("{base}.protocol"),
                "protocol is not supported",
            ))
        }
    }
    Ok(())
}

/// Go's base64 decoders are lenient about the unused bits of the last
/// character and skip line breaks; match them.
fn lenient(alphabet: &alphabet::Alphabet, padding: DecodePaddingMode) -> GeneralPurpose {
    GeneralPurpose::new(
        alphabet,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(padding),
    )
}

fn without_line_breaks(value: &str) -> String {
    value.chars().filter(|&c| c != '\r' && c != '\n').collect()
}

/// A base64 key of `want` bytes, padded (StdEncoding) or not (RawStdEncoding).
fn valid_key(value: &str, want: usize) -> bool {
    let value = without_line_breaks(value);
    let decoded = lenient(&alphabet::STANDARD, DecodePaddingMode::RequireCanonical)
        .decode(&value)
        .or_else(|_| lenient(&alphabet::STANDARD, DecodePaddingMode::RequireNone).decode(&value));
    decoded.is_ok_and(|bytes| bytes.len() == want)
}

/// The base64url, unpadded 32-byte X25519 key (RawURLEncoding).
fn valid_reality_public_key(value: &str) -> bool {
    lenient(&alphabet::URL_SAFE, DecodePaddingMode::RequireNone)
        .decode(without_line_breaks(value))
        .is_ok_and(|bytes| bytes.len() == 32)
}

/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$`
fn valid_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    groups.len() == 5
        && groups
            .iter()
            .zip(lengths)
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
        && matches!(groups[2].as_bytes()[0], b'1'..=b'5')
        && matches!(
            groups[3].as_bytes()[0],
            b'8' | b'9' | b'a' | b'b' | b'A' | b'B'
        )
}

/// An LDH host name: labels of 1-63 ASCII letters, digits and hyphens, not
/// starting or ending with a hyphen; at most 253 characters, no trailing dot.
pub(crate) fn valid_domain(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || value.ends_with('.') {
        return false;
    }
    value.split('.').all(|label| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes[0] != b'-'
            && bytes[bytes.len() - 1] != b'-'
            && bytes
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// A display label: 1-32 characters, already trimmed, no control characters.
fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label == label.trim()
        && label.chars().count() <= MAX_INGRESS_LABEL_LENGTH
        && !label.chars().any(char::is_control)
}

/// The profile's domain comparison: one trailing root dot removed, IDNA to an
/// A-label, lowercased; IP literals are not domains.
pub fn normalize_domain(value: &str) -> Option<String> {
    let value = value.strip_suffix('.').unwrap_or(value);
    if value.is_empty() || value.ends_with('.') {
        return None;
    }
    let ascii = idna::domain_to_ascii(value).ok()?.to_ascii_lowercase();
    if !valid_domain(&ascii) || ascii.parse::<IpAddr>().is_ok() {
        return None;
    }
    Some(ascii)
}

/// The canonical inclusive `start-end` port range.
pub fn parse_port_range(value: &str) -> std::result::Result<(u16, u16), &'static str> {
    if value.trim() != value || value.matches('-').count() != 1 {
        return Err("port range must use start-end");
    }
    let (start, end) = value.split_once('-').expect("one hyphen");
    let start = parse_port(start).ok_or("port range start must be between 1 and 65535")?;
    let end = parse_port(end).ok_or("port range end must be between 1 and 65535")?;
    if start > end {
        return Err("port range start must not exceed end");
    }
    Ok((start, end))
}

/// Decimal digits only (Go's ParseUint: no sign), 1-65535.
fn parse_port(value: &str) -> Option<u16> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u16>().ok().filter(|&port| port != 0)
}

fn validate_routing(r: &Routing, node_ids: &HashSet<&str>) -> Result {
    let rule_set_ids = super::ruleset::validate_rule_sets(&r.rule_sets)?;
    let mut rule_ids = HashSet::new();
    for (i, rule) in r.rules.iter().enumerate() {
        let base = format!("routing.rules[{i}]");
        if !stable_id(&rule.id, 128, false) {
            return Err(invalid(
                codes::RULE_ID_INVALID,
                format!("{base}.id"),
                "rule id is not stable or valid",
            ));
        }
        if !rule_ids.insert(rule.id.as_str()) {
            return Err(invalid(
                codes::RULE_ID_DUPLICATE,
                format!("{base}.id"),
                "rule id must be unique",
            ));
        }
        validate_match(&rule.matcher, &format!("{base}.match"))?;
        super::ruleset::validate_rule_set_refs(
            &rule.matcher.rule_set_ids,
            &rule_set_ids,
            &format!("{base}.match.rule_set_ids"),
        )?;
        validate_action(&rule.action, node_ids, &format!("{base}.action"))?;
    }
    validate_action(&r.final_action, node_ids, "routing.final")
}

fn validate_match(m: &RoutingMatch, field: &str) -> Result {
    if m.domains.is_empty()
        && m.domain_suffixes.is_empty()
        && m.ip_cidrs.is_empty()
        && !m.ip_is_private
        && m.protocols.is_empty()
        && m.ports.is_empty()
        && m.port_ranges.is_empty()
        && m.rule_set_ids.is_empty()
    {
        return Err(invalid(
            codes::RULE_MATCH_EMPTY,
            field,
            "at least one match condition is required",
        ));
    }
    validate_domains(&m.domains, &format!("{field}.domains"))?;
    validate_domains(&m.domain_suffixes, &format!("{field}.domain_suffixes"))?;
    let mut cidrs = HashSet::new();
    for (i, value) in m.ip_cidrs.iter().enumerate() {
        let Some(prefix) = parse_prefix(value) else {
            return Err(invalid(
                codes::CIDR_INVALID,
                format!("{field}.ip_cidrs[{i}]"),
                "CIDR must be valid IPv4 or IPv6",
            ));
        };
        if !cidrs.insert(prefix.masked()) {
            return Err(invalid(
                codes::CIDR_DUPLICATE,
                format!("{field}.ip_cidrs[{i}]"),
                "CIDR must be unique within a rule",
            ));
        }
    }
    let mut protocols = HashSet::new();
    for (i, protocol) in m.protocols.iter().enumerate() {
        if protocol != "tcp" && protocol != "udp" {
            return Err(invalid(
                codes::NETWORK_UNSUPPORTED,
                format!("{field}.protocols[{i}]"),
                "protocol must be tcp or udp",
            ));
        }
        if !protocols.insert(protocol.as_str()) {
            return Err(invalid(
                codes::NETWORK_DUPLICATE,
                format!("{field}.protocols[{i}]"),
                "protocol must be unique within a rule",
            ));
        }
    }
    let mut ports = HashSet::new();
    for (i, &port) in m.ports.iter().enumerate() {
        if port == 0 {
            return Err(invalid(
                codes::PORT_INVALID,
                format!("{field}.ports[{i}]"),
                "port must be non-zero",
            ));
        }
        if !ports.insert(port) {
            return Err(invalid(
                codes::PORT_DUPLICATE,
                format!("{field}.ports[{i}]"),
                "port must be unique within a rule",
            ));
        }
    }
    let mut ranges = HashSet::new();
    for (i, value) in m.port_ranges.iter().enumerate() {
        let range = parse_port_range(value).map_err(|message| {
            invalid(
                codes::PORT_RANGE_INVALID,
                format!("{field}.port_ranges[{i}]"),
                message,
            )
        })?;
        if !ranges.insert(range) {
            return Err(invalid(
                codes::PORT_RANGE_DUPLICATE,
                format!("{field}.port_ranges[{i}]"),
                "port range must be unique within a rule",
            ));
        }
    }
    Ok(())
}

fn validate_domains(values: &[String], field: &str) -> Result {
    let mut seen = HashSet::new();
    for (i, value) in values.iter().enumerate() {
        if value.contains(['*', '?']) {
            return Err(invalid(
                codes::DOMAIN_WILDCARD_UNSUPPORTED,
                format!("{field}[{i}]"),
                "wildcards are not supported",
            ));
        }
        let Some(normalized) = normalize_domain(value) else {
            return Err(invalid(
                codes::DOMAIN_INVALID,
                format!("{field}[{i}]"),
                "domain must be a valid IDNA name",
            ));
        };
        if !seen.insert(normalized) {
            return Err(invalid(
                codes::DOMAIN_DUPLICATE,
                format!("{field}[{i}]"),
                "domain must be unique within a rule",
            ));
        }
    }
    Ok(())
}

fn validate_action(action: &RoutingAction, node_ids: &HashSet<&str>, field: &str) -> Result {
    match action.kind.as_str() {
        "direct" | "reject" => {
            if !action.target.is_empty() || !action.node_id.is_empty() {
                return Err(invalid(
                    codes::ROUTING_ACTION_INVALID,
                    field,
                    "direct and reject actions cannot specify a target or node_id",
                ));
            }
        }
        "proxy" => match action.target.as_str() {
            "selected" if !action.node_id.is_empty() => {
                return Err(invalid(
                    codes::ROUTING_ACTION_INVALID,
                    format!("{field}.node_id"),
                    "selected proxy action cannot specify node_id",
                ));
            }
            "selected" => {}
            "node" if !node_ids.contains(action.node_id.as_str()) => {
                return Err(invalid(
                    codes::ROUTING_NODE_NOT_FOUND,
                    format!("{field}.node_id"),
                    "proxy action node does not exist",
                ));
            }
            "node" => {}
            _ => {
                return Err(invalid(
                    codes::ROUTING_TARGET_UNSUPPORTED,
                    format!("{field}.target"),
                    "proxy target must be selected or node",
                ))
            }
        },
        _ => {
            return Err(invalid(
                codes::ROUTING_ACTION_UNSUPPORTED,
                format!("{field}.type"),
                "action must be direct, reject, or proxy",
            ))
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_decode_like_go() {
        assert!(valid_key("AAAAAAAAAAAAAAAAAAAAAA==", 16));
        assert!(valid_key("AAAAAAAAAAAAAAAAAAAAAA", 16)); // RawStdEncoding
        assert!(valid_key("AAAAAAAAAAAAAAAAAAAAAB==", 16)); // trailing bits set: Go accepts
        assert!(!valid_key("short", 16));
        assert!(valid_reality_public_key(
            "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA"
        ));
        assert!(!valid_reality_public_key(
            "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA="
        ));
    }

    #[test]
    fn domains_normalise_like_go() {
        assert_eq!(
            normalize_domain("Example.COM.").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            normalize_domain("bücher.example").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(normalize_domain("1.2.3.4"), None);
        assert_eq!(normalize_domain("bad_domain!"), None);
        assert_eq!(normalize_domain("a.."), None);
    }

    #[test]
    fn port_ranges_parse_like_go() {
        assert_eq!(parse_port_range("80-90"), Ok((80, 90)));
        assert_eq!(parse_port_range("080-090"), Ok((80, 90)));
        assert!(parse_port_range("+80-90").is_err());
        assert!(parse_port_range("90-80").is_err());
        assert!(parse_port_range(" 80-90").is_err());
        assert!(parse_port_range("0-90").is_err());
    }

    #[test]
    fn uuids_follow_the_go_pattern() {
        assert!(valid_uuid("00000000-0000-4000-8000-000000000001"));
        assert!(!valid_uuid("00000000-0000-6000-8000-000000000001"));
        assert!(!valid_uuid("00000000-0000-4000-c000-000000000001"));
    }

    fn random<const N: usize>() -> [u8; N] {
        let mut buf = [0u8; N];
        getrandom::fill(&mut buf).unwrap();
        buf
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Go's validIngress: keyed by its domain, ordinal 0, with credentials
    /// made up for this run. No entry IP: a public one is outside the
    /// documentation ranges, and `addr::tests` covers which are accepted.
    fn ingress(protocol: &str, role: &str, domain: &str) -> Ingress {
        let mut ingress = Ingress {
            role: role.into(),
            endpoint_key: domain.into(),
            protocol: protocol.into(),
            endpoint: Endpoint {
                domain: domain.into(),
                ip: String::new(),
                port: 443,
            },
            capabilities: Capabilities {
                tcp: true,
                udp: true,
            },
            ..Ingress::default()
        };
        match protocol {
            "shadowsocks" => {
                ingress.credentials.shadowsocks = Some(ShadowsocksCredentials {
                    method: "2022-blake3-aes-128-gcm".into(),
                    identity_keys: Vec::new(),
                    user_key: base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        random::<16>(),
                    ),
                });
            }
            "vless" => {
                // A version 4 UUID of the RFC 4122 variant.
                let mut uuid = random::<16>();
                uuid[6] = 0x40 | (uuid[6] & 0x0f);
                uuid[8] = 0x80 | (uuid[8] & 0x3f);
                let h = hex(&uuid);
                ingress.credentials.vless = Some(VlessCredentials {
                    uuid: format!(
                        "{}-{}-{}-{}-{}",
                        &h[..8],
                        &h[8..12],
                        &h[12..16],
                        &h[16..20],
                        &h[20..]
                    ),
                    flow: "xtls-rprx-vision".into(),
                });
                ingress.tls = Some(Tls {
                    server_name: domain.into(),
                    reality: Some(Reality {
                        public_key: base64::Engine::encode(
                            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                            random::<32>(),
                        ),
                        short_id: "01".into(),
                    }),
                    ..Tls::default()
                });
            }
            "anytls" => {
                ingress.credentials.anytls = Some(AnyTlsCredentials {
                    password: hex(&random::<12>()),
                });
                ingress.tls = Some(Tls {
                    server_name: domain.into(),
                    ..Tls::default()
                });
            }
            other => panic!("no test ingress for {other}"),
        }
        ingress
    }

    fn backup(protocol: &str, ordinal: i64, domain: &str) -> Ingress {
        Ingress {
            replica_ordinal: ordinal,
            ..ingress(protocol, "backup", domain)
        }
    }

    /// Go's validProfile: one node, one primary ingress, no expiry.
    fn profile(protocol: &str) -> Profile {
        Profile {
            schema_version: CURRENT_SCHEMA_VERSION,
            revision: "r1".into(),
            nodes: vec![Node {
                id: "node-1".into(),
                name: "Tokyo".into(),
                entry_key: "cn-optimized".into(),
                exit: Exit {
                    ip: "203.0.113.9".into(),
                    region: "Tokyo".into(),
                    country_code: String::new(),
                },
                capabilities: Capabilities {
                    tcp: true,
                    udp: true,
                },
                ingresses: vec![ingress(protocol, "primary", "edge.example.com")],
                ..Node::default()
            }],
            selection: Selection {
                mode: "manual".into(),
                default_node_id: "node-1".into(),
            },
            routing: Routing {
                final_action: RoutingAction {
                    kind: "proxy".into(),
                    target: "selected".into(),
                    node_id: String::new(),
                },
                ..Routing::default()
            },
            ..Profile::default()
        }
    }

    /// The code and field `validate` rejects `p` with.
    fn rejection(p: &Profile) -> (&'static str, String) {
        let err = validate(p, Utc::now()).expect_err("accepted");
        (err.code, err.field.unwrap_or_default())
    }

    /// The Shadowsocks profile after `mutate`, rejected.
    fn rejected(mutate: impl FnOnce(&mut Profile)) -> (&'static str, String) {
        let mut p = profile("shadowsocks");
        mutate(&mut p);
        rejection(&p)
    }

    /// Go: profile TestIngressFailoverShapes.
    #[test]
    fn failover_shapes_fail_with_go_s_code_and_field() {
        let mut p = profile("shadowsocks");
        p.nodes[0].ingresses.extend([
            backup("vless", 1, "backup.example.com"),
            backup("anytls", 5, "backup2.example.com"),
        ]);
        validate(&p, Utc::now()).expect("a primary and two backups");

        let mut cases: Vec<(&str, &str, (&'static str, String))> = vec![
            (
                codes::FIELD_REQUIRED,
                "nodes[0].ingresses",
                rejected(|p| p.nodes[0].ingresses.clear()),
            ),
            // Roles: a backup first, an unknown role, a second primary.
            (
                codes::INGRESS_ROLE_INVALID,
                "nodes[0].ingresses[0].role",
                rejected(|p| p.nodes[0].ingresses[0].role = "backup".into()),
            ),
            (
                codes::INGRESS_ROLE_INVALID,
                "nodes[0].ingresses[0].role",
                rejected(|p| p.nodes[0].ingresses[0].role = "standby".into()),
            ),
            (
                codes::INGRESS_ROLE_INVALID,
                "nodes[0].ingresses[1].role",
                rejected(|p| {
                    p.nodes[0].ingresses.push(Ingress {
                        replica_ordinal: 1,
                        ..ingress("shadowsocks", "primary", "second.example.com")
                    })
                }),
            ),
            (
                codes::ENTRY_KEY_INVALID,
                "nodes[0].entry_key",
                rejected(|p| p.nodes[0].entry_key = String::new()),
            ),
            (
                codes::ENTRY_KEY_INVALID,
                "nodes[0].entry_key",
                rejected(|p| p.nodes[0].entry_key = "-cn".into()),
            ),
            (
                codes::ENTRY_KEY_INVALID,
                "nodes[0].entry_key",
                rejected(|p| p.nodes[0].entry_key = "a".repeat(65)),
            ),
            (
                codes::ENDPOINT_KEY_INVALID,
                "nodes[0].ingresses[0].endpoint_key",
                rejected(|p| p.nodes[0].ingresses[0].endpoint_key = String::new()),
            ),
            (
                codes::ENDPOINT_KEY_INVALID,
                "nodes[0].ingresses[0].endpoint_key",
                rejected(|p| p.nodes[0].ingresses[0].endpoint_key = "a/b".into()),
            ),
            (
                codes::ENDPOINT_KEY_INVALID,
                "nodes[0].ingresses[0].endpoint_key",
                rejected(|p| p.nodes[0].ingresses[0].endpoint_key = "a".repeat(129)),
            ),
            (
                codes::ENDPOINT_KEY_DUPLICATE,
                "nodes[0].ingresses[1].endpoint_key",
                rejected(|p| {
                    let key = p.nodes[0].ingresses[0].endpoint_key.clone();
                    p.nodes[0].ingresses.push(Ingress {
                        endpoint_key: key,
                        ..backup("shadowsocks", 1, "backup.example.com")
                    });
                }),
            ),
            // Unique across the whole profile, not just within a node.
            (
                codes::ENDPOINT_KEY_DUPLICATE,
                "nodes[1].ingresses[0].endpoint_key",
                rejected(|p| {
                    let other = Node {
                        id: "node-2".into(),
                        ..p.nodes[0].clone()
                    };
                    p.nodes.push(other);
                }),
            ),
            (
                codes::REPLICA_ORDINAL_INVALID,
                "nodes[0].ingresses[0].replica_ordinal",
                rejected(|p| p.nodes[0].ingresses[0].replica_ordinal = -1),
            ),
            // Ordinals strictly increase in failover order.
            (
                codes::REPLICA_ORDINAL_INVALID,
                "nodes[0].ingresses[1].replica_ordinal",
                rejected(|p| {
                    p.nodes[0]
                        .ingresses
                        .push(backup("shadowsocks", 0, "backup.example.com"))
                }),
            ),
            (
                codes::REPLICA_ORDINAL_INVALID,
                "nodes[0].ingresses[1].replica_ordinal",
                rejected(|p| {
                    p.nodes[0].ingresses[0].replica_ordinal = 3;
                    p.nodes[0]
                        .ingresses
                        .push(backup("shadowsocks", 2, "backup.example.com"));
                }),
            ),
            (
                codes::ENTRY_IP_NOT_PUBLIC,
                "nodes[0].ingresses[0].endpoint.ip",
                rejected(|p| p.nodes[0].ingresses[0].endpoint.ip = "not-an-ip".into()),
            ),
            // The node carries UDP; its primary does not.
            (
                codes::CAPABILITIES_INVALID,
                "nodes[0].capabilities",
                rejected(|p| p.nodes[0].ingresses[0].capabilities.udp = false),
            ),
            (
                codes::EXIT_IP_INVALID,
                "nodes[0].exit.ip",
                rejected(|p| p.nodes[0].exit.ip = "999.1.1.1".into()),
            ),
            // A backup's REALITY key is checked as a primary's is.
            (
                codes::REALITY_PUBLIC_KEY_INVALID,
                "nodes[0].ingresses[1].tls.reality.public_key",
                rejected(|p| {
                    let mut b = backup("vless", 1, "backup.example.com");
                    if let Some(reality) = b.tls.as_mut().and_then(|tls| tls.reality.as_mut()) {
                        reality.public_key = "not-a-key".into();
                    }
                    p.nodes[0].ingresses.push(b);
                }),
            ),
            (
                codes::INGRESS_COUNT_INVALID,
                "nodes[0].ingresses",
                rejected(|p| {
                    p.nodes[0].ingresses = vec![Ingress::default(); MAX_INGRESSES_PER_NODE + 1]
                }),
            ),
            // A backup's credentials are validated with the same rules.
            (
                codes::TLS_SERVER_NAME_MISMATCH,
                "nodes[0].ingresses[1].tls.server_name",
                rejected(|p| {
                    let mut b = backup("anytls", 1, "backup.example.com");
                    if let Some(tls) = b.tls.as_mut() {
                        tls.server_name = "other.example.com".into();
                    }
                    p.nodes[0].ingresses.push(b);
                }),
            ),
        ];
        // Go also rejects the label "\xff". A Rust String cannot hold
        // invalid UTF-8, and serde_json refuses such bytes when it parses
        // the profile (PROFILE_MALFORMED), so there is no case for it here.
        let long = "a".repeat(MAX_INGRESS_LABEL_LENGTH + 1);
        for label in ["", "   ", " Tokyo", "Tokyo\n2", "a\u{0}b", long.as_str()] {
            cases.push((
                codes::INGRESS_LABEL_INVALID,
                "nodes[0].ingresses[0].label",
                rejected(|p| p.nodes[0].ingresses[0].label = Some(label.into())),
            ));
        }
        for (i, (code, field, got)) in cases.iter().enumerate() {
            assert_eq!((got.0, got.1.as_str()), (*code, *field), "case {i}");
        }
    }

    /// Go: profile TestRealityServerNameIsBorrowed. REALITY borrows a third
    /// party's SNI, so its server name need not be the endpoint domain; it
    /// must still be a host name.
    #[test]
    fn a_reality_server_name_may_differ_from_the_endpoint_domain() {
        let mut p = profile("vless");
        let set = |p: &mut Profile, name: &str| {
            if let Some(tls) = p.nodes[0].ingresses[0].tls.as_mut() {
                tls.server_name = name.into();
            }
        };
        set(&mut p, "www.example.org");
        validate(&p, Utc::now()).expect("a borrowed REALITY server name");
        set(&mut p, "not a domain");
        assert_eq!(
            rejection(&p),
            (
                codes::TLS_SERVER_NAME_INVALID,
                "nodes[0].ingresses[0].tls.server_name".to_owned()
            )
        );
    }
}
