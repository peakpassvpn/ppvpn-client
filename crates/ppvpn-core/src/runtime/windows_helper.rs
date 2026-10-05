//! The process runtime::windows_tests kills: a TUN instance on
//! windows_tests' configuration (strict_route, 198.18.0.0/16 alone) that
//! says "instance running" and waits. Without PPVPN_WINDOWS_INSTANCE it
//! returns at once. Kept apart from runtime::windows_tests so that their
//! filter does not run it.

use super::windows_tests::{config, runtime};
use super::Runtime;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "started by runtime::windows_tests as the process it kills"]
async fn instance() {
    if std::env::var_os("PPVPN_WINDOWS_INSTANCE").is_none() {
        return;
    }
    let runtime = runtime("killed");
    runtime.start(&config()).await.expect("instance: start");
    println!("instance running");
    std::future::pending::<()>().await;
}
