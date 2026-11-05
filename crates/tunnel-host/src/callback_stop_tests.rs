//! A regression test against the host, not just its callback gate.

use crate::host::{self, ExitCallback, Options};
use ppvpn_core::Platform;
use std::ffi::{c_char, c_void};
use std::os::fd::AsRawFd;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

struct Probe {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    returned: mpsc::Sender<()>,
}

extern "C" fn on_exit(context: *mut c_void, _: *const c_char) {
    // Keep the probe alive even on the broken implementation: the test
    // demonstrates the early return without itself freeing live memory.
    let probe = unsafe {
        Arc::increment_strong_count(context as *const Probe);
        Arc::from_raw(context as *const Probe)
    };
    let _ = probe.entered.send(());
    let _ = probe.release.lock().unwrap().recv();
    let _ = probe.returned.send(());
}

#[test]
fn stop_does_not_return_while_the_native_exit_callback_is_running() {
    let _serial = host::test_lock();
    let dir = std::env::temp_dir().join(format!("ppvpn-callback-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tun = std::fs::File::open("/dev/null").unwrap();
    let (entered, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (returned, callback_done) = mpsc::channel();
    let probe = Arc::new(Probe {
        entered,
        release: Mutex::new(wait),
        returned,
    });
    let exit = ExitCallback::new(on_exit, Arc::as_ptr(&probe).cast_mut().cast());
    host::start(
        tun.as_raw_fd(),
        &dir.join("tunnel.sock"),
        &dir.join("state"),
        Platform::Ios,
        Options::default(),
        Some(exit.clone()),
    )
    .unwrap();
    exit.notify("TEST_EXIT", "test");
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let (stopping, started) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let stopper = std::thread::spawn(move || {
        stopping.send(()).unwrap();
        host::stop();
        done.send(()).unwrap();
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let early = finished.recv_timeout(Duration::from_millis(100)).is_ok();
    release.send(()).unwrap();
    callback_done.recv_timeout(Duration::from_secs(5)).unwrap();
    stopper.join().unwrap();
    host::stop();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        !early,
        "stop returned while native code still used its context"
    );
}
