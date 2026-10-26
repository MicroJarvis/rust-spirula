//! Unique logical application/session owner. Session state is mutated only
//! under a shared owner mutex (caller `submit`/`Drop`, the mailbox owner
//! loop, and tests) — not "exactly one OS writer thread". Bounded admission.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use spirula_core::{CancelReason, CancelToken, ContextKey, DomainError, ErrorCode};

use crate::bound::{
    bound_command_payload, bound_error, bound_retained_label, check_label, check_log_message,
    check_progress, check_reason, sanitize_worker_event, validate_run_terminal_bundle,
    validate_worker_terminal,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
use crate::command::{Command, CommandPayload, SubmitOutcome};
use crate::disk_log::{DiskLogCounters, DiskLogHandle};
use crate::event::{
    CommandResult, CursorError, DropCounters, Event, EventCursor, EventEnvelope, LogLevel,
    ReplayOutcome, RunOutcome, Snapshot, outcome_from_terminal,
};
use crate::history::ControlHistory;
use crate::ids::{CommandId, IdGen, RunId, SessionId};
use crate::limits::SessionLimits;
use crate::notify::SubscribeError;
use crate::notify::{Notifier, Subscription};
use crate::state::SessionState;
use crate::worker::{FakeWorkerBehavior, StallGate, WorkerEvent, WorkerHandle, WorkerRequest};

struct LogTail {
    capacity: usize,
    entries: VecDeque<String>,
}

impl LogTail {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
        }
    }

    fn push(&mut self, message: String) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(message);
    }
}

struct PendingStop {
    save: bool,
}

struct State {
    session_id: SessionId,
    limits: SessionLimits,
    phase: SessionState,
    seq: u64,
    started: Instant,
    drops: DropCounters,
    history: ControlHistory,
    log_tail: LogTail,
    active_command: Option<CommandId>,
    active_run: Option<RunId>,
    active_payload: Option<CommandPayload>,
    last_outcome: Option<RunOutcome>,
    loaded_label: Option<String>,
    cancel: CancelToken,
    pause: Arc<Mutex<bool>>,
    pending_stop: Option<PendingStop>,
    pending_shutdown: Option<CommandId>,
    /// First accepted Stop id that owns the cancel terminal when no probe runs.
    stop_owner: Option<CommandId>,
    shutdown_owner: Option<CommandId>,
    shutdown_done: bool,
    /// Contiguous command identities (separate from run ids).
    command_ids: IdGen,
    run_ids: IdGen,
    latest_progress: Option<spirula_core::ProgressSnapshot>,
    /// Live save-on-cancel flag shared with the running fake worker.
    active_save_on_cancel: Option<Arc<AtomicBool>>,
    /// Optional disk diagnostics handle (clone of [`Shared::disk_log`]).
    disk_log: Option<DiskLogHandle>,
}

impl State {
    fn snapshot(&self) -> Snapshot {
        let cursor = EventCursor::new(self.session_id, self.seq);
        Snapshot {
            session_id: self.session_id,
            seq: self.seq,
            state: self.phase,
            active_command: self.active_command,
            active_run: self.active_run,
            last_outcome: self.last_outcome.clone(),
            drops: self.drops,
            loaded_label: self.loaded_label.clone(),
            cursor,
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq = self.seq.saturating_add(1);
        self.seq
    }

    fn monotonic_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
}

enum AdmittedWork {
    Payload {
        id: CommandId,
        payload: CommandPayload,
    },
}

struct Shared {
    state: Mutex<State>,
    notifier: Arc<Notifier>,
    worker: Mutex<WorkerHandle>,
    behavior: Arc<Mutex<FakeWorkerBehavior>>,
    stall: StallGate,
    /// Optional rotated disk diagnostics (P1-F09). Enqueue never blocks owner.
    disk_log: Option<DiskLogHandle>,
}

/// Headless application/session service (P1-F07).
pub struct ApplicationService {
    shared: Arc<Shared>,
    work_tx: SyncSender<AdmittedWork>,
    /// Test injection into the progress mailbox (stale progress/log).
    progress_inject_tx: SyncSender<WorkerEvent>,
    /// Test injection into the result mailbox (late terminals).
    result_inject_tx: SyncSender<WorkerEvent>,
    owner: Mutex<Option<JoinHandle<()>>>,
}

impl ApplicationService {
    pub fn start(limits: SessionLimits) -> Self {
        Self::start_with(limits, None)
    }

    /// Start with optional rotated disk diagnostics (P1-F09). Disk I/O never
    /// blocks command admission, workers, or notify fanout.
    pub fn start_with(
        limits: SessionLimits,
        disk_log: Option<crate::disk_log::DiskLogConfig>,
    ) -> Self {
        let limits = limits.normalized();
        let session_id = SessionId::allocate();
        let notifier = Notifier::new(&limits);
        let behavior = Arc::new(Mutex::new(FakeWorkerBehavior::Succeed));
        let stall = StallGate::new();
        let (work_tx, work_rx) = mpsc::sync_channel(limits.command_queue_capacity.max(1));
        let (progress_tx, progress_rx) =
            mpsc::sync_channel(limits.worker_progress_queue_capacity.max(1));
        let (result_tx, result_rx) = mpsc::sync_channel(limits.worker_result_queue_capacity.max(1));

        let mut state = State {
            session_id,
            limits: limits.clone(),
            phase: SessionState::Idle,
            seq: 0,
            started: Instant::now(),
            drops: DropCounters::default(),
            history: ControlHistory::new(&limits),
            log_tail: LogTail::new(limits.log_tail_capacity),
            active_command: None,
            active_run: None,
            active_payload: None,
            last_outcome: None,
            loaded_label: None,
            cancel: CancelToken::new(),
            pause: Arc::new(Mutex::new(false)),
            pending_stop: None,
            pending_shutdown: None,
            stop_owner: None,
            shutdown_owner: None,
            shutdown_done: false,
            command_ids: IdGen::new(1),
            run_ids: IdGen::new(1),
            latest_progress: None,
            active_save_on_cancel: None,
            disk_log: None,
        };

        let worker = WorkerHandle::spawn(
            limits.clone(),
            Arc::clone(&behavior),
            progress_tx.clone(),
            result_tx.clone(),
        );
        let disk_handle = disk_log.map(DiskLogHandle::start);
        state.disk_log = disk_handle.clone();
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            notifier: Arc::clone(&notifier),
            worker: Mutex::new(worker),
            behavior: Arc::clone(&behavior),
            stall,
            disk_log: disk_handle,
        });

