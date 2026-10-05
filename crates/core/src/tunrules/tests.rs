//! The model against Go's tunrules_test.go, and against the rules sail's
//! auto_route installs for the desktop TUN.

use super::*;

fn scope() -> Scope {
    Scope {
        interface: "tun0".into(),
        table: 2091,
        rule_start: 9091,
        rule_end: 9101,
    }
}

fn prefix(s: &str) -> Prefix {
    let (addr, len) = s.split_once('/').unwrap();
    Prefix::new(addr.parse().unwrap(), len.parse().unwrap())
}

/// What sing-tun 0.8.9 installed on a Linux host (IPv4 part of `ip rule`
/// from Desktop's test, with strict route); Go's fixture.
fn sing_tun_rules() -> Vec<Rule> {
    vec![
        Rule {
            dst: Some(prefix("10.60.159.88/30")),
            ..Rule::new(Family::V4, 9091, Action::Table(2091))
        },
        Rule {
            suppress_prefixlen: Some(0),
            ..Rule::new(Family::V4, 9092, Action::Table(2091))
        },
        Rule {
            invert: true,
            dport: Some((53, 53)),
            suppress_prefixlen: Some(0),
            ..Rule::new(Family::V4, 9093, Action::Table(TABLE_MAIN))
        },
        Rule {
            iif: Some("tun0".into()),
            ..Rule::new(Family::V4, 9093, Action::Goto(9101))
        },
        Rule {
            invert: true,
            iif: Some("lo".into()),
            ..Rule::new(Family::V4, 9094, Action::Table(2091))
        },
        Rule {
            src: Some(prefix("0.0.0.0/32")),
            iif: Some("lo".into()),
            ..Rule::new(Family::V4, 9094, Action::Table(2091))
        },
        Rule::new(Family::V4, 9101, Action::Nop),
        Rule::new(Family::V6, 9095, Action::Unreachable),
    ]
}

/// What sail's auto_route (sail/src/platform/auto_route.rs, `rules`)
/// installs for the desktop TUN: IPv4 and IPv6 addresses, strict route, no
/// uid or interface selection, table 2091 from 9091; as `ip rule` lists
/// them after the start.
fn sail_rules() -> Vec<Rule> {
    let v4 = |priority, action| Rule::new(Family::V4, priority, action);
    let v6 = |priority, action| Rule::new(Family::V6, priority, action);
    let lo = Some("lo".to_owned());
    let tun = Some("tun0".to_owned());
    vec![
        Rule {
            dst: Some(prefix("10.60.159.88/30")),
            ..v4(9091, Action::Table(2091))
        },
        Rule {
            suppress_prefixlen: Some(0),
            ..v4(9092, Action::Table(2091))
        },
        Rule {
            suppress_prefixlen: Some(0),
            ..v6(9091, Action::Table(2091))
        },
        Rule {
            invert: true,
            dport: Some((53, 53)),
            suppress_prefixlen: Some(0),
            ..v4(9093, Action::Table(TABLE_MAIN))
        },
        Rule {
            invert: true,
            dport: Some((53, 53)),
            suppress_prefixlen: Some(0),
            ..v6(9092, Action::Table(TABLE_MAIN))
        },
        Rule {
            iif: tun.clone(),
            ..v4(9093, Action::Goto(9101))
        },
        Rule {
            invert: true,
            iif: lo.clone(),
            ..v4(9094, Action::Table(2091))
        },
        Rule {
            iif: lo.clone(),
            src: Some(prefix("0.0.0.0/32")),
            ..v4(9094, Action::Table(2091))
        },
        Rule {
            iif: lo.clone(),
            src: Some(prefix("10.60.159.88/30")),
            ..v4(9094, Action::Table(2091))
        },
        Rule {
            iif: lo.clone(),
            src: Some(prefix("fde2:ec40:9312:c7fd::/126")),
            ..v6(9092, Action::Table(2091))
        },
        Rule {
            iif: tun,
            ..v6(9093, Action::Goto(9101))
        },
        Rule {
            iif: lo.clone(),
            src: Some(prefix("::/1")),
            ..v6(9093, Action::Goto(9101))
        },
        Rule {
            iif: lo,
            src: Some(prefix("8000::/1")),
            ..v6(9093, Action::Goto(9101))
        },
        v6(9094, Action::Table(2091)),
        v4(9101, Action::Nop),
        v6(9101, Action::Nop),
    ]
}

/// Go: TestOwnedKeepsEverySingTunRule.
#[test]
fn owned_keeps_every_sing_tun_rule() {
    let mut listed = vec![
        Rule::new(Family::V4, 0, Action::Table(255)),
        Rule::new(Family::V4, 32766, Action::Table(TABLE_MAIN)),
    ];
    listed.extend(sing_tun_rules());
    let (owned, foreign) = scope().owned(&listed);
    assert_eq!(owned, sing_tun_rules());
    assert!(foreign.is_empty(), "{foreign:?}");
}

#[test]
fn owned_keeps_every_sail_auto_route_rule() {
    let (owned, foreign) = scope().owned(&sail_rules());
    assert_eq!(owned, sail_rules());
    assert!(foreign.is_empty(), "{foreign:?}");
}

