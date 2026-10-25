//! Rule sets through the Engine, on the fake runtime. Most need no network;
//! the recovery test uses the existing loopback TLS fixture (L2).

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::super::lifecycle_tests::{drain, profile_with, R1};
use crate::config::{EngineConfig, Platform, Role};
use crate::engine::Engine;
use crate::event::{Event, EventKind};
use crate::request::ApplyRequest;
use crate::runtime::fake::FakeRuntime;
use crate::status::{DegradedReason, EngineState};

const SET: &str = "ads";
const HOST: &str = "rules.example";

fn srs() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/rulesets/testdata/domains.srs"
    ))
    .unwrap()
}

/// The base profile with one rule set and a rule naming it.
fn with_set(sha256: &str) -> Vec<u8> {
    profile_with(R1, |p| {
        p["routing"]["rule_sets"] = json!([{
            "id": SET,
            "url": format!("https://{HOST}/{SET}.srs"),
            "sha256": sha256,
            "update_interval_seconds": 86400,
        }]);
        p["routing"]["rules"].as_array_mut().unwrap().push(json!({
            "id": "ads-direct",
            "match": { "rule_set_ids": [SET] },
            "action": { "type": "direct" },
        }));
    })
}

fn instance(dir: &Path) -> (Engine, Arc<FakeRuntime>) {
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::with_runtime(
        EngineConfig::new(Role::Standard, Platform::Linux, dir),
        fake.clone(),
    );
    (engine, fake)
}

fn names_set(fake: &FakeRuntime) -> bool {
    let config: Value = serde_json::from_str(&fake.config().unwrap()).unwrap();
    config["route"]["rule_set"]
        .as_array()
        .is_some_and(|sets| !sets.is_empty())
}

/// A set that cannot be had never fails the apply: it is reported
/// unavailable (`RuleSetChanged`, `status.rule_sets`) and, while running,
/// `Degraded{RuleSetUnavailable}`; its rules skip it.
#[tokio::test]
async fn an_unavailable_rule_set_degrades_and_never_fails_the_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = instance(tmp.path());
    let mut rx = engine.subscribe(&[EventKind::RuleSetChanged]);
    // No allowed hosts: the URL's host is not pinned, nothing is fetched.
    engine
        .apply(ApplyRequest::new(with_set(&"0".repeat(64))))
        .await
        .unwrap();
    match drain(&mut rx).as_slice() {
        [Event::RuleSetChanged {
            rule_set_id,
            message,
            code,
            ..
        }] => {
            assert_eq!(rule_set_id, SET);
            assert_eq!(message, "unavailable");
            assert_eq!(code, crate::rulesets::HOST_NOT_PINNED);
        }
        other => panic!("{other:?}"),
    }
    let status = engine.status();
    assert_eq!(status.rule_sets.len(), 1);
    assert_eq!(status.rule_sets[0].state, "unavailable");
    assert_eq!(status.state, EngineState::Configured);

    engine.start().await.unwrap();
    assert_eq!(
        engine.status().state,
        EngineState::Degraded {
            reasons: vec![DegradedReason::RuleSetUnavailable {
                rule_set_id: SET.into()
            }]
        }
    );
    assert!(!names_set(&fake), "the configuration skips it");
}

/// A cached copy matching the profile's sha256 is used at once, without a
/// download: ready, and the configuration names its local file.
#[tokio::test]
async fn a_cached_rule_set_is_used_without_the_network() {
    let tmp = tempfile::tempdir().unwrap();
    let data = srs();
    let sha256: String = Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let dir = tmp.path().join("rule-sets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{SET}.srs")), &data).unwrap();

    let (engine, fake) = instance(tmp.path());
    engine
        .apply(ApplyRequest::new(with_set(&sha256)).with_allowed_rule_set_hosts(vec![HOST.into()]))
        .await
        .unwrap();
    engine.start().await.unwrap();
    let status = engine.status();
    assert_eq!(status.rule_sets[0].state, "ready");
    assert_eq!(status.state, EngineState::Running);
    assert!(names_set(&fake));
    assert!(fake.config().unwrap().contains(&format!("{sha256}.srs")));
}

