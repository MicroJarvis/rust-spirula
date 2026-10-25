//! Bounded recoverable control-event history and command-result retention.

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
    /// Lowest control seq still retained.
    oldest_seq: Option<u64>,
    result_retention: usize,
    results: HashMap<CommandId, EventEnvelope>,
    result_order: VecDeque<CommandId>,
    /// Command ids that were accepted; used to distinguish Expired vs Unknown.
    seen_commands: HashMap<CommandId, bool>,
    seen_order: VecDeque<CommandId>,
    seen_capacity: usize,
}

impl ControlHistory {
    pub(crate) fn new(limits: &SessionLimits) -> Self {
        let result_retention = limits.result_retention.max(1);
        Self {
            capacity: limits.control_history_capacity.max(1),
            max_bytes: limits.control_history_max_bytes.max(1),
            entries: VecDeque::new(),
            bytes: 0,
            oldest_seq: None,
            result_retention,
            results: HashMap::new(),
            result_order: VecDeque::new(),
            seen_commands: HashMap::new(),
            seen_order: VecDeque::new(),
            // Bound seen-set similarly; expired results still need "seen".
            seen_capacity: result_retention.saturating_mul(4).max(16),
        }
    }

    pub(crate) fn note_accepted(&mut self, id: CommandId) {
        if self.seen_commands.insert(id, true).is_none() {
            self.seen_order.push_back(id);
        }
        while self.seen_order.len() > self.seen_capacity {
            if let Some(evict) = self.seen_order.pop_front() {
                // Keep if still in results.
                if !self.results.contains_key(&evict) {
                    self.seen_commands.remove(&evict);
                } else {
                    self.seen_order.push_back(evict);
                    break;
                }
            }
        }
    }

    pub(crate) fn push_control(&mut self, envelope: EventEnvelope, drops: &mut DropCounters) {
        debug_assert!(envelope.event.is_control());
        let size = envelope.event.approx_bytes() + 48;
        while self.entries.len() >= self.capacity
            || (self.bytes + size > self.max_bytes && !self.entries.is_empty())
        {
            if let Some(old) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.event.approx_bytes() + 48);
                drops.control_history = drops.control_history.saturating_add(1);
                self.oldest_seq = self.entries.front().map(|e| e.seq);
            } else {
                break;
            }
        }
        // Single entry larger than max_bytes: still admit one, dropping others.
        if size > self.max_bytes && !self.entries.is_empty() {
            while let Some(old) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.event.approx_bytes() + 48);
                drops.control_history = drops.control_history.saturating_add(1);
            }
            self.oldest_seq = None;
        }
        if self.oldest_seq.is_none() {
            self.oldest_seq = Some(envelope.seq);
        }
        self.bytes = self
            .bytes
            .saturating_add(size.min(self.max_bytes.saturating_mul(2)));
        if envelope.event.is_terminal_for_command()
            && let Some(id) = envelope.command_id
        {
            self.retain_result(id, envelope.clone());
        }
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

    /// Replay control events after `cursor_seq`. Caller already validated
    /// session identity and rejected future cursors.
    ///
    /// Replaceable progress/log also advance the global `seq` but are not
    /// retained here. A cursor that only "misses" those must receive
    /// `ControlEvents` (possibly empty), never `ResyncRequired`. Resync is
    /// only for a cursor older than the oldest retained **control** seq.
    pub(crate) fn replay_control_after(
        &self,
        cursor_seq: u64,
        snapshot: Snapshot,
    ) -> ReplayOutcome {
        if let Some(oldest) = self.oldest_seq
            && cursor_seq.saturating_add(1) < oldest
        {
            return ReplayOutcome::ResyncRequired {
                snapshot: Box::new(snapshot),
                retained_terminals: self.retained_terminals(),
            };
        }
        // All control entries expired. Distinguish from "only progress ran".
        if self.entries.is_empty()
            && self.oldest_seq.is_none()
            && snapshot.drops.control_history > 0
            && cursor_seq < snapshot.seq
        {
            return ReplayOutcome::ResyncRequired {
                snapshot: Box::new(snapshot),
                retained_terminals: Vec::new(),
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
        if self.seen_commands.contains_key(&id) {
            CommandResult::Expired
        } else {
            CommandResult::Unknown
        }
    }

    pub(crate) fn retained_terminals(&self) -> Vec<EventEnvelope> {
        self.entries
            .iter()
            .filter(|e| e.event.is_terminal_for_command())
            .cloned()
            .collect()
    }
}
