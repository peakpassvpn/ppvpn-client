use std::time::{Duration, Instant};

use super::*;

/// Go: internal/outboundlog TestLimiterLogsOncePerWindowWithSuppressedCount.
#[test]
fn limiter_logs_once_per_window_with_suppressed_count() {
    let l = Limiter::default();
    let start = Instant::now();
    assert_eq!(l.allow("a:1", start, 1), Some(0));
    for i in 1..=3 {
        assert_eq!(l.allow("a:1", start + Duration::from_secs(i), 1), None);
    }
    assert_eq!(
        l.allow("b:1", start + Duration::from_secs(1), 1),
        Some(0),
        "another destination is limited separately"
    );
    assert_eq!(l.allow("a:1", start + DIRECT_LIMIT, 1), Some(3));
    assert_eq!(l.allow("a:1", start + 3 * DIRECT_LIMIT, 1), Some(0));
}

/// sail batches failures (`count`): one line, the rest suppressed.
#[test]
fn a_batch_logs_one_line() {
    let l = Limiter::default();
    let start = Instant::now();
    assert_eq!(l.allow("a:1", start, 5), Some(0));
    assert_eq!(l.allow("a:1", start + DIRECT_LIMIT, 1), Some(4));
}

/// Go: internal/outboundlog TestLimiterForgetsOldDestinationsWhenFull.
#[test]
fn limiter_forgets_old_destinations_when_full() {
    let l = Limiter::default();
    let start = Instant::now();
    for i in 0..LIMITER_SIZE {
        l.allow(&format!("{i}:1"), start, 1);
    }
    l.allow("new:1", start + DIRECT_LIMIT, 1);
    assert_eq!(l.remembered(), 1, "only the new one");
}

fn translation() -> Translation {
    let mut t = Translation {
        json: String::new(),
        node_tags: Default::default(),
        outbound_nodes: Default::default(),
        ingress_keys: Default::default(),
        groups: Default::default(),
        members: Default::default(),
        direct_ipv6_hand_off: false,
        rule_ids: Vec::new(),
        dns_members: Default::default(),
        dns_local_listener: false,
        check_interval: Default::default(),
    };
    t.outbound_nodes.insert("node-a".into(), "a".into());
    t.outbound_nodes.insert("node-a-1".into(), "a".into());
    t.ingress_keys.insert("node-a-1".into(), "9001".into());
    t.groups.insert("node-a".into(), "node-a-auto".into());
    t.outbound_nodes.insert("node-b".into(), "b".into());
    t.ingress_keys.insert("node-b".into(), "9003".into());
    t
}

#[test]
fn chains_name_their_node_and_ingress() {
    let t = translation();
    assert_eq!(through(&t, "direct"), Through::Direct);
    assert_eq!(through(&t, "selected>direct"), Through::Direct);
    assert_eq!(
        through(&t, "selected>node-b"),
        Through::Node {
            node_id: "b".into(),
            endpoint_key: Some("9003".into()),
            group: false
        }
    );
    // A chain without the member (before sail 2eb3fe47): the node only.
    assert_eq!(
        through(&t, "node-a>node-a-auto"),
        Through::Node {
            node_id: "a".into(),
            endpoint_key: None,
            group: true
        }
    );
    assert_eq!(
        through(&t, "node-a>node-a-auto>node-a-1"),
        Through::Node {
            node_id: "a".into(),
            endpoint_key: Some("9001".into()),
            group: true
        }
    );
    assert_eq!(through(&t, "block"), Through::Other);
}

/// Full of keys still in their window: a new one is logged, not
/// remembered.
#[test]
fn a_full_limiter_stays_bounded() {
    let l = Limiter::default();
    let start = Instant::now();
    for i in 0..LIMITER_SIZE {
        l.allow(&format!("{i}:1"), start, 1);
    }
    assert_eq!(l.allow("new:1", start + Duration::from_secs(1), 1), Some(0));
    assert_eq!(l.remembered(), LIMITER_SIZE);
}

