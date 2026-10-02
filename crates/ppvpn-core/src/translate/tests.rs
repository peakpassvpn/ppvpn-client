use std::path::PathBuf;

use serde_json::{json, Value};

use super::*;

fn golden(path: &str) -> Profile {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/golden")
        .join(path);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    crate::profile::parse(&data).unwrap()
}

/// The contract golden's profile: VLESS+REALITY and SS2022 (EIH) behind a
/// failover group, AnyTLS alone; private bypass, a rule to a fixed node.
fn contract() -> Profile {
    golden("contract/profiles/base.json")
}

/// The routing golden's profile: SS2022 failover, AnyTLS, rejects, ports,
/// CIDRs, a baseline rule and a rule set.
fn routing() -> Profile {
    golden("routing/profiles/base.json")
}

const NODE_1: &str = "3f2c9a1e-0000-4000-8000-000000000001-128";
const NODE_2: &str = "3f2c9a1e-0000-4000-8000-000000000002-129";

fn local_proxy() -> LocalProxy {
    LocalProxy {
        port: 7890,
        prefix: "abcd1234".into(),
        password: "secret".into(),
    }
}

fn value(t: &Translation) -> Value {
    serde_json::from_str(&t.json).unwrap()
}

fn outbound<'a>(config: &'a Value, tag: &str) -> &'a Value {
    config["outbounds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["tag"] == tag)
        .unwrap_or_else(|| panic!("no outbound {tag}"))
}

/// Every fixture: sail loads it with no error and no warning.
#[test]
fn every_fixture_passes_sail_check() {
    let mut rule_sets = HashMap::new();
    rule_sets.insert(
        "cn-site".to_owned(),
        RuleSetFile {
            path: "/nonexistent/cn-site.srs".into(),
        },
    );
    let fixtures: Vec<(&str, Profile, Options)> = vec![
        ("contract", contract(), Options::default()),
        (
            "contract, local and system proxy",
            contract(),
            Options {
                local_proxy: Some(local_proxy()),
                system_proxy_port: Some(7891),
                log_level: "info".into(),
                ..Options::default()
            },
        ),
        (
            "contract, pinned and selected",
            contract(),
            Options {
                selected_node_id: Some(NODE_2.into()),
                pins: HashMap::from([(NODE_1.to_owned(), "9002".to_owned())]),
                ..Options::default()
            },
        ),
        (
            "routing, rule set unavailable",
            routing(),
            Options::default(),
        ),
        (
            "routing, global",
            routing(),
            Options {
                mode: RoutingMode::Global,
                ..Options::default()
            },
        ),
        (
            "routing, final reject",
            with_final(routing(), "reject"),
            Options::default(),
        ),
        (
            "routing, final direct",
            with_final(routing(), "direct"),
            Options::default(),
        ),
    ];
    for (name, profile, options) in fixtures {
        let translation = translate(&profile, &options).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        check(&translation.json)
            .unwrap_or_else(|e| panic!("{name}: {}\n{}", e.message, translation.json));
    }
    // With its local copy (the file itself is read at start, not checked).
    let translation = translate(
        &routing(),
        &Options {
            rule_sets,
            ..Options::default()
        },
    )
    .unwrap();
    let config = value(&translation);
    assert_eq!(
        config["route"]["rule_set"],
        json!([{"type": "local", "tag": "rule-set-cn-site", "format": "binary", "path": "/nonexistent/cn-site.srs"}])
    );
}

fn with_final(mut profile: Profile, kind: &str) -> Profile {
    profile.routing.final_action = RoutingAction {
        kind: kind.into(),
        target: String::new(),
        node_id: String::new(),
    };
    profile
}

#[test]
fn tags_are_gos() {
    // Go: "node-" + hex(sha256(id)[:8]); member: + "-" + hex(sha256(key)[:4]).
    assert_eq!(
        node_tag("node-a"),
        format!("node-{}", &hex_prefix("node-a", 8))
    );
    assert_eq!(node_tag("node-a").len(), "node-".len() + 16);
    assert_eq!(
        ingress_tag("node-a", "a-1").len(),
        "node-".len() + 16 + 1 + 8
    );
    assert_ne!(ingress_tag("node-a", "a-1"), ingress_tag("node-a", "a-2"));
}