        let owner_shared = Arc::clone(&shared);
        let owner = thread::Builder::new()
            .name("spirula-app-owner".into())
            .spawn(move || owner_loop(owner_shared, work_rx, progress_rx, result_rx))
            .expect("spawn owner");

        Self {
            shared,
            work_tx,
            progress_inject_tx: progress_tx,
            result_inject_tx: result_tx,
            owner: Mutex::new(Some(owner)),
        }
    }

    pub fn session_id(&self) -> SessionId {
        lock(&self.shared.state).session_id
    }

    pub fn limits(&self) -> SessionLimits {
        lock(&self.shared.state).limits.clone()
    }

    pub fn set_worker_behavior(&self, behavior: FakeWorkerBehavior) {
        *lock(&self.shared.behavior) = behavior;
    }

    pub fn stall_gate(&self) -> StallGate {
        self.shared.stall.share()
    }

    /// Submit a command. Never blocks on work; may reject when queues are full.
    pub fn submit(&self, command: Command) -> SubmitOutcome {
        let mut st = lock(&self.shared.state);
        if st.shutdown_done || st.phase == SessionState::Closed {
            return SubmitOutcome::Rejected {
                error: sess_error(
                    ErrorCode::InvalidState,
                    "session is closed",
                    command.payload.name(),
                ),
            };
        }
        if let Err(error) = command.payload.validate(&st.limits) {
            let visible = error.without_source();
            // Bound the retained reject payload (never keep a 1MB original label).
            let bounded = bound_command_payload(command.payload, &st.limits);
            publish_rejected(
                &mut st,
                &self.shared.notifier,
                bounded,
                visible.without_source(),
            );
            return SubmitOutcome::Rejected { error: visible };
        }

        // Stop / Shutdown: independent bounded slots + cancel.
        if command.payload.is_stop() {
            return admit_stop(&mut st, &self.shared.notifier, &command.payload);
        }
        if command.payload.is_shutdown() {
            return admit_shutdown(&mut st, &self.shared.notifier);
        }

        // In-flight mutating probe blocks other mutating probes.
        if command.payload.is_mutating_probe() && st.active_command.is_some() {
            let error = sess_error(
                ErrorCode::CommandInFlight,
                "a mutating command is already in flight",
                command.payload.name(),
            );
            publish_rejected(
                &mut st,
                &self.shared.notifier,
                command.payload,
                error.without_source(),
            );
            return SubmitOutcome::Rejected { error };
        }

        if let Err(error) = validate_in_state(st.phase, &command.payload) {
            publish_rejected(
                &mut st,
                &self.shared.notifier,
                command.payload,
                error.without_source(),
            );
            return SubmitOutcome::Rejected { error };
        }

        let id = CommandId::new(st.command_ids.next());
        let payload = command.payload.clone();

        // Pause/Resume/Reset apply immediately (no work-queue slot).
        match &payload {
            CommandPayload::Pause => {
                st.history.note_accepted(id);
                publish_control(
                    &mut st,
                    &self.shared.notifier,
                    Some(id),
                    None,
                    Event::CommandAccepted {
                        command: command.payload.clone(),
                    },
                );
                if st.phase == SessionState::Running {
                    *lock(&st.pause) = true;
                    transition(
                        &mut st,
                        &self.shared.notifier,
                        SessionState::Paused,
                        "pause",
                    );
                }
                complete_control(
                    &mut st,
                    &self.shared.notifier,
                    id,
                    RunOutcome::Completed { step: 0 },
                );
                return SubmitOutcome::Accepted { id };
            }
            CommandPayload::Resume => {
                st.history.note_accepted(id);
                publish_control(
                    &mut st,
                    &self.shared.notifier,
                    Some(id),
                    None,
                    Event::CommandAccepted {
                        command: command.payload.clone(),
                    },
                );
                if st.phase == SessionState::Paused {
                    *lock(&st.pause) = false;
                    transition(
                        &mut st,
                        &self.shared.notifier,
                        SessionState::Running,
                        "resume",
                    );
                }
                complete_control(
                    &mut st,
                    &self.shared.notifier,
                    id,
                    RunOutcome::Completed { step: 0 },
                );
                return SubmitOutcome::Accepted { id };
            }
            CommandPayload::Reset => {
                st.history.note_accepted(id);
                publish_control(
                    &mut st,
                    &self.shared.notifier,
                    Some(id),
                    None,
                    Event::CommandAccepted {
                        command: command.payload.clone(),
                    },
                );
                // Frozen Reset clears the failed snapshot error before Idle.
                st.last_outcome = None;
                transition(&mut st, &self.shared.notifier, SessionState::Idle, "reset");
                complete_control(
                    &mut st,
                    &self.shared.notifier,
                    id,
                    RunOutcome::Completed { step: 0 },
                );
                return SubmitOutcome::Accepted { id };
            }
            _ => {}
        }

        // Atomic admission: reserve identity under the same lock as try_send so
        // the owner cannot finish (and clear active) before reservation, and a
        // concurrent submit cannot observe a Ready gap.
        match self.work_tx.try_send(AdmittedWork::Payload {
            id,
            payload: payload.clone(),
        }) {
            Ok(()) => {
                st.history.note_accepted(id);
                st.active_command = Some(id);
                st.active_payload = Some(payload);
                SubmitOutcome::Accepted { id }
            }
            Err(TrySendError::Full(_)) => {
                let error =
                    sess_error(ErrorCode::CommandInFlight, "command queue is full", "queue");
                let bounded = bound_command_payload(payload, &st.limits);
                publish_rejected(
                    &mut st,
                    &self.shared.notifier,
                    bounded,
                    error.without_source(),
                );
                SubmitOutcome::Rejected { error }
            }
            Err(TrySendError::Disconnected(_)) => SubmitOutcome::Rejected {
                error: sess_error(ErrorCode::InvalidState, "session owner stopped", "owner"),
            },
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.shared.state).snapshot()
    }

    /// Subscribe and snapshot under one lock so the live stream boundary
    /// matches the returned snapshot seq.
    pub fn subscribe(&self) -> Result<(Subscription, Snapshot), SubscribeError> {
        let mut st = lock(&self.shared.state);
        match self.shared.notifier.subscribe() {
            Ok(sub) => {
                let snap = st.snapshot();
                Ok((sub, snap))
            }
            Err(SubscribeError::SubscriberLimit) => {
                st.drops.subscribe_rejected = st.drops.subscribe_rejected.saturating_add(1);
                Err(SubscribeError::SubscriberLimit)
            }
            Err(other) => Err(other),
        }
    }

    /// Atomic subscribe + control replay from the snapshot cursor.
    ///
    /// Under one lock: register the subscriber, take the snapshot, then replay
    /// retained control events with `seq > snapshot.seq` is empty by
    /// construction; callers combine `replay` with the live `Subscription` so
    /// no retained control event after the cursor is lost to a publication race.
    pub fn subscribe_and_replay(
        &self,
    ) -> Result<(Subscription, Snapshot, ReplayOutcome), SubscribeHandoffError> {
        let mut st = lock(&self.shared.state);
        let sub = match self.shared.notifier.subscribe() {
            Ok(sub) => sub,
            Err(SubscribeError::SubscriberLimit) => {
                st.drops.subscribe_rejected = st.drops.subscribe_rejected.saturating_add(1);
                return Err(SubscribeHandoffError::Subscribe(
                    SubscribeError::SubscriberLimit,
                ));
            }
            Err(e) => return Err(SubscribeHandoffError::Subscribe(e)),
        };
        let snap = st.snapshot();
        let replay = st
            .history
            .replay_control_after(snap.cursor.seq, snap.clone());
        Ok((sub, snap, replay))
    }

    pub fn replay(&self, cursor: EventCursor) -> Result<ReplayOutcome, CursorError> {
        let st = lock(&self.shared.state);
        validate_cursor(&st, cursor)?;
        Ok(st.history.replay_control_after(cursor.seq, st.snapshot()))
    }

    pub fn command_result(&self, id: CommandId) -> CommandResult {
        let st = lock(&self.shared.state);
        if st.active_command == Some(id)
            || st.stop_owner == Some(id)
            || (st.shutdown_owner == Some(id) && !st.shutdown_done)
        {
            return CommandResult::Pending;
        }
        st.history.command_result(id, None)
    }

    /// Inject a worker event (tests: stale/late runs). Progress uses try_send;
    /// terminals use the result mailbox.
    ///
    /// Events are validated/bounded **before** enqueue: malformed progress or
    /// illegal terminals are dropped (`Ok`) without closing the active run;
    /// oversized labels/errors are shrunk so the mailbox never retains them.
    pub fn inject_worker_event(&self, event: WorkerEvent) -> Result<(), DomainError> {
        let limits = self.limits();
        let Some(event) = sanitize_worker_event(event, &limits) else {
            let mut st = lock(&self.shared.state);
            st.drops.stale_worker = st.drops.stale_worker.saturating_add(1);
            return Ok(());
        };
        let is_result = matches!(
            event,
            WorkerEvent::LoadDone { .. } | WorkerEvent::RunTerminal { .. }
        );
        let tx = if is_result {
            &self.result_inject_tx
        } else {
            &self.progress_inject_tx
        };
        match tx.try_send(event) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                let mut st = lock(&self.shared.state);
                st.drops.worker_mailbox = st.drops.worker_mailbox.saturating_add(1);
                Err(sess_error(
                    ErrorCode::CommandInFlight,
                    "worker event mailbox is full",
                    "worker_mailbox",
                ))
            }
            Err(TrySendError::Disconnected(_)) => Err(sess_error(
                ErrorCode::InvalidState,
                "session owner stopped",
                "owner",
            )),
        }
    }

    pub fn log_tail(&self) -> Vec<String> {
        self.shared
            .state
            .lock()
            .expect("state")
            .log_tail
            .entries
            .iter()
            .cloned()
            .collect()
    }

    pub fn wait_until<F>(&self, timeout: Duration, mut pred: F) -> bool
    where
        F: FnMut(&Snapshot) -> bool,
    {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if pred(&self.snapshot()) {
                return true;
            }
            thread::sleep(Duration::from_millis(2));
        }
        pred(&self.snapshot())
    }

    /// Disk diagnostics counters when a sink was configured; otherwise `None`.
    pub fn disk_log_counters(&self) -> Option<DiskLogCounters> {
        self.shared.disk_log.as_ref().map(|d| d.counters())
    }

    /// Flush the disk sink within `deadline` (no-op when unset).
    pub fn flush_disk_log(&self, deadline: Duration) -> bool {
        match &self.shared.disk_log {
            Some(d) => d.flush(deadline),
            None => true,
        }
    }
}

