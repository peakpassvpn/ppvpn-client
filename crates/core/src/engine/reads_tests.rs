use std::time::Duration;

use super::super::lifecycle_tests::{engine, running};
use super::*;
use crate::runtime::RuntimeTraffic;

fn reads(fake: &crate::runtime::fake::FakeRuntime) -> u64 {
    fake.traffic_reads()
}

/// Idle, nothing reads the runtime; a host that reads gets fresh figures
/// each second while it reads, and the refresher stops after it stopped.
#[tokio::test(start_paused = true)]
async fn the_runtime_is_read_while_the_host_reads() {
    let (engine, fake) = engine();
    running(&engine).await;
    let after_start = reads(&fake);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(reads(&fake), after_start, "idle: no reads");

    fake.set_traffic(RuntimeTraffic {
        upload_bytes: 5,
        download_bytes: 6,
    });
    engine.traffic();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(engine.traffic().upload_bytes, 5, "read on demand");
    tokio::time::sleep(READ_IDLE + Duration::from_secs(5)).await;
    let stopped = reads(&fake);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(
        reads(&fake),
        stopped,
        "stopped once the host stopped reading"
    );
}

/// The first read after a start is not empty nor older than it: the start
/// reads the runtime once as it ends (a host's check that compares the
/// traffic before and after a request starts from a measured figure).
#[tokio::test]
async fn the_first_read_after_a_start_is_measured_by_it() {
    let (engine, fake) = engine();
    fake.set_traffic(RuntimeTraffic {
        upload_bytes: 3,
        download_bytes: 4,
    });
    let before = chrono::Utc::now();
    running(&engine).await;
    let traffic = engine.traffic();
    assert_eq!((traffic.upload_bytes, traffic.download_bytes), (3, 4));
    assert!(traffic.measured_at >= before, "{traffic:?}");
}
