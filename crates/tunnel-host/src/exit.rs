//! The native callback's lifetime, shared with stop, independently of the
//! runtime: its thread may itself stop that runtime.

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, ThreadId};

#[derive(Clone)]
pub struct ExitCallback(Arc<Callback>);

struct Callback {
    state: Mutex<State>,
    finished: Condvar,
    drain: Option<Box<dyn Fn() + Send + Sync>>,
}

type Handler = Box<dyn Fn(&CStr) + Send + Sync>;

struct State {
    cancelled: bool,
    running: Option<ThreadId>,
    function: Option<Handler>,
}

impl ExitCallback {
    pub fn new(function: extern "C" fn(*mut c_void, *const c_char), context: *mut c_void) -> Self {
        // Store only the address, as before; the C caller owns its lifetime.
        let context = context as usize;
        Self::from_fn(move |reason| function(context as *mut c_void, reason.as_ptr()))
    }

    /// An owned callback, used by JNI to bind a listener to this host
    /// instead of looking up a process-global listener during delivery.
    pub(crate) fn from_fn(function: impl Fn(&CStr) + Send + Sync + 'static) -> Self {
        Self::owned(function, None)
    }

    pub(crate) fn from_fn_and_drain(
        function: impl Fn(&CStr) + Send + Sync + 'static,
        drain: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::owned(function, Some(Box::new(drain)))
    }

    fn owned(
        function: impl Fn(&CStr) + Send + Sync + 'static,
        drain: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self(Arc::new(Callback {
            state: Mutex::new(State {
                cancelled: false,
                running: None,
                function: Some(Box::new(function)),
            }),
            finished: Condvar::new(),
            drain,
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn is_current(&self) -> bool {
        self.lock().running == Some(thread::current().id())
    }

    pub(crate) fn is_running(&self) -> bool {
        self.lock().running.is_some()
    }

    pub(crate) fn call(self, reason: CString) -> std::io::Result<()> {
        self.schedule(reason, |task| {
            thread::Builder::new()
                .name("ppvpn-tunnel-exit".into())
                .spawn(task)
                .map(|_| ())
        })
    }

    fn schedule(
        self,
        reason: CString,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        spawn(Box::new(move || self.invoke(reason)))
    }

    fn invoke(&self, reason: CString) {
        let function = {
            let mut state = self.lock();
            if state.cancelled {
                return;
            }
            // A host exits once, even if two producers try to notify it.
            state.cancelled = true;
            state.running = Some(thread::current().id());
            state.function.take().expect("an uncancelled callback")
        };
        // Release captured JNI references before advertising completion,
        // including if a Rust handler unwinds. No callback lock is held
        // while native code calls back into stop.
        struct Finished<'a>(&'a ExitCallback);
        impl Drop for Finished<'_> {
            fn drop(&mut self) {
                let exit = self.0;
                exit.lock().running = None;
                exit.0.finished.notify_all();
            }
        }
        let _finished = Finished(self);
        {
            let handler = function;
            handler(&reason);
        }
    }

    /// Suppress a callback not entered yet, or wait for an entered one.
    /// A callback may stop its own host; it must not wait for itself.
    pub(crate) fn cancel_and_wait(&self) {
        let mut state = self.lock();
        state.cancelled = true;
        let pending = state.function.take();
        while state.running.is_some() && state.running != Some(thread::current().id()) {
            state = self
                .0
                .finished
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        drop(state);
        drop(pending);
        if let Some(drain) = &self.0.drain {
            drain();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    extern "C" fn count(context: *mut c_void, _: *const c_char) {
        // The test keeps the counter alive until the callback is drained.
        let counter = unsafe { &*(context as *const std::sync::atomic::AtomicUsize) };
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn a_callback_scheduled_before_stop_cannot_enter_after_stop_returns() {
        let counter = std::sync::atomic::AtomicUsize::new(0);
        let exit = ExitCallback::new(count, std::ptr::from_ref(&counter).cast_mut().cast());
        let late = exit.clone();
        let (release, pending) = mpsc::channel();
        let thread = thread::spawn(move || {
            pending.recv().unwrap();
            late.invoke(CString::new("late").unwrap());
        });
        exit.cancel_and_wait();
        release.send(()).unwrap();
        thread.join().unwrap();
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
        exit.cancel_and_wait(); // Repeated stop is safe.
    }

    struct Blocked {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    extern "C" fn block(context: *mut c_void, _: *const c_char) {
        let probe = unsafe { &*(context as *const Blocked) };
        probe.entered.send(()).unwrap();
        probe.release.lock().unwrap().recv().unwrap();
    }

    #[test]
    fn stop_waits_for_an_entered_callback() {
        let (entered, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let probe = Blocked {
            entered,
            release: Mutex::new(wait),
        };
        let exit = ExitCallback::new(block, std::ptr::from_ref(&probe).cast_mut().cast());
        exit.clone().call(CString::new("exit").unwrap()).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let (done, stopped) = mpsc::channel();
        let stopping = exit.clone();
        let thread = thread::spawn(move || {
            stopping.cancel_and_wait();
            done.send(()).unwrap();
        });
        assert!(stopped.recv_timeout(Duration::from_millis(50)).is_err());
        release.send(()).unwrap();
        stopped.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
        assert!(!exit.is_running());
    }

    extern "C" fn stop_self(context: *mut c_void, _: *const c_char) {
        let exit = unsafe { &*(context as *const Mutex<Option<ExitCallback>>) };
        let exit = exit.lock().unwrap().clone().unwrap();
        assert!(exit.is_current());
        exit.cancel_and_wait();
    }

    #[test]
    fn a_callback_can_cancel_itself() {
        let slot = Mutex::new(None);
        let exit = ExitCallback::new(stop_self, std::ptr::from_ref(&slot).cast_mut().cast());
        *slot.lock().unwrap() = Some(exit.clone());
        let active = exit.clone();
        let (done, finished) = mpsc::channel();
        let thread = thread::spawn(move || {
            active.invoke(CString::new("exit").unwrap());
            done.send(()).unwrap();
        });
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
        exit.cancel_and_wait();
    }

    #[test]
    fn a_failed_spawn_does_not_leave_stop_waiting_for_a_callback() {
        let counter = std::sync::atomic::AtomicUsize::new(0);
        let exit = ExitCallback::new(count, std::ptr::from_ref(&counter).cast_mut().cast());
        let failed = exit.clone().schedule(CString::new("exit").unwrap(), |_| {
            Err(std::io::Error::other("test: no thread available"))
        });
        assert!(failed.is_err());
        exit.cancel_and_wait();
        assert!(!exit.is_running());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn cancellation_releases_an_owned_listener_without_a_new_start() {
        let listener = Arc::new(());
        let weak = Arc::downgrade(&listener);
        let exit = ExitCallback::from_fn(move |_| {
            let _listener = &listener;
        });
        assert!(weak.upgrade().is_some());
        exit.cancel_and_wait();
        // The process keeps its stopping slot and gate. It must not keep
        // a stopped service's JNI GlobalRef alive through that gate.
        assert!(weak.upgrade().is_none());
        exit.invoke(CString::new("late").unwrap());
    }

    #[test]
    fn an_owned_listener_is_released_when_a_handler_unwinds() {
        let listener = Arc::new(());
        let weak = Arc::downgrade(&listener);
        let exit = ExitCallback::from_fn(move |_| {
            let _listener = &listener;
            panic!("test handler unwind");
        });
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            exit.invoke(CString::new("exit").unwrap());
        }))
        .is_err());
        exit.cancel_and_wait();
        assert!(!exit.is_running());
        assert!(weak.upgrade().is_none());
    }
}
