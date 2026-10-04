//! Go's `profile` tests, ported: the same inputs and the same expected codes
//! (docs/rust-parity.md, profile). Each test names the Go test it follows.

use chrono::{Duration, Utc};
use serde_json::{json, Value};

use super::*;
use crate::error::Error;

const TEST_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Go's `validIngress`: the replica is keyed by its domain, ordinal 0.
fn ingress(protocol: &str, role: &str, domain: &str, ip: &str) -> Ingress {
    let mut ingress = Ingress {
        role: role.into(),
        endpoint_key: domain.into(),
        protocol: protocol.into(),
        endpoint: Endpoint {
            domain: domain.into(),
            ip: ip.into(),
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
                user_key: "AAAAAAAAAAAAAAAAAAAAAA==".into(),
                ..ShadowsocksCredentials::default()
            })
        }
        "vless" => {
            ingress.credentials.vless = Some(VlessCredentials {
                uuid: "00000000-0000-4000-8000-000000000001".into(),
                flow: "xtls-rprx-vision".into(),
            });
            ingress.tls = Some(Tls {
                server_name: domain.into(),
                reality: Some(Reality {
                    public_key: "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA".into(),
                    short_id: "01".into(),
                }),
                ..Tls::default()
            });
        }
        "anytls" => {
            // Unique to the run: not a credential anyone uses.
            ingress.credentials.anytls = Some(AnyTlsCredentials {
                password: format!("test-{}", std::process::id()),
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

/// Go's `backupIngress`.
fn backup(protocol: &str, ordinal: i64, domain: &str, ip: &str) -> Ingress {
    Ingress {
        replica_ordinal: ordinal,
        ..ingress(protocol, "backup", domain, ip)
    }
}

/// Go's `validProfile`: one node with one primary ingress.
fn profile(protocol: &str) -> Profile {
    Profile {
        schema_version: CURRENT_SCHEMA_VERSION,
        revision: "r1".into(),
        expires_at: Some((Utc::now() + Duration::hours(1)).fixed_offset()),
        nodes: vec![Node {
            id: "node-1".into(),
            name: "Tokyo".into(),
            entry_key: "cn-optimized".into(),
            exit: Exit {
                ip: "203.0.114.9".into(),
                region: "Tokyo".into(),
                ..Exit::default()
            },
            capabilities: Capabilities {
                tcp: true,
                udp: true,
            },
            ingresses: vec![ingress(protocol, "primary", "edge.example.com", "8.8.8.8")],
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
                ..RoutingAction::default()
            },
            ..Routing::default()
        },
        ..Profile::default()
    }
}

/// Go's `ruleSetProfile`.
fn rule_set_profile() -> Profile {
    let mut p = profile("shadowsocks");
    p.routing.rule_sets = vec![RuleSet {
        id: "cn-ip".into(),
        url: "https://api.example.com/api/v1/proxy-profile/rule-sets/cn-ip.srs".into(),
        sha256: TEST_SHA.into(),
        update_interval_seconds: 86400,
    }];
    p.routing.rules = vec![RoutingRule {
        id: "geoip-cn".into(),
        matcher: RoutingMatch {
            rule_set_ids: vec!["cn-ip".into()],
            ..RoutingMatch::default()
        },
        action: RoutingAction {
            kind: "direct".into(),
            ..RoutingAction::default()
        },
        ..RoutingRule::default()
    }];
    p
}

/// The validation code, or "" when the profile is valid.
fn code(p: &Profile) -> &'static str {
    match validate(p, Utc::now()) {
        Ok(()) => "",
        Err(err) => err.code,
    }
}

fn host_code(result: Result<(), Error>) -> &'static str {
    result.err().map_or("", |err| err.code)
}

fn hosts(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

/// Go: `TestIngressFailoverShapes`.
#[test]
fn ingress_failover_shapes() {
    let mut p = profile("shadowsocks");
    p.nodes[0].ingresses.extend([
        backup("vless", 1, "backup.example.com", ""),
        backup("anytls", 5, "backup2.example.com", "2606:4700::1111"),
    ]);
    assert_eq!(code(&p), "", "a primary and two backups");

    type Mutate = Box<dyn Fn(&mut Profile)>;
    let label = |value: &'static str| -> Mutate {
        Box::new(move |p: &mut Profile| p.nodes[0].ingresses[0].label = Some(value.into()))
    };
    let cases: Vec<(&str, Mutate)> = vec![
        ("FIELD_REQUIRED", Box::new(|p| p.nodes[0].ingresses.clear())),
        (
            "INGRESS_ROLE_INVALID",
            Box::new(|p| p.nodes[0].ingresses[0].role = "backup".into()),
        ),
        (
            "INGRESS_ROLE_INVALID",
            Box::new(|p| p.nodes[0].ingresses[0].role = "standby".into()),
        ),
        (
            "INGRESS_ROLE_INVALID",
            Box::new(|p| {
                let mut second = ingress("shadowsocks", "primary", "second.example.com", "");
                second.replica_ordinal = 1;
                p.nodes[0].ingresses.push(second);
            }),
        ),
        (
            "ENTRY_KEY_INVALID",
            Box::new(|p| p.nodes[0].entry_key = String::new()),
        ),
        (
            "ENTRY_KEY_INVALID",
            Box::new(|p| p.nodes[0].entry_key = "-cn".into()),
        ),
        (
            "ENTRY_KEY_INVALID",
            Box::new(|p| p.nodes[0].entry_key = "a".repeat(65)),
        ),
        (
            "ENDPOINT_KEY_INVALID",
            Box::new(|p| p.nodes[0].ingresses[0].endpoint_key = String::new()),
        ),
        (
            "ENDPOINT_KEY_INVALID",
            Box::new(|p| p.nodes[0].ingresses[0].endpoint_key = "a/b".into()),
        ),
        (
            "ENDPOINT_KEY_INVALID",
            Box::new(|p| p.nodes[0].ingresses[0].endpoint_key = "a".repeat(129)),
        ),
        (
            "ENDPOINT_KEY_DUPLICATE",
            Box::new(|p| {
                let mut b = backup("shadowsocks", 1, "backup.example.com", "");
                b.endpoint_key = p.nodes[0].ingresses[0].endpoint_key.clone();
                p.nodes[0].ingresses.push(b);
            }),
        ),
        (
            // Unique across the whole profile, not just within a node.
            "ENDPOINT_KEY_DUPLICATE",
            Box::new(|p| {
                let mut other = p.nodes[0].clone();
                other.id = "node-2".into();
                p.nodes.push(other);
            }),
        ),
        ("INGRESS_LABEL_INVALID", label("")),
        ("INGRESS_LABEL_INVALID", label("   ")),
        ("INGRESS_LABEL_INVALID", label(" Tokyo")),
        ("INGRESS_LABEL_INVALID", label("Tokyo\n2")),
        ("INGRESS_LABEL_INVALID", label("a\u{0}b")),
        (
            "INGRESS_LABEL_INVALID",
            Box::new(|p| p.nodes[0].ingresses[0].label = Some("a".repeat(33))),
        ),
        // Go's "\xff" label: a Rust string cannot hold invalid UTF-8; see
        // invalid_utf8_in_the_profile_bytes below.
        (
            "REPLICA_ORDINAL_INVALID",
            Box::new(|p| p.nodes[0].ingresses[0].replica_ordinal = -1),
        ),
        (
            "REPLICA_ORDINAL_INVALID",
            Box::new(|p| {
                p.nodes[0]
                    .ingresses
                    .push(backup("shadowsocks", 0, "backup.example.com", ""))
            }),
        ),
        (
            "REPLICA_ORDINAL_INVALID",
            Box::new(|p| {
                p.nodes[0].ingresses[0].replica_ordinal = 3;
                p.nodes[0]
                    .ingresses
                    .push(backup("shadowsocks", 2, "backup.example.com", ""));
            }),
        ),
        (
            "ENTRY_IP_NOT_PUBLIC",
            Box::new(|p| p.nodes[0].ingresses[0].endpoint.ip = "not-an-ip".into()),
        ),
        (
            "CAPABILITIES_INVALID",
            Box::new(|p| p.nodes[0].ingresses[0].capabilities.udp = false),
        ),
        (
            "EXIT_IP_INVALID",
            Box::new(|p| p.nodes[0].exit.ip = "999.1.1.1".into()),
        ),
        (
            "REALITY_PUBLIC_KEY_INVALID",
            Box::new(|p| {
                let mut b = backup("vless", 1, "backup.example.com", "");
                b.tls.as_mut().unwrap().reality.as_mut().unwrap().public_key = "not-a-key".into();
                p.nodes[0].ingresses.push(b);
            }),
        ),
        (
            "INGRESS_COUNT_INVALID",
            Box::new(|p| {
                p.nodes[0].ingresses = vec![Ingress::default(); MAX_INGRESSES_PER_NODE + 1]
            }),
        ),
    ];
    for (i, (want, mutate)) in cases.iter().enumerate() {
        let mut p = profile("shadowsocks");
        mutate(&mut p);
        assert_eq!(code(&p), *want, "case {i}");
    }

    // A backup's own credentials are validated with the same rules.
    let mut p = profile("shadowsocks");
    let mut b = backup("anytls", 1, "backup.example.com", "");
    b.tls.as_mut().unwrap().server_name = "other.example.com".into();
    p.nodes[0].ingresses.push(b);
    let err = validate(&p, Utc::now()).unwrap_err();
    assert_eq!(
        (err.code, err.field.as_deref()),
        (
            "TLS_SERVER_NAME_MISMATCH",
            Some("nodes[0].ingresses[1].tls.server_name")
        )
    );
}

/// Go's encoding/json reads invalid UTF-8 in a string as U+FFFD, so the
/// Go core accepted such a profile (a label of "\u{fffd}" is valid); the
/// Rust decoder rejects the bytes as malformed. Recorded in rust-parity.md.
#[test]
fn invalid_utf8_in_the_profile_bytes() {
    let fixture = include_bytes!("../../../../testdata/profiles/multi-ingress.json");
    let label = b"\"Tokyo relay\"";
    let at = fixture
        .windows(label.len())
        .position(|w| w == label)
        .expect("the fixture has the label");
    let mut bytes = fixture.to_vec();
    // "Tokyo\xffrelay": the space becomes a byte that is not UTF-8.
    bytes[at + "\"Tokyo".len()] = 0xff;
    assert_eq!(parse(&bytes).unwrap_err().code, "PROFILE_MALFORMED");
}

/// Go: `TestParseIgnoresUnknownFields`.
#[test]
fn parse_ignores_unknown_fields() {
    let fixture = include_bytes!("../../../../testdata/profiles/multi-ingress.json");
    let mut doc: Value = serde_json::from_slice(fixture).unwrap();
    doc["future_top_level"] = json!({"x": 1});
    let node = &mut doc["nodes"][0];
    node["future_node_field"] = json!("x");
    if !node["exit"].is_object() {
        node["exit"] = json!({});
    }
    node["exit"]["country_code"] = json!("CN");
    node["exit"]["future_exit_field"] = json!(true);
    node["ingresses"][0]["future_ingress_field"] = json!([1]);
    let p = parse(doc.to_string().as_bytes()).expect("unknown fields must be ignored");
    assert_eq!(p.nodes[0].exit.country_code, "CN");
    assert_eq!(code(&p), "", "a profile with unknown fields must validate");
}

/// Go: `TestRealityServerNameIsBorrowed`.
#[test]
fn reality_server_name_is_borrowed() {
    let mut p = profile("vless");
    let server_name = |p: &mut Profile, name: &str| {
        p.nodes[0].ingresses[0].tls.as_mut().unwrap().server_name = name.into();
    };
    // Not the ingress's own domain: REALITY borrows a third-party site's.
    server_name(&mut p, "cloudflare-dns.com");
    assert_eq!(code(&p), "", "a borrowed REALITY SNI");
    server_name(&mut p, "not a domain");
    assert_eq!(code(&p), "TLS_SERVER_NAME_INVALID");
}

/// Go: `TestRuleSetHostPinning`.
#[test]
fn rule_set_host_pinning() {
    let mut p = rule_set_profile();
    for allowed in [
        &["api.example.com"][..],
        &["API.example.com:443"],
        &["other.example", "api.example.com"],
    ] {
        assert_eq!(
            host_code(validate_rule_set_hosts(&p, &hosts(allowed))),
            "",
            "{allowed:?}"
        );
    }
    for allowed in [
        &[][..],
        &["example.com"],
        &["api.example.com:8443"],
        &["evil.api.example.com"],
    ] {
        assert_eq!(
            host_code(validate_rule_set_hosts(&p, &hosts(allowed))),
            "RULE_SET_HOST_NOT_ALLOWED",
            "{allowed:?}"
        );
    }
    assert_eq!(
        host_code(validate_rule_set_hosts(
            &p,
            &hosts(&["https://api.example.com/"])
        )),
        "RULE_SET_HOSTS_INVALID",
        "a malformed allowed host"
    );
    p.routing.rule_sets[0].url = "https://api.example.com:8443/x.srs".into();
    assert_eq!(
        host_code(validate_rule_set_hosts(
            &p,
            &hosts(&["api.example.com:8443"])
        )),
        "",
        "an explicit port"
    );
    p.routing.rule_sets[0].url = "https://[2001:DB8::1]/x.srs".into();
    assert_eq!(
        host_code(validate_rule_set_hosts(&p, &hosts(&["[2001:db8::1]:443"]))),
        "",
        "IPv6"
    );
}

/// Go: `TestRoutingValidationIDNAPortsCIDRAndStrictJSON`.
#[test]
fn routing_validation_idna_ports_cidr_and_unknown_fields() {
    let complete = RoutingRule {
        id: "complete".into(),
        matcher: RoutingMatch {
            domains: vec!["例子.测试.".into()],
            domain_suffixes: vec!["Example.COM".into()],
            ip_cidrs: vec!["2001:4860::/32".into(), "1.1.1.0/24".into()],
            protocols: vec!["tcp".into(), "udp".into()],
            ports: vec![53, 443],
            port_ranges: vec!["8000-9000".into()],
            ..RoutingMatch::default()
        },
        action: RoutingAction {
            kind: "proxy".into(),
            target: "selected".into(),
            ..RoutingAction::default()
        },
        ..RoutingRule::default()
    };
    let mut p = profile("shadowsocks");
    p.routing.rules = vec![complete.clone()];
    assert_eq!(code(&p), "");
    assert_eq!(
        normalize_domain("例子.测试.").as_deref(),
        Some("xn--fsqu00a.xn--0zwm56d")
    );

    type Mutate = fn(&mut RoutingMatch);
    let invalid: [(&str, Mutate); 5] = [
        ("wildcard suffix", |m| {
            m.domain_suffixes = vec!["*.example.com".into()]
        }),
        ("CIDR", |m| m.ip_cidrs = vec!["not-cidr".into()]),
        ("protocol", |m| m.protocols = vec!["icmp".into()]),
        ("port 0", |m| m.ports = vec![0]),
        ("reversed range", |m| {
            m.port_ranges = vec!["9000-8000".into()]
        }),
    ];
    for (what, mutate) in invalid {
        let mut bad = profile("shadowsocks");
        let mut rule = complete.clone();
        mutate(&mut rule.matcher);
        bad.routing.rules = vec![rule];
        assert_ne!(code(&bad), "", "an invalid rule was accepted: {what}");
    }

    let unknown = br#"{
        "schema_version":1,
        "revision":"r",
        "nodes":[],
        "selection":{"mode":"manual","default_node_id":"n"},
        "routing":{"rules":[],"final":{"type":"direct"},"unknown":true}
    }"#;
    parse(unknown).expect("an unknown routing field must be ignored");
}
