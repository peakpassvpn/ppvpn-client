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
    // A failed group names the group alone (sail, transitional).
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
