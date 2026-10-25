//! Bounded notification fanout. Never blocks publishers.
//!
//! Both per-subscriber queue depth and global subscriber count are capped.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::event::{DropCounters, EventEnvelope};
use crate::limits::SessionLimits;

#[derive(Debug)]
struct Subscriber {
    tx: SyncSender<EventEnvelope>,
}

/// Handle returned to callers that want live envelopes.
#[derive(Debug)]
pub struct Subscription {
    rx: Receiver<EventEnvelope>,
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
    subscribers: Mutex<Vec<Subscriber>>,
    closed: Mutex<bool>,
}

impl Notifier {
    pub(crate) fn new(limits: &SessionLimits) -> Arc<Self> {
        Arc::new(Self {
            capacity: limits.notification_queue_capacity.max(1),
            max_subscribers: limits.max_subscribers.max(1),
            subscribers: Mutex::new(Vec::new()),
            closed: Mutex::new(false),
        })
    }

    pub(crate) fn subscribe(self: &Arc<Self>) -> Result<Subscription, SubscribeError> {
        if *self.closed.lock().expect("closed") {
            return Err(SubscribeError::Closed);
        }
        let mut subs = self.subscribers.lock().expect("subscribers");
        // Dead subscribers are pruned on publish (`Disconnected`). Count cap is
        // on currently registered handles, including stalled ones.
        if subs.len() >= self.max_subscribers {
            return Err(SubscribeError::SubscriberLimit);
        }
        let (tx, rx) = mpsc::sync_channel(self.capacity);
        subs.push(Subscriber { tx });
        Ok(Subscription { rx })
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
