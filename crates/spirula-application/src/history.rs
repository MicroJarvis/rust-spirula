//! Bounded recoverable control-event history and command-result retention.
//!
//! Accepted-command expiry uses an O(1) watermark over a contiguous command-id
//! space — never an unbounded seen-set. Control-ring byte/count caps are hard:
//! a single entry larger than `control_history_max_bytes` is dropped (watermark
//! advanced for resync) while its terminal may still live in the bounded
//! result cache.

use std::collections::{HashMap, VecDeque};

use crate::event::{CommandResult, DropCounters, EventEnvelope, ReplayOutcome, Snapshot};
use crate::ids::CommandId;
use crate::limits::SessionLimits;

#[derive(Debug)]
pub(crate) struct ControlHistory {
    capacity: usize,
    max_bytes: usize,
    entries: VecDeque<EventEnvelope>,
    bytes: usize,
    /// Max seq of any control event dropped from retention. Resync when
    /// `max_dropped_control_seq > cursor.seq`.
    max_dropped_control_seq: u64,
    result_retention: usize,
    results: HashMap<CommandId, EventEnvelope>,
    result_order: VecDeque<CommandId>,
    /// Highest accepted command id (dedicated contiguous command id space).
    /// Any id in `1..=max_accepted` that is not Ready/Pending is Expired.
    max_accepted_command: u64,
}

impl ControlHistory {
    pub(crate) fn new(limits: &SessionLimits) -> Self {
        let result_retention = limits.result_retention.max(1);
        Self {
            capacity: limits.control_history_capacity.max(1),
            max_bytes: limits.control_history_max_bytes.max(1),
            entries: VecDeque::new(),
            bytes: 0,
            max_dropped_control_seq: 0,
            result_retention,
            results: HashMap::new(),
            result_order: VecDeque::new(),
            max_accepted_command: 0,
        }
    }

    pub(crate) fn note_accepted(&mut self, id: CommandId) {
        self.max_accepted_command = self.max_accepted_command.max(id.get());
    }

    #[cfg(test)]
    pub(crate) fn max_accepted_command(&self) -> u64 {
        self.max_accepted_command
    }

    #[cfg(test)]
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub(crate) fn retained_len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    #[cfg(test)]
    pub(crate) fn max_dropped_control_seq(&self) -> u64 {
        self.max_dropped_control_seq
    }

    /// Retain a terminal for `command_result` without publishing (post-shutdown).
    pub(crate) fn store_result(&mut self, id: CommandId, envelope: EventEnvelope) {
        self.note_accepted(id);
        self.retain_result(id, envelope);
    }

    fn drop_front(&mut self, drops: &mut DropCounters) {
        if let Some(old) = self.entries.pop_front() {
            self.bytes = self.bytes.saturating_sub(old.event.approx_bytes() + 48);
            drops.control_history = drops.control_history.saturating_add(1);
            self.max_dropped_control_seq = self.max_dropped_control_seq.max(old.seq);
        }
    }

    pub(crate) fn push_control(&mut self, envelope: EventEnvelope, drops: &mut DropCounters) {
        debug_assert!(envelope.event.is_control());
        let size = envelope.event.approx_bytes() + 48;

        // Result cache is independent of the control-ring byte budget.
        if envelope.event.is_terminal_for_command()
            && let Some(id) = envelope.command_id
        {
            self.retain_result(id, envelope.clone());
        }

        // Never retain a single entry over the byte budget.
        if size > self.max_bytes {
            drops.control_history = drops.control_history.saturating_add(1);
            self.max_dropped_control_seq = self.max_dropped_control_seq.max(envelope.seq);
            return;
        }

        while self.entries.len() >= self.capacity
            || (self.bytes + size > self.max_bytes && !self.entries.is_empty())
        {
            self.drop_front(drops);
        }

        debug_assert!(self.bytes + size <= self.max_bytes || self.entries.is_empty());
        self.bytes = self.bytes.saturating_add(size);
        debug_assert!(self.bytes <= self.max_bytes);
        self.entries.push_back(envelope);
    }

    fn retain_result(&mut self, id: CommandId, envelope: EventEnvelope) {
        if self.results.insert(id, envelope).is_none() {
            self.result_order.push_back(id);
        }
        while self.result_order.len() > self.result_retention {
            if let Some(evict) = self.result_order.pop_front() {
                self.results.remove(&evict);
            }
        }
    }