/// Go: TestOwnedLeavesOtherProgramsRules.
#[test]
fn owned_leaves_other_programs_rules() {
    let others = vec![
        Rule::new(Family::V4, 9095, Action::Table(100)),
        Rule {
            fwmark: Some((1, u32::MAX)),
            ..Rule::new(Family::V4, 9096, Action::Table(TABLE_MAIN))
        },
        Rule::new(Family::V4, 9097, Action::Goto(32766)),
        Rule {
            iif: Some("eth0".into()),
            ..Rule::new(Family::V4, 9098, Action::Nop)
        },
        Rule {
            invert: true,
            dport: Some((53, 53)),
            suppress_prefixlen: Some(0),
            ..Rule::new(Family::V4, 9099, Action::Table(100))
        },
    ];
    let mut listed = sing_tun_rules();
    listed.extend(others.clone());
    let (owned, foreign) = scope().owned(&listed);
    assert_eq!(owned, sing_tun_rules());
    assert_eq!(foreign, others);
}

/// Go: TestMissingCountsDuplicates.
#[test]
fn missing_counts_duplicates() {
    let want = sing_tun_rules();
    // What networkd left in Desktop's test: only the goto it failed to drop.
    let have = vec![want[3].clone()];
    let gone = missing(&want, &have);
    assert_eq!(gone.len(), want.len() - 1, "{gone:?}");
    assert!(
        gone.iter().all(|r| r.to_string() != want[3].to_string()),
        "{} is present but reported missing",
        want[3]
    );

    let mut more = want.clone();
    more.push(want[0].clone());
    assert!(missing(&want, &more).is_empty());

    let mut twice = want.clone();
    twice.push(want[0].clone());
    assert_eq!(
        missing(&twice, &want).len(),
        1,
        "one of two identical rules is missing"
    );
}

/// Go: TestRestoreOrderPutsGotoTargetsFirst.
#[test]
fn restore_order_puts_goto_targets_first() {
    let ordered = restore_order(&sing_tun_rules());
    assert_eq!(
        (ordered[0].action, ordered[0].priority),
        (Action::Nop, 9101),
        "first restored: {}",
        ordered[0]
    );
    assert!(
        ordered.windows(2).all(|w| w[0].priority >= w[1].priority),
        "{ordered:?}"
    );
}

/// Go: TestRuleString.
#[test]
fn rule_string() {
    let rules = sing_tun_rules();
    let cases = [
        (
            rules[2].clone(),
            "9093/v4 not dport 53-53 lookup 254 suppress_prefixlength 0",
        ),
        (rules[3].clone(), "9093/v4 iif tun0 goto 9101"),
        (rules[6].clone(), "9101/v4 nop"),
        (
            Rule {
                uid_range: Some((1000, 1000)),
                ..Rule::new(Family::V4, 9091, Action::Goto(9101))
            },
            "9091/v4 uidrange 1000-1000 goto 9101",
        ),
        (
            Rule {
                fwmark: Some((0x2024, u32::MAX)),
                ..Rule::new(Family::V6, 9096, Action::Table(2091))
            },
            "9096/v6 fwmark 0x2024/0xffffffff lookup 2091",
        ),
        (
            Rule {
                iif: Some("lo".into()),
                src: Some(prefix("fde2:ec40:9312:c7fd::/126")),
                ..Rule::new(Family::V6, 9092, Action::Table(2091))
            },
            "9092/v6 from fde2:ec40:9312:c7fd::/126 iif lo lookup 2091",
        ),
    ];
    for (rule, want) in cases {
        assert_eq!(rule.to_string(), want);
    }
}

/// Go: TestRouteRestoreOrderPutsGatewayCoveringRoutesLast.
#[test]
fn route_restore_order_puts_gateway_covering_routes_last() {
    let route = |dst: &str| Route {
        dst: prefix(dst),
        gateway: Some("fde2:ec40:9312:c7fd::2".parse().unwrap()),
        oif: 2,
        metric: 1024,
    };
    // Table 2091's IPv6 routes as listed (sorted by prefix).
    let listed: Vec<Route> = ["::/1", "8000::/2", "fc00::/7", "fe00::/9", "fec0::/10"]
        .into_iter()
        .map(route)
        .collect();
    let want: Vec<Route> = ["::/1", "8000::/2", "fe00::/9", "fec0::/10", "fc00::/7"]
        .into_iter()
        .map(route)
        .collect();
    assert_eq!(route_restore_order(&listed), want);
}

#[test]
fn names_prefix_routes() {
    let routes = [Route {
        dst: prefix("0.0.0.0/0"),
        gateway: None,
        oif: 7,
        metric: 0,
    }];
    assert_eq!(
        names(&sing_tun_rules()[6..7], &routes),
        ["9101/v4 nop", "route 0.0.0.0/0 dev#7 metric 0"]
    );
}

#[test]
fn the_desktop_scope_is_the_translation_s() {
    let scope = Scope::desktop("ppvpn0");
    assert_eq!(
        (scope.table, scope.rule_start, scope.rule_end),
        (2091, 9091, 9101)
    );
    assert_eq!(scope.interface, "ppvpn0");
}
