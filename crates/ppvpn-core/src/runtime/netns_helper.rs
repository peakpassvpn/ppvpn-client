//! A TCP echo the netns tests start in the uplink namespace (`ip netns exec
//! ppvpn-w <this test binary> --exact runtime::netns_helper::echo
//! --ignored`), with the address in PPVPN_NETNS_ECHO. Outside that it
//! returns at once. Kept apart from runtime::netns_tests so that their
//! filter does not run it.

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
