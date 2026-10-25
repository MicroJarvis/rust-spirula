//! Bounded recoverable control-event history and command-result retention.

use std::collections::{HashMap, HashSet, VecDeque};

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
    /// `cursor.seq < max_dropped_control_seq` is false and
    /// `cursor.seq < max_dropped` — i.e. a dropped control lies after the cursor.
    max_dropped_control_seq: u64,
    result_retention: usize,
    results: HashMap<CommandId, EventEnvelope>,
    result_order: VecDeque<CommandId>,
    /// Accepted command ids. Never forgotten so Expired cannot become Unknown.
    seen_commands: HashSet<CommandId>,
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
            seen_commands: HashSet::new(),
        }
    }

    pub(crate) fn note_accepted(&mut self, id: CommandId) {
        self.seen_commands.insert(id);
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
        while self.entries.len() >= self.capacity
            || (self.bytes + size > self.max_bytes && !self.entries.is_empty())
        {
            if self.entries.is_empty() {
                break;
            }
            self.drop_front(drops);
        }
        if size > self.max_bytes && !self.entries.is_empty() {
            while !self.entries.is_empty() {
                self.drop_front(drops);
            }
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
                // `seen_commands` keeps the id so command_result stays Expired.
            }
        }
    }

    /// Replay control events after `cursor_seq`.
    ///
    /// Resync only when a **dropped control** seq is strictly after the cursor
    /// (`max_dropped_control_seq > cursor_seq`). Progress-only seq gaps do not
    /// bump that watermark and must not force resync.
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
        if self.seen_commands.contains(&id) {
            CommandResult::Expired
        } else {
            CommandResult::Unknown
        }
    }

    /// Terminals still available for resync: result-cache first (survives
    /// control-ring overflow), then any control-ring terminals not already
    /// listed. Callers must not invent gap-fill events.
    pub(crate) fn retained_terminals(&self) -> Vec<EventEnvelope> {
        let mut out: Vec<EventEnvelope> = self
            .result_order
            .iter()
            .filter_map(|id| self.results.get(id).cloned())
            .collect();
        let seen: HashSet<CommandId> = out.iter().filter_map(|e| e.command_id).collect();
        for env in self
            .entries
            .iter()
            .filter(|e| e.event.is_terminal_for_command())
        {
            if env.command_id.map(|id| !seen.contains(&id)).unwrap_or(true) {
                out.push(env.clone());
            }
        }
        out
    }
}