#[test]
fn a_multi_ingress_node_is_a_selector_over_fallback_and_ingresses() {
    let t = translate(&contract(), &Options::default()).unwrap();
    let config = value(&t);
    let node = node_tag(NODE_1);
    let auto = format!("{node}{AUTO_SUFFIX}");
    let members = vec![ingress_tag(NODE_1, "9001"), ingress_tag(NODE_1, "9002")];

    let selector = outbound(&config, &node);
    assert_eq!(selector["type"], "selector");
    assert_eq!(selector["default"], auto.as_str());
    assert_eq!(selector["outbounds"], json!([auto, members[0], members[1]]));

    let fallback = outbound(&config, &auto);
    assert_eq!(fallback["type"], "fallback");
    assert_eq!(fallback["outbounds"], json!(members));
    assert_eq!(fallback["url"], "http://www.gstatic.com/generate_204");

    assert_eq!(t.groups[&node], auto);
    assert_eq!(t.members[&node], members);
    assert_eq!(t.ingress_keys[&members[1]], "9002");
    assert_eq!(t.outbound_nodes[&members[0]], NODE_1);

    // VLESS + REALITY, dialled at the IP, uTLS on.
    let vless = outbound(&config, &members[0]);
    assert_eq!(vless["type"], "vless");
    assert_eq!(vless["server"], "8.8.8.8");
    assert_eq!(vless["flow"], "xtls-rprx-vision");
    assert_eq!(vless["tls"]["server_name"], "tyo-01.edge.example.com");
    assert_eq!(vless["tls"]["reality"]["short_id"], "01");
    assert_eq!(
        vless["tls"]["utls"],
        json!({"enabled": true, "fingerprint": "chrome"})
    );

    // SS2022 EIH: iPSKs then the uPSK; no IP, so the domain.
    let ss = outbound(&config, &members[1]);
    assert_eq!(ss["type"], "shadowsocks");
    assert_eq!(ss["server"], "tyo-01-relay.edge.example.com");
    assert_eq!(
        ss["password"],
        "AQEBAQEBAQEBAQEBAQEBAQ==:AAAAAAAAAAAAAAAAAAAAAA=="
    );

    // A single ingress is the node itself.
    let anytls = outbound(&config, &node_tag(NODE_2));
    assert_eq!(anytls["type"], "anytls");
    assert_eq!(
        anytls["tls"],
        json!({"enabled": true, "server_name": "sjc-01.edge.example.com"})
    );
    assert_eq!(t.ingress_keys[&node_tag(NODE_2)], "9003");

    let selected = outbound(&config, SELECTED_TAG);
    assert_eq!(selected["outbounds"], json!([node, node_tag(NODE_2)]));
    assert_eq!(selected["default"], node.as_str());
}

#[test]
fn a_pin_is_the_node_selectors_default_and_the_host_selection_wins() {
    let options = Options {
        selected_node_id: Some(NODE_2.into()),
        pins: HashMap::from([(NODE_1.to_owned(), "9002".to_owned())]),
        ..Options::default()
    };
    let config = value(&translate(&contract(), &options).unwrap());
    assert_eq!(
        outbound(&config, &node_tag(NODE_1))["default"],
        ingress_tag(NODE_1, "9002").as_str()
    );
    assert_eq!(
        outbound(&config, SELECTED_TAG)["default"],
        node_tag(NODE_2).as_str()
    );

    let unknown = Options {
        selected_node_id: Some("nope".into()),
        ..Options::default()
    };
    assert_eq!(
        translate(&contract(), &unknown).unwrap_err().code,
        codes::CORE_OPERATION_FAILED
    );
}

