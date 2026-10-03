//! Event delivery (docs/host-integration.md, section 6): every subscription
//! has one bounded buffer per kind, so a burst of one kind never pushes out
//! another. A full buffer drops its oldest event and counts it; the receiver
//! then reads a `Lagged` item where the dropped events were. Items of
//! different kinds come out in the order they were published.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::Notify;

use crate::event::{Event, EventItem, EventKind};

/// Events of one kind a subscription holds before it drops the oldest.
pub(crate) const KIND_CAPACITY: usize = 64;

#[derive(Default)]
pub(crate) struct Bus {
    subscribers: Mutex<Vec<Weak<Shared>>>,
    closed: Mutex<bool>,
}

impl Bus {
    pub(crate) fn subscribe(&self, kinds: &[EventKind]) -> Subscription {
        let shared = Arc::new(Shared {
            kinds: kinds.to_vec(),
            queues: Mutex::default(),
            notify: Notify::new(),
        });
        if *self.closed.lock().expect("bus") {
            shared.queues.lock().expect("queues").closed = true;
        } else {
            let mut subscribers = self.subscribers.lock().expect("bus");
            subscribers.retain(|s| s.strong_count() > 0);
            subscribers.push(Arc::downgrade(&shared));
        }
        Subscription { shared }
    }

    pub(crate) fn publish(&self, event: Event) {
        let kind = event.kind();
        let mut subscribers = self.subscribers.lock().expect("bus");
        subscribers.retain(|s| s.strong_count() > 0);
        for shared in subscribers.iter().filter_map(Weak::upgrade) {
            if shared.kinds.contains(&kind) {
                shared.push(kind, event.clone());
            }
        }
    }

    /// Ends every subscription (shutdown): receivers read what is buffered,
    /// then `None`; later subscriptions are closed from the start.
    pub(crate) fn close(&self) {
        *self.closed.lock().expect("bus") = true;
        for shared in self
            .subscribers
            .lock()
            .expect("bus")
            .drain(..)
            .filter_map(|s| s.upgrade())
        {
            shared.queues.lock().expect("queues").closed = true;
            shared.notify.notify_one();
        }
    }
}

struct Shared {
    kinds: Vec<EventKind>,
    queues: Mutex<Queues>,
    notify: Notify,
}

#[derive(Default)]
struct Queues {
    seq: u64,
    kinds: HashMap<EventKind, KindQueue>,
    closed: bool,
}

#[derive(Default)]
struct KindQueue {
    events: VecDeque<(u64, Event)>,
    /// Dropped since the receiver last read this kind.
    dropped: u64,
    /// Where the first of them was.
    lagged_at: u64,
}

impl KindQueue {
    /// The position of what the receiver reads next of this kind.
    fn head(&self) -> Option<u64> {
        if self.dropped > 0 {
            Some(self.lagged_at)
        } else {
            self.events.front().map(|(seq, _)| *seq)
        }
    }
}

impl Shared {
    fn push(&self, kind: EventKind, event: Event) {
        let mut queues = self.queues.lock().expect("queues");
        if queues.closed {
            return;
        }
        queues.seq += 1;
        let seq = queues.seq;
        let queue = queues.kinds.entry(kind).or_default();
        if queue.events.len() >= KIND_CAPACITY {
            if let Some((oldest, _)) = queue.events.pop_front() {
                if queue.dropped == 0 {
                    queue.lagged_at = oldest;
                }
                queue.dropped += 1;
            }
        }
        queue.events.push_back((seq, event));
        drop(queues);
        self.notify.notify_one();
    }

    /// The next item in publication order; `Err` when closed and empty.
    fn next(&self) -> Result<Option<EventItem>, ()> {
        let mut queues = self.queues.lock().expect("queues");
        let next = queues
            .kinds
            .iter()
            .filter_map(|(kind, q)| q.head().map(|seq| (seq, *kind)))
            .min_by_key(|(seq, _)| *seq);
        let Some((_, kind)) = next else {
            return if queues.closed { Err(()) } else { Ok(None) };
        };
        let queue = queues.kinds.get_mut(&kind).expect("queued kind");
        if queue.dropped > 0 {
            let dropped = std::mem::take(&mut queue.dropped);
            return Ok(Some(EventItem::Lagged { kind, dropped }));
        }
        Ok(queue
            .events
            .pop_front()
            .map(|(_, event)| EventItem::Event { event }))
    }
}

