use std::path::PathBuf;

use serde_json::{json, Value};

use super::*;
use crate::config::Platform;

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
        listen: "127.0.0.1".into(),
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
            mirror_dns: true,
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
        (
            "contract, TUN desktop",
            contract(),
            Options {
                tun: Some(desktop_tun(LocalDns::System)),
                local_proxy: Some(local_proxy()),
                ..Options::default()
            },
        ),
        (
            "contract, TUN desktop without IPv6, host DNS servers",
            contract(),
            Options {
                tun: Some(Tun {
                    ipv6: false,
                    ..desktop_tun(LocalDns::Servers(vec![
                        "192.168.50.1:53".parse().unwrap(),
                        "[2001:db8::53]:5353".parse().unwrap(),
                    ]))
                }),
                ..Options::default()
            },
        ),
        (
            "contract, TUN desktop, dns-local listener, unnamed (macOS)",
            contract(),
            Options {
                tun: Some(Tun {
                    interface_name: interface_name(Platform::Macos).into(),
                    ..desktop_tun(listener())
                }),
                ..Options::default()
            },
        ),
        (
            "contract, TUN desktop, dns-local listener, Windows name",
            contract(),
            Options {
                tun: Some(Tun {
                    interface_name: interface_name(Platform::Windows).into(),
                    ..desktop_tun(listener())
                }),
                ..Options::default()
            },
        ),
        (
            "contract, TUN desktop without a host IPv6 path",
            contract(),
            Options {
                tun: Some(Tun {
                    no_host_ipv6_route: true,
                    ..desktop_tun(LocalDns::System)
                }),
                local_proxy: Some(local_proxy()),
                ..Options::default()
            },
        ),
        (
            "routing, TUN desktop, final direct",
            with_final(routing(), "direct"),
            Options {
                tun: Some(desktop_tun(LocalDns::System)),
                ..Options::default()
            },
        ),
    ];
    // All of them, so one run shows every failure.
    let mut failures = Vec::new();
    for (name, profile, options) in fixtures {
        match translate(&profile, &options) {
            Err(e) => failures.push(format!("{name}: {e:?}")),
            Ok(t) => {
                if let Err(e) = check(&t.json) {
                    failures.push(format!("{name}: {}\n{}", e.message, t.json));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
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
        format!("node-{}", hex_prefix("node-a", 8))
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
            // D4: node-b carries no UDP.
            json!({"domain": ["video.example"], "domain_suffix": [".video.example"], "network": ["udp"], "action": "reject", "method": "default"}),
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
    // D4: NODE_2's user carries no UDP.
    assert_eq!(
        rules[1],
        json!({"inbound": ["local-proxy"], "auth_user": [format!("abcd1234-{NODE_2}")], "network": ["udp"], "action": "reject", "method": "default"})
    );
    assert_eq!(rules[3]["type"], "logical");
    assert_eq!(
        rules[3]["rules"][1],
        json!({"auth_user": ["abcd1234"], "invert": true})
    );
    // Then the profile's own rules.
    assert_eq!(rules[4]["ip_cidr"], json!(PRIVATE_PREFIXES));
}

fn desktop_tun(local_dns: LocalDns) -> Tun {
    Tun {
        desktop: true,
        ipv6: true,
        no_host_ipv6_route: false,
        interface_name: interface_name(Platform::Linux).into(),
        local_dns,
    }
}

fn tun_options() -> Options {
    Options {
        tun: Some(desktop_tun(LocalDns::System)),
        ..Options::default()
    }
}

#[test]
fn tun_rules_lead_then_ingress_bypass_then_profile() {
    let config = value(&translate(&contract(), &tun_options()).unwrap());
    let rules = config["route"]["rules"].as_array().unwrap();
    assert_eq!(rules[0], json!({"inbound": ["tun"], "action": "sniff"}));
    assert_eq!(
        rules[1],
        json!({"inbound": ["tun"], "action": "route-options", "override_destination": true})
    );
    assert_eq!(rules[2]["action"], "hijack-dns");
    assert_eq!(
        rules[3],
        json!({"inbound": ["tun"], "port": [53], "action": "hijack-dns"})
    );
    assert_eq!(
        rules[4]["ip_cidr"],
        json!(["10.60.159.88/30", "fde2:ec40:9312:c7fd::/126"])
    );
    assert_eq!(rules[5]["type"], "logical");
    assert_eq!(rules[6]["ip_cidr"], json!(PRIVATE_PREFIXES));
    assert_eq!(rules[6]["outbound"], "direct");
    // Every ingress, primary and backup, by domain and by IP.
    assert_eq!(
        rules[7..12].to_vec(),
        vec![
            json!({"domain": ["tyo-01.edge.example.com"], "action": "route", "outbound": "direct"}),
            json!({"ip_cidr": ["8.8.8.8"], "action": "route", "outbound": "direct"}),
            json!({"domain": ["tyo-01-relay.edge.example.com"], "action": "route", "outbound": "direct"}),
            json!({"domain": ["sjc-01.edge.example.com"], "action": "route", "outbound": "direct"}),
            json!({"ip_cidr": ["1.1.1.1"], "action": "route", "outbound": "direct"}),
        ]
    );
    // The profile's bypass-private rule follows.
    assert_eq!(rules[12]["ip_cidr"], json!(PRIVATE_PREFIXES));
    assert_eq!(config["route"]["auto_detect_interface"], true);
    assert_eq!(
        config["route"]["default_domain_resolver"],
        json!({"server": "dns-local"})
    );
}

#[test]
fn desktop_tun_routes_ipv6_and_keeps_ingresses_out() {
    let config = value(&translate(&contract(), &tun_options()).unwrap());
    let tun = &config["inbounds"][0];
    assert_eq!(tun["type"], "tun");
    assert_eq!(
        tun["address"],
        json!(["10.60.159.89/30", "fde2:ec40:9312:c7fd::1/126"])
    );
    assert_eq!(tun["auto_route"], true);
    assert_eq!(tun["strict_route"], true);
    assert_eq!(tun["iproute2_table_index"], 2091);
    assert_eq!(tun["iproute2_rule_index"], 9091);
    assert_eq!(
        tun["route_exclude_address"],
        json!([
            "8.8.8.8/32",
            "1.1.1.1/32",
            "224.0.0.0/4",
            "255.255.255.255/32",
            "169.254.0.0/16",
            "fe80::/10",
            "ff00::/8"
        ])
    );
    assert!(tun.get("stack").is_none(), "sail warns on stack");

    let no_ipv6 = Options {
        tun: Some(Tun {
            ipv6: false,
            ..desktop_tun(LocalDns::System)
        }),
        ..Options::default()
    };
    let config = value(&translate(&contract(), &no_ipv6).unwrap());
    let tun = &config["inbounds"][0];
    assert_eq!(tun["address"], json!(["10.60.159.89/30"]));
    assert_eq!(
        tun["route_exclude_address"],
        json!([
            "8.8.8.8/32",
            "1.1.1.1/32",
            "224.0.0.0/4",
            "255.255.255.255/32",
            "169.254.0.0/16"
        ])
    );

    let mobile = Options {
        tun: Some(Tun {
            desktop: false,
            ipv6: false,
            no_host_ipv6_route: true,
            interface_name: String::new(),
            local_dns: LocalDns::System,
        }),
        ..Options::default()
    };
    let config = value(&translate(&contract(), &mobile).unwrap());
    assert_eq!(
        config["inbounds"][0],
        json!({"type": "tun", "tag": "tun", "address": ["10.60.159.89/30"]})
    );
    assert!(config["route"].get("auto_detect_interface").is_none());
}

#[test]
fn dns_mirrors_domain_rules_in_order() {
    let mut options = tun_options();
    options.rule_sets.insert(
        "cn-site".into(),
        RuleSetFile {
            path: "/x/cn-site.srs".into(),
            mirror_dns: true,
        },
    );
    let config = value(&translate(&routing(), &options).unwrap());
    let dns = &config["dns"];
    assert_eq!(dns["final"], "dns-remote");
    assert_eq!(dns["reverse_mapping"], true);
    assert_eq!(dns["timeout"], "10s");
    let local = |d: &str| json!({"domain": [d], "action": "route", "server": "dns-local"});
    assert_eq!(
        dns["rules"].as_array().unwrap().clone(),
        vec![
            // The ingress bypass.
            local("a1.edge.example.com"),
            local("a2.edge.example.com"),
            local("b1.edge.example.com"),
            json!({"domain": ["blocked.example"], "domain_suffix": [".blocked.example"], "action": "reject"}),
            // The D4 reject before it is not mirrored: the name resolves.
            json!({"domain": ["video.example"], "domain_suffix": [".video.example"], "action": "route", "server": "dns-remote"}),
            json!({"domain": ["direct.test"], "domain_suffix": [".direct.test"], "action": "route", "server": "dns-local"}),
            json!({"rule_set": ["rule-set-cn-site"], "action": "route", "server": "dns-local"}),
        ]
    );

    options.rule_sets.get_mut("cn-site").unwrap().mirror_dns = false;
    let config = value(&translate(&routing(), &options).unwrap());
    let rules = config["dns"]["rules"].as_array().unwrap();
    assert!(
        rules.iter().all(|r| r.get("rule_set").is_none()),
        "a set with CIDRs never affects DNS"
    );
}

#[test]
fn dns_servers_local_then_remote_in_order() {
    let options = Options {
        tun: Some(desktop_tun(LocalDns::Servers(vec!["192.168.50.1:53"
            .parse()
            .unwrap()]))),
        ..Options::default()
    };
    let config = value(&translate(&with_final(contract(), "direct"), &options).unwrap());
    let dns = &config["dns"];
    assert_eq!(dns["final"], "dns-local");
    assert_eq!(
        dns["servers"],
        json!([
            {"type": "udp", "tag": "dns-local", "server": "192.168.50.1", "server_port": 53},
            {"type": "tls", "tag": "dns-remote-1.1.1.1", "server": "1.1.1.1", "detour": "selected"},
            {"type": "tls", "tag": "dns-remote-8.8.8.8", "server": "8.8.8.8", "detour": "selected"},
            {"type": "tls", "tag": "dns-remote-9.9.9.9", "server": "9.9.9.9", "detour": "selected"},
            {"type": "sequential", "tag": "dns-remote", "servers": ["dns-remote-1.1.1.1", "dns-remote-8.8.8.8", "dns-remote-9.9.9.9"], "attempt_timeout": "3s", "budget": "8s", "prefer_for": "10m"},
        ])
    );
    let empty = Options {
        tun: Some(desktop_tun(LocalDns::Servers(vec![]))),
        ..Options::default()
    };
    assert!(translate(&contract(), &empty).is_err());
}

#[test]
fn no_tun_no_dns() {
    let config = value(&translate(&contract(), &Options::default()).unwrap());
    assert!(config.get("dns").is_none());
    assert!(config["route"].get("default_domain_resolver").is_none());
}

#[test]
fn d4_rejects_udp_to_a_fixed_udp_less_node() {
    // A final to a fixed node without UDP: UDP is rejected before it.
    let mut profile = routing();
    profile.routing.final_action = RoutingAction {
        kind: "proxy".into(),
        target: "node".into(),
        node_id: "node-b".into(),
    };
    let config = value(&translate(&profile, &Options::default()).unwrap());
    let rules = config["route"]["rules"].as_array().unwrap();
    assert_eq!(
        rules.last().unwrap(),
        &json!({"network": ["udp"], "action": "reject", "method": "default"})
    );
    assert_eq!(config["route"]["final"], node_tag("node-b").as_str());

    // A TCP-only rule to it needs none.
    let mut profile = routing();
    profile.routing.rules[3].matcher.protocols = vec!["tcp".into()];
    let config = value(&translate(&profile, &Options::default()).unwrap());
    let rules = config["route"]["rules"].as_array().unwrap();
    assert!(rules.iter().all(|r| r["network"] != json!(["udp"])));

    // The selected node is the Engine's: nothing here.
    let mut profile = routing();
    profile.selection.default_node_id = "node-b".into();
    profile.routing.rules.clear();
    let config = value(&translate(&profile, &Options::default()).unwrap());
    assert!(config["route"].get("rules").is_none());
}

#[test]
fn local_dns_servers_as_go_reads_them() {
    let read = |v: &[&str]| local_dns_servers(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(
        read(&[
            " 192.168.50.1 ",
            "192.168.50.2:5353",
            "[2001:db8::1]:53",
            "::ffff:192.168.50.3"
        ])
        .unwrap(),
        vec![
            "192.168.50.1:53".parse().unwrap(),
            "192.168.50.2:5353".parse().unwrap(),
            "[2001:db8::1]:53".parse().unwrap(),
            "192.168.50.3:53".parse().unwrap(),
        ]
    );
    // Inside the tunnel, now or before 0.5.7: left out.
    assert_eq!(
        read(&["10.60.159.90", "172.19.0.2", "fde2:ec40:9312:c7fd::2"]).unwrap(),
        Vec::<std::net::SocketAddr>::new()
    );
    for bad in ["dns.example", "0.0.0.0", "192.168.50.1:0", "[::]:53", ""] {
        assert!(read(&[bad]).is_err(), "{bad}");
    }
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

#[test]
fn without_a_host_ipv6_path_direct_hands_global_ipv6_its_domain() {
    let options = Options {
        tun: Some(Tun {
            no_host_ipv6_route: true,
            ..desktop_tun(LocalDns::System)
        }),
        ..Options::default()
    };
    let t = translate(&contract(), &options).unwrap();
    assert!(t.direct_ipv6_hand_off);
    let config = value(&t);
    let rules = config["route"]["rules"].as_array().unwrap();
    // After the TUN's own override, so it goes before for global IPv6.
    assert_eq!(
        rules[1],
        json!({"inbound": ["tun"], "action": "route-options", "override_destination": true})
    );
    assert_eq!(
        rules[2],
        json!({"inbound": ["tun"], "ip_cidr": ["2000::/3"], "action": "route-options", "override_destination": "proxy_and_direct"})
    );
    assert_eq!(
        outbound(&config, DIRECT_TAG),
        &json!({"type": "direct", "tag": "direct", "domain_resolver": {"server": "dns-local", "strategy": "ipv4_only"}})
    );
    // The TUN itself is the same as on a host with an IPv6 path.
    let with_path = value(&translate(&contract(), &tun_options()).unwrap());
    assert_eq!(config["inbounds"], with_path["inbounds"]);

    // It needs the TUN's IPv6: not without it, nor on mobile.
    for tun in [
        Tun {
            ipv6: false,
            no_host_ipv6_route: true,
            ..desktop_tun(LocalDns::System)
        },
        Tun {
            desktop: false,
            ipv6: false,
            no_host_ipv6_route: true,
            interface_name: String::new(),
            local_dns: LocalDns::System,
        },
    ] {
        let options = Options {
            tun: Some(tun),
            ..Options::default()
        };
        let t = translate(&contract(), &options).unwrap();
        assert!(!t.direct_ipv6_hand_off);
        let config = value(&t);
        assert!(outbound(&config, DIRECT_TAG)
            .get("domain_resolver")
            .is_none());
        assert!(config["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["override_destination"] != "proxy_and_direct"));
    }
}

fn listener() -> LocalDns {
    LocalDns::Listener("127.0.0.1:53053".parse().unwrap())
}

#[test]
fn dns_local_listener_is_asked_over_tcp() {
    let options = Options {
        tun: Some(desktop_tun(listener())),
        ..Options::default()
    };
    let config = value(&translate(&contract(), &options).unwrap());
    assert_eq!(
        config["dns"]["servers"][0],
        json!({"type": "tcp", "tag": "dns-local", "server": "127.0.0.1", "server_port": 53053})
    );
    assert_eq!(
        config["route"]["default_domain_resolver"],
        json!({"server": "dns-local"})
    );
}

#[test]
fn the_tun_is_named_per_platform() {
    assert_eq!(interface_name(Platform::Linux), "ppvpn0");
    assert_eq!(interface_name(Platform::Windows), "PPVPN");
    // The kernel numbers a utun; sail reports the one it got.
    assert_eq!(interface_name(Platform::Macos), "");
    assert_eq!(interface_name(Platform::Ios), "");
    let config = value(&translate(&contract(), &tun_options()).unwrap());
    assert_eq!(config["inbounds"][0]["interface_name"], "ppvpn0");
    let macos = Options {
        tun: Some(Tun {
            interface_name: interface_name(Platform::Macos).into(),
            ..desktop_tun(LocalDns::System)
        }),
        ..Options::default()
    };
    let config = value(&translate(&contract(), &macos).unwrap());
    assert!(config["inbounds"][0].get("interface_name").is_none());
}

#[test]
fn local_proxy_listens_where_configured() {
    let options = Options {
        local_proxy: Some(LocalProxy {
            listen: "::1".into(),
            ..local_proxy()
        }),
        ..Options::default()
    };
    let config = value(&translate(&contract(), &options).unwrap());
    assert_eq!(config["inbounds"][0]["listen"], "::1");
    let bad = Options {
        local_proxy: Some(LocalProxy {
            listen: "localhost".into(),
            ..local_proxy()
        }),
        ..Options::default()
    };
    assert!(
        translate(&contract(), &bad).is_err(),
        "listen must be an address"
    );
}

#[test]
fn debug_leaves_credentials_out() {
    // A value no field name or default could contain by chance.
    let secret = format!("redact-{:x}", std::process::id() as u64 * 7919 + 11);
    let local_proxy = LocalProxy {
        listen: "127.0.0.1".into(),
        port: 7890,
        prefix: "p".into(),
        password: secret.clone(),
    };
    let shown = format!("{local_proxy:?}");
    assert!(!shown.contains(&secret), "{shown}");
    assert!(shown.contains("7890"), "{shown}");

    let translation = Translation {
        json: format!(r#"{{"inbounds":[{{"password":"{secret}"}}]}}"#),
        node_tags: Default::default(),
        outbound_nodes: Default::default(),
        ingress_keys: Default::default(),
        groups: Default::default(),
        members: Default::default(),
        direct_ipv6_hand_off: false,
        rule_ids: Vec::new(),
    };
    let shown = format!("{translation:?}");
    assert!(!shown.contains(&secret), "{shown}");
    assert!(shown.contains("bytes"), "{shown}");
}

/// sail names a connection's rule by its index in `route.rules`: each
/// index maps back to the profile rule that made it (its D4 rejection
/// included) or to the engine's own rules.
#[test]
fn every_route_rule_maps_back_to_what_made_it() {
    let t = translate(&routing(), &Options::default()).unwrap();
    let config = value(&t);
    let rules = config["route"]["rules"].as_array().unwrap();
    assert_eq!(t.rule_ids.len(), rules.len());
    let video: Vec<usize> = (0..rules.len())
        .filter(|&i| rules[i]["domain"] == json!(["video.example"]))
        .collect();
    assert_eq!(video.len(), 2, "the rule and its D4 rejection");
    let id = &t.rule_ids[video[0]];
    assert!(video.iter().all(|&i| &t.rule_ids[i] == id));
    assert_ne!(id, "engine");

    let t = translate(&contract(), &tun_options()).unwrap();
    assert_eq!(t.rule_ids[0], "tun", "the TUN's sniff rule");
    assert_eq!(
        t.rule_ids.len(),
        value(&t)["route"]["rules"].as_array().unwrap().len()
    );
}
