//! Bounded notification fanout. Never blocks publishers.

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

#[derive(Debug)]
pub(crate) struct Notifier {
    capacity: usize,
    subscribers: Mutex<Vec<Subscriber>>,
    closed: Mutex<bool>,
}

impl Notifier {
    pub(crate) fn new(limits: &SessionLimits) -> Arc<Self> {
        Arc::new(Self {
            capacity: limits.notification_queue_capacity.max(1),
            subscribers: Mutex::new(Vec::new()),
            closed: Mutex::new(false),
        })
    }

    pub(crate) fn subscribe(self: &Arc<Self>) -> Option<Subscription> {
        if *self.closed.lock().expect("closed") {
            return None;
        }
        let (tx, rx) = mpsc::sync_channel(self.capacity);
        self.subscribers
            .lock()
            .expect("subscribers")
            .push(Subscriber { tx });
        Some(Subscription { rx })
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
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
        *subs = live;
    }

    pub(crate) fn close(&self) {
        *self.closed.lock().expect("closed") = true;
        self.subscribers.lock().expect("subscribers").clear();
    }
}