#[test]
fn profile_rules_in_order_then_final() {
    let config = value(&translate(&routing(), &Options::default()).unwrap());
    let rules = config["route"]["rules"].as_array().unwrap();
    assert_eq!(
        rules.clone(),
        vec![
            json!({"ip_cidr": ["172.16.0.0/12"], "action": "route", "outbound": "direct"}),
            json!({"domain": ["blocked.example"], "domain_suffix": [".blocked.example"], "action": "reject", "method": "default"}),
            json!({"port": [25], "action": "reject", "method": "default"}),
            json!({"domain": ["video.example"], "domain_suffix": [".video.example"], "action": "route", "outbound": node_tag("node-b")}),
            json!({"domain": ["direct.test"], "domain_suffix": [".direct.test"], "action": "route", "outbound": "direct"}),
            json!({"ip_cidr": ["203.0.113.0/24", "2001:db8:1::/48"], "action": "route", "outbound": "direct"}),
            // "cn" is gone: its only matcher is an unavailable rule set.
        ]
    );
    assert_eq!(config["route"]["final"], SELECTED_TAG);
    assert!(config["route"].get("rule_set").is_none());
    assert_eq!(outbound(&config, DIRECT_TAG)["type"], "direct");
}

#[test]
fn private_is_gos_list() {
    let config = value(&translate(&contract(), &Options::default()).unwrap());
    let rules = config["route"]["rules"].as_array().unwrap();
    assert_eq!(rules[0]["ip_cidr"], json!(PRIVATE_PREFIXES));
}

#[test]
fn global_keeps_baseline_rules_and_proxies_the_rest() {
    let options = Options {
        mode: RoutingMode::Global,
        ..Options::default()
    };
    let config = value(&translate(&routing(), &options).unwrap());
    assert_eq!(
        config["route"]["rules"],
        json!([{"ip_cidr": ["172.16.0.0/12"], "action": "route", "outbound": "direct"}])
    );
    assert_eq!(config["route"]["final"], SELECTED_TAG);
}

#[test]
fn final_reject_is_a_catch_all_rule() {
    let config = value(&translate(&with_final(routing(), "reject"), &Options::default()).unwrap());
    let rules = config["route"]["rules"].as_array().unwrap();
    assert_eq!(
        rules.last().unwrap(),
        &json!({"network": ["tcp", "udp"], "action": "reject", "method": "default"})
    );
    assert!(config["route"].get("final").is_none());
}

#[test]
fn local_proxy_users_go_to_their_node_and_strangers_are_rejected() {
    let options = Options {
        local_proxy: Some(local_proxy()),
        system_proxy_port: Some(7891),
        ..Options::default()
    };
    let config = value(&translate(&contract(), &options).unwrap());
    let inbounds = config["inbounds"].as_array().unwrap();
    assert_eq!(
        inbounds[0],
        json!({
            "type": "mixed",
            "tag": "local-proxy",
            "listen": "127.0.0.1",
            "listen_port": 7890,
            "users": [
                {"username": format!("abcd1234-{NODE_1}"), "password": "secret"},
                {"username": format!("abcd1234-{NODE_2}"), "password": "secret"},
                {"username": "abcd1234", "password": "secret"},
            ],
        })
    );
    assert_eq!(
        inbounds[1],
        json!({"type": "mixed", "tag": "system-proxy", "listen": "127.0.0.1", "listen_port": 7891})
    );
    let rules = config["route"]["rules"].as_array().unwrap();
    assert_eq!(
        rules[0],
        json!({"inbound": ["local-proxy"], "auth_user": [format!("abcd1234-{NODE_1}")], "action": "route", "outbound": node_tag(NODE_1)})
    );
    assert_eq!(rules[2]["type"], "logical");
    assert_eq!(
        rules[2]["rules"][1],
        json!({"auth_user": ["abcd1234"], "invert": true})
    );
    // Then the profile's own rules.
    assert_eq!(rules[3]["ip_cidr"], json!(PRIVATE_PREFIXES));
}

#[test]
fn cidrs_are_masked_as_go_does() {
    assert_eq!(masked_prefix("10.1.2.3/8").as_deref(), Some("10.0.0.0/8"));
    assert_eq!(
        masked_prefix("2001:db8:1::5/48").as_deref(),
        Some("2001:db8:1::/48")
    );
    assert_eq!(masked_prefix("0.0.0.0/0").as_deref(), Some("0.0.0.0/0"));
    assert_eq!(masked_prefix("10.0.0.0/33"), None);
    assert_eq!(masked_prefix("10.0.0.0/08"), None);
    assert_eq!(masked_prefix("10.0.0.0"), None);
}
