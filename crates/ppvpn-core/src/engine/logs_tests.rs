//! The instance's log on the fake runtime: core and sail lines to the sink,
//! drops counted and told of, which instance a line is for, and no secret
//! in any line.
//!
//! Other tests in this binary may log through the global subscriber, and a
//! line of no instance goes to every instance: the checks here look for
//! their own lines and count at least, never exactly.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures_util::FutureExt;

use super::*;
use crate::config::{EngineConfig, LocalProxyConfig, Platform, Role};
use crate::engine::lifecycle_tests::{profile_with, NODE_2, R1};
use crate::engine::Engine;
use crate::localproxy::{LocalProxyState, STATE_FILE};
use crate::request::ApplyRequest;
use crate::runtime::fake::FakeRuntime;

const WAIT: Duration = Duration::from_secs(5);

/// Our layer as this thread's subscriber (the tests' runtime is this
/// thread), whatever the process's global one is.
fn subscribe() -> tracing::subscriber::DefaultGuard {
    tracing::subscriber::set_default(tracing_subscriber::registry().with(tracing_layer()))
}

fn engine(level: LogLevel, sink: LogSink) -> (Engine, Arc<FakeRuntime>) {
    let fake = Arc::new(FakeRuntime::default());
    let config = EngineConfig::new(Role::Standard, Platform::Linux, "/nonexistent")
        .with_log(LogConfig::new(level, sink));
    (Engine::with_runtime(config, fake.clone()), fake)
}

/// The lines already queued.
fn drain(rx: &mut LogReceiver) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(Some(line)) = rx.receiver.recv().now_or_never() {
        lines.push(line);
    }
    lines
}

/// Lines until one matches `want`; all of them, that one last.
async fn until(rx: &mut LogReceiver, want: impl Fn(&str) -> bool) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        let line = tokio::time::timeout(WAIT, rx.recv())
            .await
            .expect("the line in time")
            .expect("the channel open");
        let done = want(&line);
        lines.push(line);
        if done {
            return lines;
        }
    }
}

/// `n` random bytes, hex: tests never hard-code a secret.
fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).unwrap();
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn a_channel_gets_core_and_sail_lines() {
    let _subscriber = subscribe();
    let (engine, fake) = engine(LogLevel::Info, LogSink::Channel);
    let mut rx = engine.logs();

    engine
        .inner
        .log
        .span()
        .in_scope(|| tracing::warn!(node = "jp 1", count = 3u32, "core-channel-line"));
    let core = until(&mut rx, |l| l.contains("core-channel-line")).await;
    let core = core.last().unwrap();
    assert!(
        core.contains(" level=warn msg=core-channel-line node=\"jp 1\" count=3 source=core"),
        "{core}"
    );
    assert!(!core.contains("instance="), "{core}");

    fake.log("2026-10-03T00:00:00Z level=info msg=from-sail source=sail");
    until(&mut rx, |l| l.contains("msg=from-sail source=sail")).await;
    assert_eq!(engine.status().dropped_log_lines, 0);

    // Taken once: a second receiver is closed.
    assert_eq!(engine.logs().recv().await, None);
}

#[tokio::test]
async fn other_sinks_have_no_channel_and_none_keeps_nothing() {
    let _subscriber = subscribe();
    let (engine, fake) = engine(LogLevel::Debug, LogSink::None);
    assert_eq!(engine.logs().recv().await, None);
    fake.log("dropped by nobody");
    engine
        .inner
        .log
        .span()
        .in_scope(|| tracing::info!("core-none-line"));
    assert_eq!(engine.status().dropped_log_lines, 0);
}

#[tokio::test]
async fn levels_and_instances_decide_who_gets_a_line() {
    let _subscriber = subscribe();
    let (info, _f1) = engine(LogLevel::Info, LogSink::Channel);
    let (debug, _f2) = engine(LogLevel::Debug, LogSink::Channel);
    let (mut info_rx, mut debug_rx) = (info.logs(), debug.logs());

    info.inner
        .log
        .span()
        .in_scope(|| tracing::info!("for-info-only"));
    debug
        .inner
        .log
        .span()
        .in_scope(|| tracing::debug!("for-debug-only"));
    info.inner
        .log
        .span()
        .in_scope(|| tracing::debug!("too-verbose-for-info"));
    tracing::info!("for-every-instance");

    let to_info = drain(&mut info_rx);
    let to_debug = drain(&mut debug_rx);
    let has =
        |lines: &[String], msg: &str| lines.iter().any(|l| l.contains(&format!(" msg={msg} ")));
    assert!(has(&to_info, "for-info-only"));
    assert!(!has(&to_debug, "for-info-only"));
    assert!(has(&to_debug, "for-debug-only"));
    assert!(!has(&to_info, "for-debug-only"));
    assert!(!has(&to_info, "too-verbose-for-info"));
    assert!(!has(&to_debug, "too-verbose-for-info"));
    assert!(has(&to_info, "for-every-instance"));
    assert!(has(&to_debug, "for-every-instance"));
}

