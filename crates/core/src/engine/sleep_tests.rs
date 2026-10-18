//! sleep and wake (host-integration 4.7) on the fake runtime.

use std::time::Duration;

use super::lifecycle_tests::{engine, running};
use super::sleep::LEASE;
use crate::runtime::fake::Call;

fn sleeps(calls: Vec<Call>) -> Vec<Call> {
    calls
        .into_iter()
        .filter(|c| matches!(c, Call::Sleep | Call::Wake))
        .collect()
}

#[tokio::test]
async fn nothing_is_told_while_not_running() {
    let (engine, fake) = engine();
    engine.sleep().await;
    engine.wake().await;
    assert!(fake.calls().is_empty());
    assert!(!engine.inner.sleep_lease.holds());
}

#[tokio::test]
async fn a_sleep_and_its_wake_go_to_the_runtime() {
    let (engine, fake) = engine();
    running(&engine).await;
    engine.sleep().await;
    assert!(engine.inner.sleep_lease.holds());
    engine.wake().await;
    assert!(!engine.inner.sleep_lease.holds());
    assert!(matches!(
        sleeps(fake.calls()).as_slice(),
        [Call::Sleep, Call::Wake]
    ));
}

#[tokio::test(start_paused = true)]
async fn a_missed_wake_is_done_after_the_lease() {
    let (engine, fake) = engine();
    running(&engine).await;
    engine.sleep().await;
    tokio::time::sleep(LEASE - Duration::from_secs(1)).await;
    assert!(matches!(sleeps(fake.calls()).as_slice(), [Call::Sleep]));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(matches!(
        sleeps(fake.calls()).as_slice(),
        [Call::Sleep, Call::Wake]
    ));
    // The host's late wake finds nothing asleep: told again, harmless.
    engine.wake().await;
    assert!(!engine.inner.sleep_lease.holds());
}

#[tokio::test(start_paused = true)]
async fn a_wake_in_time_ends_the_lease() {
    let (engine, fake) = engine();
    running(&engine).await;
    engine.sleep().await;
    engine.wake().await;
    tokio::time::sleep(LEASE * 2).await;
    assert!(matches!(
        sleeps(fake.calls()).as_slice(),
        [Call::Sleep, Call::Wake]
    ));
}