/// Preparing a valid new set does not commit it when Sail refuses the reload.
/// A subsequent successful apply switches the object and the restart cache.
#[tokio::test]
async fn a_rejected_reload_keeps_the_active_rule_set_bytes() {
    use crate::runtime::fake::Op;
    use crate::runtime::RuntimeError;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("rule-sets");
    std::fs::create_dir_all(&dir).unwrap();
    let old = srs();
    let digest = |data: &[u8]| -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    std::fs::write(dir.join(format!("{SET}.srs")), &old).unwrap();
    let (engine, fake) = instance(tmp.path());
    engine
        .apply(ApplyRequest::new(with_set(&digest(&old))))
        .await
        .unwrap();
    engine.start().await.unwrap();
    let before = fake.config().unwrap();
    let config: Value = serde_json::from_str(&before).unwrap();
    let active_path = config["route"]["rule_set"][0]["path"].as_str().unwrap();
    let new = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/rulesets/testdata/domains-more.srs"
    ))
    .unwrap();
    // A prepared download's immutable cache object (no external server needed).
    std::fs::write(
        dir.join("objects").join(format!("{}.srs", digest(&new))),
        &new,
    )
    .unwrap();
    let mut profile: Value = serde_json::from_slice(&with_set(&digest(&new))).unwrap();
    profile["revision"] = super::super::lifecycle_tests::R2.into();
    let profile = serde_json::to_vec(&profile).unwrap();
    fake.fail_next(Op::Reload, RuntimeError::new("config", "refused"));
    engine
        .apply(ApplyRequest::new(profile.clone()))
        .await
        .unwrap_err();
    assert_eq!(fake.config().unwrap(), before);
    assert_eq!(std::fs::read(active_path).unwrap(), old);
    assert_eq!(std::fs::read(dir.join(format!("{SET}.srs"))).unwrap(), old);
    assert_eq!(engine.status().revision.as_deref(), Some(R1));

    engine.apply(ApplyRequest::new(profile)).await.unwrap();
    assert!(fake
        .config()
        .unwrap()
        .contains(&format!("{}.srs", digest(&new))));
    assert_eq!(std::fs::read(dir.join(format!("{SET}.srs"))).unwrap(), new);
}

/// L2: the existing loopback TLS fixture, with a fake runtime. A recovered
/// download's first reload is refused; no further apply is needed to use it.
#[tokio::test]
async fn a_recovered_rule_set_retries_a_refused_reload_until_activated() {
    use std::time::Duration;

    use crate::engine::lifecycle_tests::next_event;
    use crate::rulesets::tests::{tls, RuleSetServer};
    use crate::runtime::fake::{Call, Op};
    use crate::runtime::RuntimeError;

    let old = srs();
    let new = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/rulesets/testdata/domains-more.srs"
    ))
    .unwrap();
    let digest: String = Sha256::digest(&new)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let server = RuleSetServer::start(new.clone()).await;
    server.set(Vec::new(), true);
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("rule-sets");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(format!("{SET}.srs")), &old).unwrap();
    let fake = Arc::new(FakeRuntime::default());
    let engine = Engine::assemble_with_rule_sets(
        EngineConfig::new(Role::Standard, Platform::Linux, tmp.path()),
        fake.clone(),
        crate::engine::Logs::discard(),
        crate::rulesets::Options {
            trust_pem: Some(tls().0.clone()),
            retry_min: Duration::from_millis(100),
            retry_max: Duration::from_secs(1),
            ..crate::rulesets::Options::default()
        },
    )
    .unwrap();
    let mut profile: Value = serde_json::from_slice(&with_set(&digest)).unwrap();
    profile["routing"]["rule_sets"][0]["url"] = format!("https://{}/{SET}.srs", server.host).into();
    engine
        .apply(
            ApplyRequest::new(serde_json::to_vec(&profile).unwrap())
                .with_allowed_rule_set_hosts(vec![server.host.clone()]),
        )
        .await
        .unwrap();
    engine.start().await.unwrap();
    assert_eq!(engine.status().rule_sets[0].state, "stale");

    let mut events = engine.subscribe(&[EventKind::ReloadFailed, EventKind::KernelSwitched]);
    fake.fail_next(Op::Reload, RuntimeError::new("config", "refused once"));
    server.set(new.clone(), false);
    assert!(matches!(
        next_event(&mut events).await,
        Event::ReloadFailed { .. }
    ));
    assert!(matches!(
        next_event(&mut events).await,
        Event::KernelSwitched { .. }
    ));
    // KernelSwitched precedes activation's cache write; wait for the operation.
    drop(engine.inner.op.lock().await);
    assert!(fake.config().unwrap().contains(&format!("{digest}.srs")));
    assert_eq!(std::fs::read(dir.join(format!("{SET}.srs"))).unwrap(), new);
    let reloads = || {
        fake.calls()
            .iter()
            .filter(|call| matches!(call, Call::Reload(_)))
            .count()
    };
    assert_eq!(reloads(), 2, "one refused reload, then automatic recovery");

    // Activation cancels the old generation's retries. No network is due
    // during this interval; advance the timer instead of sleeping.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(reloads(), 2);
}

