//! Bounded notification fanout. Never blocks publishers.
//!
//! Both per-subscriber queue depth and global subscriber count are capped.
//! Dropping a [`Subscription`] releases its slot immediately.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::event::{DropCounters, EventEnvelope};
use crate::limits::SessionLimits;

#[derive(Debug)]
struct Subscriber {
    id: u64,
    tx: SyncSender<EventEnvelope>,
}

/// Handle returned to callers that want live envelopes.
#[derive(Debug)]
pub struct Subscription {
    rx: Receiver<EventEnvelope>,
    id: u64,
    notifier: Weak<Notifier>,
}

impl Subscription {
    pub fn try_recv(&self) -> Option<EventEnvelope> {
        self.rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<EventEnvelope, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    pub fn recv(&self) -> Option<EventEnvelope> {
        self.rx.recv().ok()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(n) = self.notifier.upgrade() {
            n.unsubscribe(self.id);
        }
    }
}

/// Why `subscribe` refused a new live consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscribeError {
    Closed,
    SubscriberLimit,
}

#[derive(Debug)]
pub(crate) struct Notifier {
    capacity: usize,
    max_subscribers: usize,
    next_id: AtomicU64,
    subscribers: Mutex<Vec<Subscriber>>,
    closed: Mutex<bool>,
}

impl Notifier {
    pub(crate) fn new(limits: &SessionLimits) -> Arc<Self> {
        Arc::new(Self {
            capacity: limits.notification_queue_capacity.max(1),
            max_subscribers: limits.max_subscribers.max(1),
            next_id: AtomicU64::new(1),
            subscribers: Mutex::new(Vec::new()),
            closed: Mutex::new(false),
        })
    }

    pub(crate) fn subscribe(self: &Arc<Self>) -> Result<Subscription, SubscribeError> {
        if *self.closed.lock().expect("closed") {
            return Err(SubscribeError::Closed);
        }
        let mut subs = self.subscribers.lock().expect("subscribers");
        if subs.len() >= self.max_subscribers {
            return Err(SubscribeError::SubscriberLimit);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(self.capacity);
        subs.push(Subscriber { id, tx });
        Ok(Subscription {
            rx,
            id,
            notifier: Arc::downgrade(self),
        })
    }

    fn unsubscribe(&self, id: u64) {
        let mut subs = self.subscribers.lock().expect("subscribers");
        subs.retain(|s| s.id != id);
    }

    pub(crate) fn publish(&self, envelope: &EventEnvelope, drops: &mut DropCounters) {
        if *self.closed.lock().expect("closed") {
            return;
        }
        let mut subs = self.subscribers.lock().expect("subscribers");
        let mut live = Vec::with_capacity(subs.len());
        for sub in subs.drain(..) {
            match sub.tx.try_send(envelope.clone()) {
                Ok(()) => live.push(sub),
                Err(TrySendError::Full(_)) => {
                    drops.notification = drops.notification.saturating_add(1);
                    live.push(sub);
                }
                Err(TrySendError::Disconnected(_)) => {
                    drops.subscriber_disconnect = drops.subscriber_disconnect.saturating_add(1);
                }
            }
        }
        *subs = live;
    }

    pub(crate) fn close(&self) {
        *self.closed.lock().expect("closed") = true;
        self.subscribers.lock().expect("subscribers").clear();
    }
}
