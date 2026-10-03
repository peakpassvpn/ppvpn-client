//! Watch connections (`Command::Watch`): long-lived IPC connections over
//! which the service tells a client, as it happens, that its session's TUN
//! core stopped (`core_stopped`) or that the service is shutting down
//! (`stopping`). Between events the service writes a keepalive; the client
//! reads EOF or a broken pipe as "the service is gone".
//!
//! Lock order: `CORE`, then the hub. The hub never takes `CORE`, and a watch
//! connection's writer only ever waits on its own channel, so a watcher can
//! neither hold up a core operation nor the service's shutdown.

use crate::protocol::SessionRef;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Pause between keepalives on an idle watch connection. The client gives up
/// on a watch that stayed silent for a few of these.
pub const KEEPALIVE: Duration = Duration::from_secs(5);
/// How long the stopping sequence waits for watch connections to write
/// `stopping` before it closes them anyway.
pub const CLOSE_GRACE: Duration = Duration::from_millis(100);
/// Watch connections open at once (one per connected client is normal).
const MAX_WATCHERS: usize = 32;

/// The global hub, shared by the IPC server and `CORE`.
pub static HUB: Lazy<Arc<Hub>> = Lazy::new(|| Arc::new(Hub::default()));

/// What a watch connection is told; each ends the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The service is shutting down (its core stops next).
    Stopping,
    /// The session's core stopped, with the reason (see `core::stop_reason`).
    CoreStopped(String),
}

/// Forces a watch connection closed, even while its writer is blocked on a
/// client that stopped reading (Unix: `shutdown`; Windows: cancel the
/// pending write and disconnect the pipe instance).
pub type Closer = Box<dyn Fn() + Send + Sync>;

struct Watcher {
    id: u64,
    session: SessionRef,
    events: mpsc::Sender<Event>,
    closer: Option<Closer>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    /// Set by [`Hub::close_all`]: no new watchers from then on.
    closed: bool,
    watchers: Vec<Watcher>,
}

#[derive(Default)]
pub struct Hub {
    state: Mutex<State>,
    /// Watch connections whose writer is still running (including those
    /// already removed from `state`).
    live: AtomicUsize,
}

/// One registered watch connection. Dropping it (the writer finished)
/// unregisters it.
pub struct Registration {
    id: u64,
    hub: Arc<Hub>,
    pub events: mpsc::Receiver<Event>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        // Drops this watcher's closer too (on Unix a duplicate of the socket).
        let removed = {
            let mut state = self.hub.state.lock();
            state
                .watchers
                .iter()
                .position(|watcher| watcher.id == self.id)
                .map(|index| state.watchers.swap_remove(index))
        };
        drop(removed);
        self.hub.live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Hub {
    /// Registers a watcher for `session`. Refused once the service is
    /// stopping (`SERVICE_STOPPING`) or when too many are open.
    pub fn register(
        self: &Arc<Self>,
        session: SessionRef,
        closer: Option<Closer>,
    ) -> Result<Registration, &'static str> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(crate::ipc::SERVICE_STOPPING);
        }
        if state.watchers.len() >= MAX_WATCHERS {
            return Err("WATCH_LIMIT_REACHED");
        }
        state.next_id += 1;
        let id = state.next_id;
        let (events, receiver) = mpsc::channel();
        state.watchers.push(Watcher {
            id,
            session,
            events,
            closer,
        });
        self.live.fetch_add(1, Ordering::SeqCst);
        Ok(Registration {
            id,
            hub: self.clone(),
            events: receiver,
        })
    }

    /// `session`'s core stopped: its watchers are told why, and their
    /// connections end after that. Never blocks (called under `CORE`).
    pub fn core_stopped(&self, session: &SessionRef, reason: &str) {
        let mut state = self.state.lock();
        let mut index = 0;
        while index < state.watchers.len() {
            if state.watchers[index].session == *session {
                let watcher = state.watchers.swap_remove(index);
                let _ = watcher.events.send(Event::CoreStopped(reason.to_string()));
            } else {
                index += 1;
            }
        }
    }

    /// The service is stopping: every watcher is told so, then its connection
    /// is closed — once it has written `stopping`, or after `grace` for a
    /// writer blocked on a client that does not read. Refuses new watchers
    /// from now on. Returns within about `grace`.
    pub fn close_all(&self, grace: Duration) {
        let watchers = {
            let mut state = self.state.lock();
            state.closed = true;
            std::mem::take(&mut state.watchers)
        };
        let count = watchers.len();
        let mut closers = Vec::with_capacity(count);
        for watcher in watchers {
            let _ = watcher.events.send(Event::Stopping);
            // Dropping the sender ends the writer right after `stopping`.
            closers.extend(watcher.closer);
        }
        let deadline = Instant::now() + grace;
        while self.live.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        let stuck = self.live.load(Ordering::SeqCst);
        for close in &closers {
            close();
        }
        if count > 0 {
            log::info!("closed {count} watch connection(s) ({stuck} still writing were cut off)");
        }
    }

    #[cfg(test)]
    pub fn watchers(&self) -> usize {
        self.state.lock().watchers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, generation: u64) -> SessionRef {
        SessionRef {
            session_id: id.to_string(),
            generation,
        }
    }

    #[test]
    fn core_stopped_reaches_only_that_sessions_watchers() {
        let hub = Arc::new(Hub::default());
        let mine = hub.register(session("a", 1), None).unwrap();
        let older = hub.register(session("a", 0), None).unwrap();
        hub.core_stopped(&session("a", 1), "exited");
        assert_eq!(
            mine.events.recv().unwrap(),
            Event::CoreStopped("exited".into())
        );
        // The sender is gone: the writer ends after the event.
        assert!(mine.events.recv().is_err());
        assert!(older
            .events
            .recv_timeout(Duration::from_millis(20))
            .is_err_and(|error| error == mpsc::RecvTimeoutError::Timeout));
        assert_eq!(hub.watchers(), 1);
        drop(older);
        assert_eq!(hub.watchers(), 0);
        assert_eq!(hub.live.load(Ordering::SeqCst), 1, "mine still writing");
        drop(mine);
        assert_eq!(hub.live.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn close_all_says_stopping_refuses_new_watchers_and_never_waits_long() {
        let hub = Arc::new(Hub::default());
        let closed = Arc::new(AtomicUsize::new(0));
        let counter = closed.clone();
        // A writer that never finishes (blocked on a client that stopped
        // reading) must not hold up the stop.
        let stuck = hub
            .register(
                session("a", 1),
                Some(Box::new(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                })),
            )
            .unwrap();
        let started = Instant::now();
        hub.close_all(Duration::from_millis(50));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(closed.load(Ordering::SeqCst), 1, "forced closed");
        assert_eq!(stuck.events.recv().unwrap(), Event::Stopping);
        assert!(stuck.events.recv().is_err());
        assert_eq!(
            hub.register(session("a", 1), None).err(),
            Some(crate::ipc::SERVICE_STOPPING)
        );
    }
}