    /// Replay control events after `cursor_seq`.
    ///
    /// Resync only when a **dropped control** seq is strictly after the cursor.
    /// Progress-only seq gaps do not bump that watermark.
    pub(crate) fn replay_control_after(
        &self,
        cursor_seq: u64,
        snapshot: Snapshot,
    ) -> ReplayOutcome {
        if self.max_dropped_control_seq > cursor_seq {
            return ReplayOutcome::ResyncRequired {
                snapshot: Box::new(snapshot),
                retained_terminals: self.retained_terminals(),
            };
        }
        let events: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.seq > cursor_seq)
            .cloned()
            .collect();
        ReplayOutcome::ControlEvents(events)
    }

    pub(crate) fn command_result(&self, id: CommandId, active: Option<CommandId>) -> CommandResult {
        if let Some(env) = self.results.get(&id) {
            return CommandResult::Ready(env.clone());
        }
        if active == Some(id) {
            return CommandResult::Pending;
        }
        if id.get() > 0 && id.get() <= self.max_accepted_command {
            CommandResult::Expired
        } else {
            CommandResult::Unknown
        }
    }

    /// Terminals still available for resync: result-cache first (survives
    /// control-ring overflow), then any control-ring terminals not already
    /// listed.
    pub(crate) fn retained_terminals(&self) -> Vec<EventEnvelope> {
        let mut out: Vec<EventEnvelope> = self
            .result_order
            .iter()
            .filter_map(|id| self.results.get(id).cloned())
            .collect();
        for env in self
            .entries
            .iter()
            .filter(|e| e.event.is_terminal_for_command())
        {
            let already = env
                .command_id
                .is_some_and(|id| out.iter().any(|e| e.command_id == Some(id)));
            if !already {
                out.push(env.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandPayload;
    use crate::event::{Event, EventCursor};
    use crate::ids::SessionId;
    use crate::state::SessionState;
    use spirula_core::{CancelReason, ContextKey, DomainError, ErrorCode, OutcomeView};

    fn limits_tiny_bytes() -> SessionLimits {
        SessionLimits {
            control_history_capacity: 8,
            control_history_max_bytes: 64,
            result_retention: 4,
            label_max_bytes: 32,
            error_event_max_bytes: 256,
            ..SessionLimits::for_test()
        }
    }

    fn envelope(seq: u64, command_id: Option<CommandId>, event: Event) -> EventEnvelope {
        EventEnvelope {
            event_version: 1,
            session_id: SessionId::new(1),
            run_id: None,
            seq,
            command_id,
            monotonic_ms: 0,
            event,
        }
    }

    #[test]
    fn oversized_control_entry_dropped_but_result_retained() {
        let limits = limits_tiny_bytes();
        let mut hist = ControlHistory::new(&limits);
        let mut drops = DropCounters::default();
        let id = CommandId::new(1);
        hist.note_accepted(id);
        let huge = "x".repeat(10_000);
        let event = Event::Completed {
            outcome: OutcomeView::Canceled {
                step: 0,
                reason: CancelReason::User,
                saved_checkpoint: Some(huge),
            },
        };
        let size = event.approx_bytes() + 48;
        assert!(size > hist.max_bytes());
        hist.push_control(envelope(1, Some(id), event), &mut drops);
        assert_eq!(hist.retained_len(), 0);
        assert!(hist.retained_bytes() <= hist.max_bytes());
        assert_eq!(hist.max_dropped_control_seq(), 1);
        assert!(drops.control_history >= 1);
        assert!(matches!(
            hist.command_result(id, None),
            CommandResult::Ready(_)
        ));
        assert!(matches!(
            hist.replay_control_after(
                0,
                Snapshot {
                    session_id: SessionId::new(1),
                    seq: 1,
                    state: SessionState::Ready,
                    active_command: None,
                    active_run: None,
                    last_outcome: None,
                    drops: DropCounters::default(),
                    loaded_label: None,
                    cursor: EventCursor::new(SessionId::new(1), 1),
                }
            ),
            ReplayOutcome::ResyncRequired { .. }
        ));
    }

    #[test]
    fn long_lifetime_watermark_stays_constant_space_and_expired() {
        let limits = SessionLimits {
            control_history_capacity: 4,
            control_history_max_bytes: 512,
            result_retention: 2,
            ..SessionLimits::for_test()
        };
        let mut hist = ControlHistory::new(&limits);
        let mut drops = DropCounters::default();
        let first = CommandId::new(1);
        for n in 1..=200u64 {
            let id = CommandId::new(n);
            hist.note_accepted(id);
            let event = Event::CommandAccepted {
                command: CommandPayload::ProbeRun { steps: 1 },
            };
            hist.push_control(envelope(n, Some(id), event), &mut drops);
            let terminal = Event::Completed {
                outcome: OutcomeView::Completed { step: 1 },
            };
            hist.push_control(envelope(n + 10_000, Some(id), terminal), &mut drops);
        }
        assert!(hist.retained_bytes() <= hist.max_bytes());
        assert!(hist.retained_len() <= limits.control_history_capacity.max(1));
        assert_eq!(hist.max_accepted_command(), 200);
        assert!(matches!(
            hist.command_result(first, None),
            CommandResult::Expired
        ));
        assert!(matches!(
            hist.command_result(CommandId::new(201), None),
            CommandResult::Unknown
        ));
        let _ = drops;
    }

    #[test]
    fn rejected_payload_bytes_include_label_and_error() {
        let label = "y".repeat(40);
        let error = DomainError::try_new(ErrorCode::ValueOutOfRange, "label exceeds size limit")
            .unwrap()
            .with_context(ContextKey::Field, "label")
            .unwrap();
        let event = Event::CommandRejected {
            command: CommandPayload::ProbeLoad {
                label: label.clone(),
            },
            error,
        };
        let size = event.approx_bytes();
        assert!(size > 48 + "probe_load".len());
        assert!(size >= 48 + 16 + label.len());
    }
}