#[tokio::test]
async fn a_full_channel_drops_counts_and_tells() {
    let _subscriber = subscribe();
    let (engine, fake) = engine(LogLevel::Info, LogSink::Channel);
    let mut rx = engine.logs();
    let span = engine.inner.log.span();
    // A line is queued or counted as it is logged; none is waited for.
    let extra = 10;
    for i in 0..QUEUE + extra {
        span.in_scope(|| tracing::info!(i, "core-flood"));
    }
    let dropped = engine.status().dropped_log_lines;
    assert!(dropped >= extra as u64, "{dropped}");
    let queued = drain(&mut rx);
    assert!(queued.len() <= QUEUE, "{}", queued.len());
    assert!(queued.iter().any(|l| l.contains("msg=core-flood i=0 ")));

    // The next line that fits comes after one that says how many went.
    span.in_scope(|| tracing::info!("after-the-drops"));
    let lines = until(&mut rx, |l| l.contains("msg=after-the-drops ")).await;
    let summary = lines
        .iter()
        .find(|l| l.contains(" level=warn msg=\"log lines dropped\" dropped="))
        .expect("the summary line");
    let told: u64 = summary
        .split(" dropped=")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .expect("a count");
    assert!(told >= dropped, "{summary}: {dropped} dropped");
    assert!(summary.ends_with(" source=core"), "{summary}");

    // sail's lines go the same way.
    fake.log("sail-after-the-drops");
    until(&mut rx, |l| l == "sail-after-the-drops").await;
}

#[tokio::test]
async fn a_file_gets_both_appended() {
    let _subscriber = subscribe();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("core.log");
    std::fs::write(&path, "kept\n").unwrap();
    let (engine, fake) = engine(LogLevel::Info, LogSink::File { path: path.clone() });
    assert_eq!(engine.logs().recv().await, None);
    fake.log("2026-10-03T00:00:00Z level=info msg=sail-to-file source=sail");
    engine
        .inner
        .log
        .span()
        .in_scope(|| tracing::info!("core-to-file"));
    let read = |path: &Path| std::fs::read_to_string(path).unwrap_or_default();
    tokio::time::timeout(WAIT, async {
        loop {
            let text = read(&path);
            if text.contains("msg=sail-to-file") && text.contains("msg=core-to-file") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both lines written");
    let text = read(&path);
    assert!(text.starts_with("kept\n"), "appended");
    assert!(text.lines().all(|l| l == "kept" || l.contains(" level=")));
}

#[test]
fn a_file_that_cannot_be_opened_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = LogConfig::new(
        LogLevel::Info,
        LogSink::File {
            path: dir.path().join("missing").join("core.log"),
        },
    );
    let err = Logs::new(&config).err().expect("an error");
    assert_eq!(err.code, codes::CORE_OPERATION_FAILED);
}

/// Credentials never reach a line (section 10), at debug: the local proxy
/// state's password, read from a broken file, and a profile's node secrets
/// through apply, start, a switch and stop.
#[tokio::test]
async fn no_line_holds_a_secret() {
    let _subscriber = subscribe();
    let (engine, _fake) = engine(LogLevel::Debug, LogSink::Channel);
    let mut rx = engine.logs();
    let password = random_hex(16);
    let number: u64 = u64::from_str_radix(&random_hex(7), 16).unwrap() | (1 << 60);
    let user_key = {
        let mut key = [0u8; 16];
        getrandom::fill(&mut key).unwrap();
        base64::engine::general_purpose::STANDARD.encode(key)
    };
    let secrets = [password.clone(), number.to_string(), user_key.clone()];

    // The local proxy state, broken around its password.
    let states = [
        format!(r#"{{"version":2,"prefix":"abcde","password":"{password}","port":"x"}}"#),
        format!(r#"{{"version":2,"prefix":"abcde","password":{number}}}"#),
        format!(r#"{{"version":"{password}"}}"#),
        format!(r#"{{"version":2,"prefix":"ABCDE","password":"{password}"}}"#),
        format!(r#"{{"version":2,"prefix":"abcde","password":"{password}""#),
    ];
    for content in &states {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(STATE_FILE), content).unwrap();
        LocalProxyState::open(dir.path(), &LocalProxyConfig::new().with_preferred_port(0))
            .expect("rebuilt");
    }

    // A profile carrying them, applied and run.
    let with_secrets = |revision: &str| {
        profile_with(revision, |v| {
            v["nodes"][0]["ingresses"][1]["credentials"]["shadowsocks"]["user_key"] =
                user_key.as_str().into();
            v["nodes"][1]["ingresses"][0]["credentials"]["anytls"]["password"] =
                password.as_str().into();
        })
    };
    engine
        .apply(ApplyRequest::new(with_secrets(R1)))
        .await
        .unwrap();
    engine.start().await.unwrap();
    engine.select_node(NODE_2).await.unwrap();
    let broken = profile_with("2026-09-29T00:00:00Z#9", |v| {
        v["nodes"][0]["ingresses"][1]["credentials"]["shadowsocks"]["user_key"] =
            format!("{password}!").into();
    });
    assert!(engine.apply(ApplyRequest::new(broken)).await.is_err());
    engine.stop().await.unwrap();
    tracing::info!("secrets-done");

    let lines = until(&mut rx, |l| l.contains("msg=secrets-done")).await;
    let rebuilt = lines
        .iter()
        .filter(|l| l.contains("msg=\"local proxy: state unusable, rebuilt\""))
        .count();
    assert!(rebuilt >= states.len(), "the broken states were logged");
    for line in &lines {
        for secret in &secrets {
            assert!(!line.contains(secret.as_str()), "a log line holds a secret");
        }
    }
}