/// One subscriber's end; dropping it unsubscribes.
pub(crate) struct Subscription {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("kinds", &self.shared.kinds)
            .finish_non_exhaustive()
    }
}

impl Subscription {
    pub(crate) async fn recv(&mut self) -> Option<EventItem> {
        loop {
            match self.shared.next() {
                Ok(Some(item)) => return Some(item),
                Err(()) => return None,
                // A push between next and here leaves a permit: no wake-up
                // is lost.
                Ok(None) => self.shared.notify.notified().await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use futures_util::FutureExt;

    use super::*;

    fn applied(revision: &str) -> Event {
        Event::ProfileApplied {
            at: Utc::now(),
            revision: revision.into(),
        }
    }

    fn started() -> Event {
        Event::CoreStarted { at: Utc::now() }
    }

    fn revision(item: Option<EventItem>) -> String {
        match item {
            Some(EventItem::Event {
                event: Event::ProfileApplied { revision, .. },
            }) => revision,
            other => panic!("not ProfileApplied: {other:?}"),
        }
    }

    #[tokio::test]
    async fn only_the_kinds_subscribed_in_publication_order() {
        let bus = Bus::default();
        let mut all = bus.subscribe(EventKind::ALL);
        let mut started_only = bus.subscribe(&[EventKind::CoreStarted]);
        bus.publish(applied("r1"));
        bus.publish(started());
        bus.publish(applied("r2"));
        assert_eq!(revision(all.recv().await), "r1");
        assert!(matches!(
            all.recv().await,
            Some(EventItem::Event {
                event: Event::CoreStarted { .. }
            })
        ));
        assert_eq!(revision(all.recv().await), "r2");
        assert!(all.recv().now_or_never().is_none(), "nothing more");
        assert!(matches!(
            started_only.recv().await,
            Some(EventItem::Event {
                event: Event::CoreStarted { .. }
            })
        ));
        assert!(started_only.recv().now_or_never().is_none());
    }

    #[tokio::test]
    async fn a_burst_of_one_kind_lags_only_that_kind() {
        let bus = Bus::default();
        let mut rx = bus.subscribe(EventKind::ALL);
        bus.publish(started());
        for i in 0..KIND_CAPACITY + 3 {
            bus.publish(applied(&format!("r{i}")));
        }
        // The other kind was not pushed out.
        assert!(matches!(
            rx.recv().await,
            Some(EventItem::Event {
                event: Event::CoreStarted { .. }
            })
        ));
        assert_eq!(
            rx.recv().await,
            Some(EventItem::Lagged {
                kind: EventKind::ProfileApplied,
                dropped: 3
            })
        );
        assert_eq!(revision(rx.recv().await), "r3");
        for _ in 4..KIND_CAPACITY + 3 {
            rx.recv().await.unwrap();
        }
        assert!(rx.recv().now_or_never().is_none());
    }

    #[tokio::test]
    async fn a_waiting_receiver_wakes_and_close_ends_it() {
        let bus = Arc::new(Bus::default());
        let mut rx = bus.subscribe(EventKind::ALL);
        let publisher = bus.clone();
        let task = tokio::spawn(async move { rx.recv().await.map(|_| rx) });
        tokio::task::yield_now().await;
        publisher.publish(started());
        let mut rx = task.await.unwrap().expect("woken with the event");
        bus.publish(applied("r1"));
        bus.close();
        assert_eq!(revision(rx.recv().await), "r1", "buffered items first");
        assert_eq!(rx.recv().await, None);
        let mut late = bus.subscribe(EventKind::ALL);
        assert_eq!(late.recv().await, None);
    }

    #[test]
    fn dropped_receivers_are_forgotten() {
        let bus = Bus::default();
        drop(bus.subscribe(EventKind::ALL));
        bus.publish(started());
        assert!(bus.subscribers.lock().unwrap().is_empty());
    }
}
