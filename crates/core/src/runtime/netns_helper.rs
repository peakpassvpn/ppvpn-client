//! Processes the netns tests start from this test binary: a TCP echo in the
//! uplink namespace (`ip netns exec ppvpn-w <this test binary> --exact
//! runtime::netns_helper::echo --ignored`, the address in PPVPN_NETNS_ECHO),
//! and an Engine to kill (`runtime::netns_helper::engine`). Outside that
//! each returns at once. Kept apart from runtime::netns_tests so that their
//! filter does not run them.

use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
#[ignore = "started by runtime::netns_tests in the uplink namespace"]
fn echo() {
    let Ok(addr) = std::env::var("PPVPN_NETNS_ECHO") else {
        return;
    };
    let listener = TcpListener::bind(&addr).expect("echo: bind");
    println!("echo listening on {addr}");
    for conn in listener.incoming() {
        let Ok(mut conn) = conn else { continue };
        std::thread::spawn(move || {
            let mut buf = [0u8; 64 << 10];
            while let Ok(n) = conn.read(&mut buf) {
                if n == 0 || conn.write_all(&buf[..n]).is_err() {
                    return;
                }
            }
        });
    }
}

/// An Engine with a TUN, started on the contract golden's profile with its
/// state in PPVPN_NETNS_ENGINE_DIR, that says "engine running" and waits to
/// be killed (runtime::netns_tests' kill -9 test starts it as a child).
/// Outside that it returns at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "started by runtime::netns_tests as the process it kills"]
async fn engine() {
    use crate::config::{EngineConfig, Platform, Role};
    use crate::engine::Engine;
    use crate::request::ApplyRequest;

    let Ok(dir) = std::env::var("PPVPN_NETNS_ENGINE_DIR") else {
        return;
    };
    let engine = Engine::new(EngineConfig::new(Role::Tun, Platform::Linux, &dir))
        .await
        .expect("engine: new");
    let profile = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/golden/contract/profiles/base.json"
    ))
    .expect("engine: profile");
    engine
        .apply(ApplyRequest::new(profile))
        .await
        .expect("engine: apply");
    engine.start().await.expect("engine: start");
    println!("engine running");
    std::future::pending::<()>().await;
}