fn routed(rule: Option<usize>, request: Option<&str>) -> crate::runtime::Routed {
    crate::runtime::Routed {
        id: Some(7),
        network: "udp".into(),
        inbound: "tun".into(),
        source: "172.19.0.1:5000".into(),
        destination: "198.51.100.50:9999".into(),
        domain: None,
        domain_source: None,
        protocol: None,
        rule,
        action: "outbound".into(),
        chain: vec!["direct".into()],
        request_destination: request.map(Into::into),
        target: None,
        error: None,
        connect_ms: Some(1),
    }
}

/// The rule is the profile's id (by its index), `final` without one; the
/// target is what the outbound was asked to reach, and its kind.
#[test]
fn connection_lines_name_the_profile_rule_and_the_target() {
    let mut t = translation();
    t.rule_ids = vec!["tun".into(), "video".into()];
    assert_eq!(
        rule_and_target(Some(&t), &routed(Some(1), Some("video.example:443"))),
        ("video".into(), "video.example:443", "domain")
    );
    assert_eq!(
        rule_and_target(Some(&t), &routed(None, None)),
        ("final".into(), "198.51.100.50:9999", "ip")
    );
    assert_eq!(
        rule_and_target(Some(&t), &routed(Some(0), Some("[2001:db8::1]:443"))),
        ("tun".into(), "[2001:db8::1]:443", "ip")
    );
}

fn exchange(server: Option<&str>, source: &str) -> crate::runtime::DnsExchange {
    crate::runtime::DnsExchange {
        name: "example.com".into(),
        qtype: "A".into(),
        qtype_code: 1,
        server: server.map(Into::into),
        source: source.into(),
        attempt: None,
        rcode: Some(0),
        rcode_name: Some("NOERROR".into()),
        error: None,
        answers: Vec::new(),
        answers_total: 1,
        ttl: Some(60),
        duration_ms: Some(3),
        for_instance: false,
    }
}

/// As Go: only exchanges sent upstream get a line; those of the engine's
/// own dns-local listener are its own lines.
#[test]
fn only_upstream_exchanges_of_sails_servers_are_logged() {
    let mut t = translation();
    t.dns_local_listener = true;
    let t = Some(&t);
    assert!(logs_dns(t, &exchange(Some("dns-remote"), "exchanged")));
    assert!(!logs_dns(t, &exchange(Some("dns-remote"), "cached")));
    assert!(!logs_dns(t, &exchange(Some("dns-remote"), "optimistic")));
    assert!(!logs_dns(t, &exchange(None, "rule")));
    assert!(!logs_dns(
        t,
        &exchange(Some(crate::translate::DNS_LOCAL_TAG), "exchanged")
    ));
    assert_eq!(crate::localdns::rcode_name(3), "NXDOMAIN");
}

/// sail names the member of a sequential server that answered: the line
/// names the server, and the member's resolver as `upstream` (lab
/// dns-hijack.5: `server=dns-remote`).
#[test]
fn a_member_maps_back_to_its_server() {
    let mut t = translation();
    t.dns_members.insert(
        "dns-remote-1.1.1.1".into(),
        ("dns-remote".into(), "1.1.1.1".into()),
    );
    t.dns_members.insert(
        "dns-local-0".into(),
        ("dns-local".into(), "192.0.2.1:53".into()),
    );
    let e = exchange(Some("dns-remote-1.1.1.1"), "exchanged");
    assert_eq!(dns_server(Some(&t), &e), ("dns-remote", Some("1.1.1.1")));
    let e = exchange(Some("dns-remote"), "exchanged");
    assert_eq!(dns_server(Some(&t), &e), ("dns-remote", None));
    // dns-local made of servers (not the engine's listener) is sail's: logged.
    let e = exchange(Some("dns-local-0"), "exchanged");
    assert!(logs_dns(Some(&t), &e));
}