impl Drop for ApplicationService {
    fn drop(&mut self) {
        // Unstick paused/stalled workers, request shutdown cancel, then drain.
        self.shared.stall.set_stalled(false);
        {
            let st = lock(&self.shared.state);
            *lock(&st.pause) = false;
            let _ = st.cancel.request(CancelReason::Shutdown);
        }
        let _ = self.submit(Command::new(CommandPayload::Shutdown));
        let deadline = lock(&self.shared.state).limits.teardown_deadline;
        if let Some(handle) = self.owner.lock().expect("owner").take() {
            let start = Instant::now();
            while !handle.is_finished() {
                if start.elapsed() >= deadline {
                    let mut st = lock(&self.shared.state);
                    if !st.shutdown_done {
                        force_teardown_terminals(&mut st, &self.shared.notifier);
                    }
                    // Still join — never detach an immortal owner.
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            let _ = handle.join();
        } else {
            let mut st = lock(&self.shared.state);
            if !st.shutdown_done {
                force_teardown_terminals(&mut st, &self.shared.notifier);
            }
        }
        // Join the fake worker before Drop returns (no survivors).
        if let Ok(mut w) = self.shared.worker.lock() {
            w.shutdown_join();
        }
        // Disk sink flush/join under the same teardown deadline (never blocks
        // workers while running; only Drop waits).
        if let Some(disk) = &self.shared.disk_log {
            let _ = disk.flush(deadline);
            let _ = disk.join(deadline);
        }
    }
}

/// Errors from [`ApplicationService::subscribe_and_replay`].
#[derive(Debug)]
pub enum SubscribeHandoffError {
    Subscribe(SubscribeError),
}

fn validate_cursor(st: &State, cursor: EventCursor) -> Result<(), CursorError> {
    if cursor.session_id != st.session_id {
        return Err(CursorError::ForeignSession {
            expected: st.session_id,
            actual: cursor.session_id,
            error: sess_error(
                ErrorCode::InvalidState,
                "cursor session_id does not match this session",
                "cursor",
            ),
        });
    }
    if cursor.seq > st.seq {
        return Err(CursorError::Future {
            snapshot_seq: st.seq,
            cursor_seq: cursor.seq,
            error: DomainError::try_new(
                ErrorCode::ValueOutOfRange,
                "cursor seq is ahead of snapshot",
            )
            .expect("message")
            .with_context(ContextKey::Field, "cursor.seq")
            .expect("field")
            .with_context(ContextKey::Expected, format!("<= {}", st.seq))
            .expect("expected")
            .with_context(ContextKey::Actual, cursor.seq.to_string())
            .expect("actual"),
        });
    }
    Ok(())
}

fn admit_stop(st: &mut State, notifier: &Arc<Notifier>, payload: &CommandPayload) -> SubmitOutcome {
    let CommandPayload::Stop { save } = payload else {
        unreachable!("admit_stop");
    };
    let save = *save;

    // Idempotent: already have pending/owner stop while working or stopping.
    if st.stop_owner.is_some() || st.pending_stop.is_some() {
        let id = CommandId::new(st.command_ids.next());
        st.history.note_accepted(id);
        publish_control(
            st,
            notifier,
            Some(id),
            st.active_run,
            Event::CommandAccepted {
                command: payload.clone(),
            },
        );
        // Coalesced Stop: own terminal; must not overwrite the run outcome.
        complete_control(st, notifier, id, RunOutcome::Completed { step: 0 });
        return SubmitOutcome::Accepted { id };
    }

    if !matches!(
        st.phase,
        SessionState::Loading
            | SessionState::Preparing
            | SessionState::Running
            | SessionState::Paused
            | SessionState::Stopping
    ) && st.active_command.is_none()
    {
        let error = sess_error(
            ErrorCode::InvalidState,
            "stop is invalid in the current state",
            "stop",
        );
        publish_rejected(st, notifier, payload.clone(), error.without_source());
        return SubmitOutcome::Rejected { error };
    }

    let id = CommandId::new(st.command_ids.next());
    st.history.note_accepted(id);
    publish_control(
        st,
        notifier,
        Some(id),
        st.active_run,
        Event::CommandAccepted {
            command: payload.clone(),
        },
    );
    st.stop_owner = Some(id);
    st.pending_stop = Some(PendingStop { save });
    if let Some(flag) = &st.active_save_on_cancel {
        flag.store(save, Ordering::Release);
    }
    let _ = st.cancel.request(CancelReason::User);
    if st.phase == SessionState::Running || st.phase == SessionState::Paused {
        transition(st, notifier, SessionState::Stopping, "stop");
    }
    SubmitOutcome::Accepted { id }
}

fn admit_shutdown(st: &mut State, notifier: &Arc<Notifier>) -> SubmitOutcome {
    if let Some(existing) = st.shutdown_owner {
        let id = CommandId::new(st.command_ids.next());
        st.history.note_accepted(id);
        publish_control(
            st,
            notifier,
            Some(id),
            None,
            Event::CommandAccepted {
                command: CommandPayload::Shutdown,
            },
        );
        // Coalesced Shutdown: own Completed terminal; ShutdownComplete stays unique
        // to `shutdown_owner`.
        complete_control(st, notifier, id, RunOutcome::Completed { step: 0 });
        let _ = existing;
        return SubmitOutcome::Accepted { id };
    }

    let id = CommandId::new(st.command_ids.next());
    st.history.note_accepted(id);
    publish_control(
        st,
        notifier,
        Some(id),
        None,
        Event::CommandAccepted {
            command: CommandPayload::Shutdown,
        },
    );
    st.shutdown_owner = Some(id);
    st.pending_shutdown = Some(id);
    let _ = st.cancel.request(CancelReason::Shutdown);
    SubmitOutcome::Accepted { id }
}

fn validate_in_state(phase: SessionState, payload: &CommandPayload) -> Result<(), DomainError> {
    let ok = match payload {
        CommandPayload::ProbeLoad { .. } => {
            matches!(
                phase,
                SessionState::Idle | SessionState::Ready | SessionState::Failed
            )
        }
        CommandPayload::ProbeRun { .. } => phase == SessionState::Ready,
        CommandPayload::Pause => {
            matches!(phase, SessionState::Running | SessionState::Paused)
        }
        CommandPayload::Resume => {
            matches!(phase, SessionState::Paused | SessionState::Running)
        }
        CommandPayload::Reset => phase == SessionState::Failed,
        CommandPayload::Stop { .. } | CommandPayload::Shutdown => true,
    };
    if ok {
        Ok(())
    } else {
        Err(sess_error(
            ErrorCode::InvalidState,
            "command is invalid in the current state",
            payload.name(),
        ))
    }
}

fn owner_loop(
    shared: Arc<Shared>,
    work_rx: Receiver<AdmittedWork>,
    progress_rx: Receiver<WorkerEvent>,
    result_rx: Receiver<WorkerEvent>,
) {
    let poll = {
        let st = lock(&shared.state);
        st.limits.poll_interval()
    };
    loop {
        {
            let mut st = lock(&shared.state);
            if st.shutdown_done {
                break;
            }
            // Drive shutdown when idle (no active command).
            if st.pending_shutdown.is_some() && st.active_command.is_none() {
                finish_shutdown(&mut st, &shared.notifier);
                break;
            }
        }

        // Results first so a full progress mailbox cannot delay terminals.
        let mut drained_result = false;
        match result_rx.try_recv() {
            Ok(event) => {
                drained_result = true;
                apply_worker_event(&shared, event);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                let mut st = lock(&shared.state);
                if let Some(id) = st.active_command {
                    let err = DomainError::try_new(
                        ErrorCode::Internal,
                        "fake worker result channel disconnected",
                    )
                    .expect("message");
                    finish_active(
                        &mut st,
                        &shared.notifier,
                        id,
                        RunOutcome::Failed { error: err },
                        None,
                    );
                }
            }
        }

        match work_rx.try_recv() {
            Ok(AdmittedWork::Payload { id, payload }) => {
                start_work(&shared, id, payload);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {}
        }

        match progress_rx.try_recv() {
            Ok(event) => apply_worker_event(&shared, event),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {}
        }

        if !drained_result {
            thread::sleep(poll.min(Duration::from_millis(5)));
        }
    }
}

fn start_work(shared: &Shared, id: CommandId, payload: CommandPayload) {
    let mut st = lock(&shared.state);
    publish_control(
        &mut st,
        &shared.notifier,
        Some(id),
        None,
        Event::CommandAccepted {
            command: payload.clone(),
        },
    );
    if st.shutdown_done || st.phase == SessionState::Closed {
        finish_active(
            &mut st,
            &shared.notifier,
            id,
            RunOutcome::Canceled {
                step: 0,
                reason: CancelReason::Shutdown,
                saved_checkpoint: None,
            },
            None,
        );
        return;
    }
    // Fresh cancel token: do **not** inherit a prior run's reason. Only apply
    // cancel if Stop/Shutdown is still pending for this admission.
    st.cancel = CancelToken::new();
    if st.pending_shutdown.is_some() {
        let _ = st.cancel.request(CancelReason::Shutdown);
    } else if st.pending_stop.is_some() {
        let _ = st.cancel.request(CancelReason::User);
    }
    *lock(&st.pause) = false;
    let run_id = RunId::new(st.run_ids.next());
    st.active_command = Some(id);
    st.active_run = Some(run_id);
    st.active_payload = Some(payload.clone());
    st.latest_progress = None;
    let save_flag = Arc::new(AtomicBool::new(
        st.pending_stop.as_ref().map(|p| p.save).unwrap_or(true),
    ));
    st.active_save_on_cancel = Some(Arc::clone(&save_flag));

    let cancel = st.cancel.clone();
    let pause = Arc::clone(&st.pause);
    let stall = shared.stall.clone_arc();
    let limits_delay = st.limits.clone();
    let _ = limits_delay;

    match payload {
        CommandPayload::ProbeLoad { label } => {
            if check_label(&label, &st.limits).is_err() {
                finish_active(
                    &mut st,
                    &shared.notifier,
                    id,
                    RunOutcome::Failed {
                        error: sess_error(
                            ErrorCode::ValueOutOfRange,
                            "probe load label exceeds size limit",
                            "label",
                        ),
                    },
                    Some(run_id),
                );
                return;
            }
            transition(
                &mut st,
                &shared.notifier,
                SessionState::Loading,
                "probe_load",
            );
            drop(st);
            lock(&shared.worker).send(WorkerRequest::Load {
                run_id,
                command_id: id,
                label,
                cancel,
            });
        }
        CommandPayload::ProbeRun { steps } => {
            transition(
                &mut st,
                &shared.notifier,
                SessionState::Preparing,
                "probe_run",
            );
            publish_control(
                &mut st,
                &shared.notifier,
                Some(id),
                Some(run_id),
                Event::ProbeStarted { steps },
            );
            transition(
                &mut st,
                &shared.notifier,
                SessionState::Running,
                "probe_prepared",
            );
            drop(st);
            lock(&shared.worker).send(WorkerRequest::Run {
                run_id,
                command_id: id,
                steps,
                cancel,
                pause,
                stall,
                save_on_cancel: save_flag,
            });
        }
        other => {
            finish_active(
                &mut st,
                &shared.notifier,
                id,
                RunOutcome::Failed {
                    error: sess_error(
                        ErrorCode::Internal,
                        "unexpected queued payload",
                        other.name(),
                    ),
                },
                None,
            );
        }
    }
}

fn apply_worker_event(shared: &Shared, event: WorkerEvent) {
    let mut st = lock(&shared.state);
    let (run_id, command_id) = match &event {
        WorkerEvent::Progress {
            run_id, command_id, ..
        }
        | WorkerEvent::Log {
            run_id, command_id, ..
        }
        | WorkerEvent::LoadDone {
            run_id, command_id, ..
        }
        | WorkerEvent::RunTerminal {
            run_id, command_id, ..
        } => (*run_id, *command_id),
    };

    if st.active_run != Some(run_id) || st.active_command != Some(command_id) {
        st.drops.stale_worker = st.drops.stale_worker.saturating_add(1);
        return;
    }

    match event {
        WorkerEvent::Progress { snapshot, .. } => {
            if check_progress(&snapshot, &st.limits).is_err() {
                st.drops.progress = st.drops.progress.saturating_add(1);
                return;
            }
            // Latest-wins replaceable; not control history. Seq still advances
            // for live subscribers; intentional gaps vs control replay.
            if st.latest_progress.is_some() {
                st.drops.progress = st.drops.progress.saturating_add(1);
            }
            st.latest_progress = Some(snapshot.clone());
            publish_replaceable(
                &mut st,
                &shared.notifier,
                Some(command_id),
                Some(run_id),
                Event::Progress(snapshot),
            );
        }
        WorkerEvent::Log { message, .. } => {
            if check_log_message(&message, &st.limits).is_err() {
                st.drops.log = st.drops.log.saturating_add(1);
                return;
            }
            st.log_tail.push(message.clone());
            publish_replaceable(
                &mut st,
                &shared.notifier,
                Some(command_id),
                Some(run_id),
                Event::Log {
                    level: LogLevel::Info,
                    message,
                },
            );
        }
        WorkerEvent::LoadDone { label, .. } => {
            // Kind vs active payload: LoadDone cannot complete ProbeRun even
            // with matching run/command ids.
            let is_load = matches!(st.active_payload, Some(CommandPayload::ProbeLoad { .. }));
            if !is_load || st.phase != SessionState::Loading {
                st.drops.stale_worker = st.drops.stale_worker.saturating_add(1);
                return;
            }
            let label = bound_retained_label(label, &st.limits);
            st.loaded_label = Some(label.clone());
            publish_control(
                &mut st,
                &shared.notifier,
                Some(command_id),
                Some(run_id),
                Event::ProbeLoaded { label },
            );
            transition(&mut st, &shared.notifier, SessionState::Ready, "load_done");
            finish_active(
                &mut st,
                &shared.notifier,
                command_id,
                RunOutcome::Completed { step: 0 },
                Some(run_id),
            );
            complete_pending_stop(&mut st, &shared.notifier);
        }
        WorkerEvent::RunTerminal {
            outcome,
            checkpoint_label,
            save_failed,
            final_progress,
            ..
        } => {
            // Defense in depth: contradictory / malformed bundles stay Pending.
            if validate_worker_terminal(&outcome).is_err()
                || validate_run_terminal_bundle(
                    &outcome,
                    save_failed,
                    final_progress.as_ref(),
                    &st.limits,
                )
                .is_err()
            {
                st.drops.stale_worker = st.drops.stale_worker.saturating_add(1);
                return;
            }
            let _ = checkpoint_label; // canonical label lives on outcome after sanitize
            // `save_failed` only pairs with Canceled; never hide Failed → Ready.
            let save_failed =
                save_failed && matches!(outcome, spirula_core::TerminalOutcome::Canceled { .. });
            // Canonical ordering: final progress before terminal (separate mailboxes).
            // Present final_progress was validated; never strip-and-close.
            if let Some(snapshot) = final_progress {
                st.latest_progress.replace(snapshot.clone());
                publish_replaceable(
                    &mut st,
                    &shared.notifier,
                    Some(command_id),
                    Some(run_id),
                    Event::Progress(snapshot),
                );
            }
            let mut outcome = outcome_from_terminal(outcome);
            if let RunOutcome::Failed { error } = &outcome {
                outcome = RunOutcome::Failed {
                    error: bound_error(error, &st.limits),
                };
            }
            if let RunOutcome::Canceled {
                step,
                reason,
                saved_checkpoint,
            } = &outcome
            {
                outcome = RunOutcome::Canceled {
                    step: *step,
                    reason: *reason,
                    saved_checkpoint: saved_checkpoint
                        .clone()
                        .map(|label| bound_retained_label(label, &st.limits)),
                };
            }
            // §5 save-on-cancel failure: only with Canceled.
            // Successful Stop{save}: Stopping → Saving → checkpoint → Ready.
            if save_failed {
                let RunOutcome::Canceled { step, reason, .. } = &outcome else {
                    st.drops.stale_worker = st.drops.stale_worker.saturating_add(1);
                    return;
                };
                outcome = RunOutcome::Canceled {
                    step: *step,
                    reason: *reason,
                    saved_checkpoint: None,
                };
                let warn = DomainError::try_new(
                    ErrorCode::WriteFailed,
                    "save-on-cancel failed; previous checkpoint remains intact",
                )
                .expect("message");
                let warn = bound_error(&warn, &st.limits);
                publish_control(
                    &mut st,
                    &shared.notifier,
                    Some(command_id),
                    Some(run_id),
                    Event::Warning { error: warn },
                );
                if st.phase == SessionState::Stopping {
                    transition(
                        &mut st,
                        &shared.notifier,
                        SessionState::Ready,
                        "cancel_save_failed",
                    );
                } else if st.phase == SessionState::Loading {
                    transition(
                        &mut st,
                        &shared.notifier,
                        SessionState::Idle,
                        "run_terminal",
                    );
                } else {
                    transition(
                        &mut st,
                        &shared.notifier,
                        SessionState::Ready,
                        "run_terminal",
                    );
                }
            } else {
                // CheckpointSaved label must match terminal saved_checkpoint.
                let checkpoint = match &outcome {
                    RunOutcome::Canceled {
                        step,
                        saved_checkpoint: Some(label),
                        ..
                    } => Some((*step, label.clone())),
                    _ => None,
                };
                if let Some((step, label)) = checkpoint {
                    if st.phase == SessionState::Stopping {
                        transition(&mut st, &shared.notifier, SessionState::Saving, "stop_save");
                    }
                    publish_control(
                        &mut st,
                        &shared.notifier,
                        Some(command_id),
                        Some(run_id),
                        Event::CheckpointSaved { step, label },
                    );
                    transition(&mut st, &shared.notifier, SessionState::Ready, "saved");
                } else {
                    let next = match &outcome {
                        RunOutcome::Failed { .. } => SessionState::Failed,
                        RunOutcome::Canceled { .. } | RunOutcome::Completed { .. } => {
                            if st.phase == SessionState::Loading {
                                SessionState::Idle
                            } else {
                                SessionState::Ready
                            }
                        }
                    };
                    transition(&mut st, &shared.notifier, next, "run_terminal");
                }
            }
            finish_active(&mut st, &shared.notifier, command_id, outcome, Some(run_id));
            complete_pending_stop(&mut st, &shared.notifier);
        }
    }
}

fn complete_pending_stop(st: &mut State, notifier: &Arc<Notifier>) {
    if let Some(stop_id) = st.stop_owner.take() {
        complete_control(st, notifier, stop_id, RunOutcome::Completed { step: 0 });
    }
    st.pending_stop = None;
    st.active_save_on_cancel = None;
}

fn finish_active(
    st: &mut State,
    notifier: &Arc<Notifier>,
    command_id: CommandId,
    outcome: RunOutcome,
    run_id: Option<RunId>,
) {
    st.last_outcome = Some(outcome.clone());
    let rid = run_id.or(st.active_run.take());
    if st.shutdown_done {
        // No canonical events after ShutdownComplete; still retain the result.
        let envelope = EventEnvelope {
            event_version: 1,
            session_id: st.session_id,
            run_id: rid,
            seq: st.seq,
            command_id: Some(command_id),
            monotonic_ms: st.monotonic_ms(),
            event: Event::Completed { outcome },
        };
        st.history.store_result(command_id, envelope);
    } else {
        publish_control(
            st,
            notifier,
            Some(command_id),
            rid,
            Event::Completed { outcome },
        );
    }
    st.active_command = None;
    st.active_run = None;
    st.active_payload = None;
    st.latest_progress = None;
    st.active_save_on_cancel = None;
    *lock(&st.pause) = false;
}

/// Terminal for Pause/Resume/Stop/Shutdown no-ops. Does **not** update
/// `last_outcome` (that tracks the active run / probe only).
fn complete_control(
    st: &mut State,
    notifier: &Arc<Notifier>,
    command_id: CommandId,
    outcome: RunOutcome,
) {
    publish_control(
        st,
        notifier,
        Some(command_id),
        None,
        Event::Completed { outcome },
    );
}

fn force_teardown_terminals(st: &mut State, notifier: &Arc<Notifier>) {
    if let Some(id) = st.active_command.take() {
        let outcome = RunOutcome::Canceled {
            step: 0,
            reason: CancelReason::Shutdown,
            saved_checkpoint: None,
        };
        st.last_outcome = Some(outcome.clone());
        let run_id = st.active_run.take();
        publish_control(st, notifier, Some(id), run_id, Event::Completed { outcome });
        st.active_payload = None;
        st.latest_progress = None;
    }
    if let Some(stop_id) = st.stop_owner.take() {
        complete_control(
            st,
            notifier,
            stop_id,
            RunOutcome::Canceled {
                step: 0,
                reason: CancelReason::Shutdown,
                saved_checkpoint: None,
            },
        );
    }
    st.pending_stop = None;
    if !st.shutdown_done {
        if st.shutdown_owner.is_none() {
            st.shutdown_owner = Some(CommandId::new(st.command_ids.next()));
        }
        finish_shutdown(st, notifier);
    }
}

fn finish_shutdown(st: &mut State, notifier: &Arc<Notifier>) {
    let id = st
        .pending_shutdown
        .take()
        .or(st.shutdown_owner)
        .unwrap_or_else(|| CommandId::new(st.command_ids.next()));
    transition(st, notifier, SessionState::Closed, "shutdown");
    publish_control(st, notifier, Some(id), None, Event::ShutdownComplete);
    st.shutdown_done = true;
    notifier.close();
}

fn transition(st: &mut State, notifier: &Arc<Notifier>, to: SessionState, reason: &str) {
    if st.phase == to {
        return;
    }
    let reason = if check_reason(reason, &st.limits).is_ok() {
        reason.to_string()
    } else {
        "transition".to_string()
    };
    let from = st.phase;
    st.phase = to;
    publish_control(
        st,
        notifier,
        st.active_command,
        st.active_run,
        Event::StateChanged { from, to, reason },
    );
}

fn publish_rejected(
    st: &mut State,
    notifier: &Arc<Notifier>,
    command: CommandPayload,
    error: DomainError,
) {
    let error = bound_error(&error, &st.limits);
    publish_control(
        st,
        notifier,
        None,
        None,
        Event::CommandRejected { command, error },
    );
}

fn publish_control(
    st: &mut State,
    notifier: &Arc<Notifier>,
    command_id: Option<CommandId>,
    run_id: Option<RunId>,
    event: Event,
) {
    // ShutdownComplete is the last canonical event; nothing after.
    if st.shutdown_done {
        return;
    }
    let is_shutdown_complete = matches!(event, Event::ShutdownComplete);
    mirror_disk_log(st, &event);
    let seq = st.next_seq();
    let envelope = EventEnvelope {
        event_version: 1,
        session_id: st.session_id,
        run_id,
        seq,
        command_id,
        monotonic_ms: st.monotonic_ms(),
        event,
    };
    st.history.push_control(envelope.clone(), &mut st.drops);
    notifier.publish(&envelope, &mut st.drops);
    // Never publish after ShutdownComplete.
    if !is_shutdown_complete {
        emit_disk_failure_warning_once(st, notifier);
    }
}

fn publish_replaceable(
    st: &mut State,
    notifier: &Arc<Notifier>,
    command_id: Option<CommandId>,
    run_id: Option<RunId>,
    event: Event,
) {
    if st.shutdown_done {
        return;
    }
    mirror_disk_log(st, &event);
    let seq = st.next_seq();
    let envelope = EventEnvelope {
        event_version: 1,
        session_id: st.session_id,
        run_id,
        seq,
        command_id,
        monotonic_ms: st.monotonic_ms(),
        event,
    };
    // Not retained in control history (intentional gaps on replay).
    notifier.publish(&envelope, &mut st.drops);
    emit_disk_failure_warning_once(st, notifier);
}

/// Mirror diagnostic text to the async disk sink. Never blocks; never
/// re-enters the sink when recording a disk-failure warning.
fn mirror_disk_log(st: &State, event: &Event) {
    let Some(disk) = st.disk_log.as_ref() else {
        return;
    };
    match event {
        Event::Log { level, message } => {
            let tag = match level {
                LogLevel::Trace => "TRACE",
                LogLevel::Debug => "DEBUG",
                LogLevel::Info => "INFO",
                LogLevel::Warn => "WARN",
                LogLevel::Error => "ERROR",
            };
            let _ = disk.try_enqueue_line(format!("{tag} {message}"));
        }
        Event::Warning { error } => {
            // Skip disk mirror for the dedicated disk-failure warning code path
            // by using a distinct marker in context — see emit helper.
            if error.context_value(ContextKey::Field) == Some("disk_log") {
                return;
            }
            let _ = disk.try_enqueue_line(format!("WARN {error}"));
        }
        Event::StateChanged { from, to, reason } => {
            let _ = disk.try_enqueue_line(format!(
                "STATE {} -> {} ({reason})",
                from.as_str(),
                to.as_str()
            ));
        }
        Event::Completed { outcome } => {
            let _ = disk.try_enqueue_line(format!("COMPLETED {outcome:?}"));
        }
        Event::ShutdownComplete => {
            let _ = disk.try_enqueue_line("SHUTDOWN");
        }
        _ => {}
    }
}

fn emit_disk_failure_warning_once(st: &mut State, notifier: &Arc<Notifier>) {
    let Some(disk) = st.disk_log.clone() else {
        return;
    };
    if !disk.take_failure_notice() {
        return;
    }
    if st.shutdown_done {
        return;
    }
    let error = DomainError::try_new(ErrorCode::WriteFailed, "disk diagnostics write failed")
        .expect("message")
        .with_context(ContextKey::Field, "disk_log")
        .expect("field");
    // Publish in-memory only: mirror_disk_log skips field=disk_log warnings.
    let seq = st.next_seq();
    let envelope = EventEnvelope {
        event_version: 1,
        session_id: st.session_id,
        run_id: None,
        seq,
        command_id: None,
        monotonic_ms: st.monotonic_ms(),
        event: Event::Warning { error },
    };
    st.history.push_control(envelope.clone(), &mut st.drops);
    notifier.publish(&envelope, &mut st.drops);
}

fn sess_error(code: ErrorCode, message: &str, command: &str) -> DomainError {
    DomainError::try_new(code, message)
        .expect("message")
        .with_context(ContextKey::Command, command)
        .expect("command")
}