/// A busy lifecycle operation cannot accumulate a task for every retry.
#[tokio::test]
async fn repeated_rebuild_notifications_queue_only_one_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, _) = instance(tmp.path());
    let op = engine.inner.op.lock().await;
    let owners = Arc::strong_count(&engine.inner);
    for _ in 0..32 {
        engine.inner.queue_rule_set_rebuild();
    }
    // Each queued worker owns Inner, and none can finish while `op` is held.
    assert_eq!(Arc::strong_count(&engine.inner), owners + 1);
    drop(op);
    tokio::task::yield_now().await;
    assert!(!engine.inner.rule_set_inputs().rebuilding);
}

/// A new list of allowed hosts is a new apply, even for the same
/// revision: it may let a set be fetched.
#[tokio::test]
async fn new_allowed_hosts_are_not_deduplicated() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, _fake) = instance(tmp.path());
    let profile = with_set(&"0".repeat(64));
    let first = engine
        .apply(ApplyRequest::new(profile.clone()))
        .await
        .unwrap();
    assert!(first.applied);
    let again = engine
        .apply(ApplyRequest::new(profile.clone()))
        .await
        .unwrap();
    assert!(!again.applied, "same request: deduplicated");
    // The host is pinned now: the apply runs and tries it (a name under
    // .example never resolves).
    let pinned = engine
        .apply(ApplyRequest::new(profile).with_allowed_rule_set_hosts(vec![HOST.into()]))
        .await
        .unwrap();
    assert!(pinned.applied);
    assert_ne!(
        engine.status().rule_sets[0].error,
        crate::rulesets::HOST_NOT_PINNED
    );
}

/// With no host allowed, a cached copy of another version is not used:
/// only the profile's own version would be (contract 4.1).
#[tokio::test]
async fn without_allowed_hosts_an_older_copy_is_not_used() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("rule-sets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{SET}.srs")), srs()).unwrap();
    let (engine, fake) = instance(tmp.path());
    engine
        .apply(ApplyRequest::new(with_set(&"0".repeat(64))))
        .await
        .unwrap();
    engine.start().await.unwrap();
    assert_eq!(engine.status().rule_sets[0].state, "unavailable");
    assert!(!names_set(&fake));
}

/// A rebuild of an expired profile fails with PROFILE_EXPIRED and
/// `ReloadFailed`; the configuration in use stays.
#[tokio::test]
async fn an_expired_profile_is_not_rebuilt() {
    let tmp = tempfile::tempdir().unwrap();
    let (engine, fake) = instance(tmp.path());
    let expires = chrono::Utc::now() + chrono::Duration::milliseconds(300);
    let profile = profile_with(R1, |p| {
        p["expires_at"] = expires.to_rfc3339().into();
    });
    engine.apply(ApplyRequest::new(profile)).await.unwrap();
    engine.start().await.unwrap();
    let before = fake.config();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let mut rx = engine.subscribe(&[EventKind::ReloadFailed]);
    engine.inner.clone().rebuild_rule_sets().await;
    match drain(&mut rx).as_slice() {
        [Event::ReloadFailed { code, .. }] => {
            assert_eq!(code, crate::error::codes::PROFILE_EXPIRED)
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fake.config(), before);
}
