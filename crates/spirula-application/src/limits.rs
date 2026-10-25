//! Explicit capacity, size, and retention policy for the session owner.

use std::time::Duration;

/// Bounded resource policy. Defaults match `docs/phase-1/application.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLimits {
    pub command_queue_capacity: usize,
    /// Replaceable progress/log mailbox (try_send; drops allowed).
    pub worker_progress_queue_capacity: usize,
    /// Terminal/load result mailbox (owner drains first; send may block briefly).
    pub worker_result_queue_capacity: usize,
    pub control_history_capacity: usize,
    pub control_history_max_bytes: usize,
    /// Per-subscriber notification queue capacity.
    pub notification_queue_capacity: usize,
    /// Hard cap on concurrent live subscribers (global fanout cardinality).
    pub max_subscribers: usize,
    pub progress_max_bytes: usize,
    pub log_message_max_bytes: usize,
    pub log_tail_capacity: usize,
    pub label_max_bytes: usize,
    pub reason_max_bytes: usize,
    pub error_message_max_bytes: usize,
    pub context_value_max_bytes: usize,
    pub error_event_max_bytes: usize,
    pub result_retention: usize,
    pub heartbeat_interval: Duration,
    /// Upper bound on cancel observation latency while a probe is active.
    pub cancel_poll_interval: Duration,
    /// Nominal delay per fake step; sliced by [`Self::cancel_poll_interval`].
    pub worker_step_delay: Duration,
    /// Owner join / worker drain deadline on service drop.
    pub teardown_deadline: Duration,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            command_queue_capacity: 8,
            worker_progress_queue_capacity: 32,
            worker_result_queue_capacity: 8,
            control_history_capacity: 64,
            control_history_max_bytes: 256 * 1024,
            notification_queue_capacity: 16,
            max_subscribers: 32,
            progress_max_bytes: 4 * 1024,
            log_message_max_bytes: 2 * 1024,
            log_tail_capacity: 32,
            label_max_bytes: 256,
            reason_max_bytes: 256,
            error_message_max_bytes: 1024,
            context_value_max_bytes: 256,
            error_event_max_bytes: 2 * 1024,
            result_retention: 32,
            heartbeat_interval: Duration::from_secs(1),
            cancel_poll_interval: Duration::from_millis(50),
            worker_step_delay: Duration::from_millis(10),
            teardown_deadline: Duration::from_secs(2),
        }
    }
}

impl SessionLimits {
    /// Fast defaults for unit tests (short heartbeat/step/poll).
    pub fn for_test() -> Self {
        Self {
            heartbeat_interval: Duration::from_millis(20),
            cancel_poll_interval: Duration::from_millis(5),
            worker_step_delay: Duration::from_millis(1),
            notification_queue_capacity: 2,
            max_subscribers: 4,
            command_queue_capacity: 2,
            worker_progress_queue_capacity: 4,
            worker_result_queue_capacity: 4,
            control_history_capacity: 8,
            control_history_max_bytes: 4 * 1024,
            result_retention: 4,
            log_tail_capacity: 8,
            label_max_bytes: 64,
            reason_max_bytes: 64,
            error_message_max_bytes: 128,
            context_value_max_bytes: 64,
            error_event_max_bytes: 256,
            teardown_deadline: Duration::from_millis(500),
            ..Self::default()
        }
        .normalized()
    }

    /// Clamp zero/tiny capacities and timing so the returned policy matches
    /// what the owner actually enforces (queues ≥ 1, cancel poll ≤ heartbeat
    /// and ≤ 250ms, byte floors large enough for overflow substitute text).
    pub fn normalized(mut self) -> Self {
        self.command_queue_capacity = self.command_queue_capacity.max(1);
        self.worker_progress_queue_capacity = self.worker_progress_queue_capacity.max(1);
        self.worker_result_queue_capacity = self.worker_result_queue_capacity.max(1);
        self.control_history_capacity = self.control_history_capacity.max(1);
        self.control_history_max_bytes = self.control_history_max_bytes.max(64);
        self.notification_queue_capacity = self.notification_queue_capacity.max(1);
        self.max_subscribers = self.max_subscribers.max(1);
        self.log_tail_capacity = self.log_tail_capacity.max(1);
        self.result_retention = self.result_retention.max(1);
        // Floors must fit `[omitted N bytes]` / overflow substitute payloads.
        self.label_max_bytes = self.label_max_bytes.max(32);
        self.reason_max_bytes = self.reason_max_bytes.max(32);
        self.log_message_max_bytes = self.log_message_max_bytes.max(32);
        self.progress_max_bytes = self.progress_max_bytes.max(64);
        self.error_message_max_bytes = self.error_message_max_bytes.max(64);
        self.context_value_max_bytes = self.context_value_max_bytes.max(32);
        self.error_event_max_bytes = self
            .error_event_max_bytes
            .max(self.error_message_max_bytes.saturating_add(64));

        if self.heartbeat_interval.is_zero() {
            self.heartbeat_interval = Duration::from_millis(1);
        }
        if self.worker_step_delay.is_zero() {
            self.worker_step_delay = Duration::from_millis(1);
        }
        if self.teardown_deadline.is_zero() {
            self.teardown_deadline = Duration::from_millis(1);
        }
        if self.cancel_poll_interval.is_zero() {
            self.cancel_poll_interval = Duration::from_millis(1);
        }
        let max_poll = Duration::from_millis(250).min(self.heartbeat_interval);
        if self.cancel_poll_interval > max_poll {
            self.cancel_poll_interval = max_poll;
        }
        self
    }

    pub(crate) fn poll_interval(&self) -> Duration {
        let poll = self.cancel_poll_interval;
        if poll.is_zero() {
            Duration::from_millis(1)
        } else if poll > self.heartbeat_interval {
            self.heartbeat_interval
        } else {
            poll
        }
    }
}
