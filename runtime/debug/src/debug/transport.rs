use super::identities::RequestIssuer;
use super::*;
use crate::engine::{FailureCode, FailureReport, Rejected, RejectionReason, SourceDigest};
use std::{
    collections::VecDeque,
    mem::size_of,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, RwLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

/// Options for one native debugger session; startup pausing is explicit.
#[derive(Clone, Copy, Debug, Default)]
pub struct DebugSessionOptions {
    /// Pause at the first native application entry before executing its code.
    pub break_on_start: bool,
}
/// Result of accepted normal-command publication; wake failure never returns ownership.
#[derive(Clone, Debug)]
pub enum DebugRequestReceipt {
    /// The command and terminal reply reservation were accepted.
    Accepted {
        /// Stable request identity used by its terminal reply.
        id: DebugRequestId,
    },
    /// Publication was accepted, but fatal signaling failed; do not retry.
    AcceptedWakeFailed {
        /// Already accepted request identity.
        id: DebugRequestId,
        /// Detached signaling failure.
        error: DebugError,
    },
}
/// Admission result for an urgent control that consumes no normal reply permit.
#[derive(Clone, Debug)]
pub enum DebugControlReceipt {
    /// A new control decision was accepted, not necessarily completed.
    Accepted,
    /// An equivalent coalesced control is already pending or satisfied.
    AlreadyRequested,
    /// Control publication succeeded but signaling failed; do not retry as new work.
    AcceptedWakeFailed {
        /// Detached signaling failure after accepted publication.
        error: DebugError,
    },
}
/// Non-cloneable transferable controller for one debugger session.
///
/// It owns bounded metadata and a revocable native signal, never a native value or
/// engine. Only one protocol consumer should drain updates. Drop requests urgent
/// detach; old controllers cannot reach a replacement session.
pub struct VmDebugHandle {
    pub(super) session: Arc<Session>,
}
impl VmDebugHandle {
    /// Borrows immutable negotiated native debugger/build capabilities.
    pub fn descriptor(&self) -> &DebugDescriptor {
        &self.session.descriptor
    }
    /// Reads the authoritative published state; a pause request alone does not pause.
    pub fn state(&self) -> DebugExecutionState {
        self.session.lock().execution
    }
    /// Admits a bounded command only after reserving its terminal reply allowance.
    ///
    /// Rejection returns the original owned request. Accepted wake failure carries
    /// the accepted identity and must not be retried as another command.
    #[allow(
        clippy::result_large_err,
        reason = "Rejected debugger commands return their original owned input without adding an allocation."
    )]
    pub fn try_request(
        &self,
        request: DebugRequest,
    ) -> Result<DebugRequestReceipt, Rejected<DebugRequest>> {
        self.session.request(request)
    }
    /// Coalesces a native safe-point pause request independently of normal capacity.
    pub fn request_pause(&self) -> Result<DebugControlReceipt, DebugError> {
        self.session.pause()
    }
    /// Admits the first resume/step decision for the current pause.
    ///
    /// Competing decisions return `InFlight`; stale pauses fail before native access.
    /// An in-flight evaluator blocks continue, but not detach or stop.
    pub fn continue_execution(
        &self,
        pause: PauseId,
        action: DebugContinue,
    ) -> Result<DebugControlReceipt, DebugError> {
        self.session.continue_execution(pause, action)
    }
    /// Requests cancellation of exactly the currently dispatched evaluator.
    ///
    /// Queued, finished, and non-evaluator request identities are rejected. This
    /// urgent control consumes no request/reply capacity and does not stop the
    /// application. Acceptance records intent; the native owner must unwind the
    /// evaluator and publish its one terminal reply. Requires `CancelEvaluation`.
    pub fn cancel_evaluation(&self, id: DebugRequestId) -> Result<DebugControlReceipt, DebugError> {
        self.session.cancel_evaluation(id)
    }
    /// Requests observer/breakpoint removal and default resume, even under saturation.
    pub fn detach(&self) -> Result<DebugControlReceipt, DebugError> {
        self.session.detach()
    }
    /// Consumes a reply/update without engine entry; reading a reply releases its permit.
    pub fn try_recv(&self) -> Option<DebugUpdate> {
        self.session.recv(false)
    }
    /// Waits for an update on host-provided concurrency, never on an application task.
    ///
    /// Returns `None` after the session is closed and all terminal records are read.
    pub fn wait_update(&self) -> Option<DebugUpdate> {
        self.session.recv(true)
    }
    pub(super) fn limits(&self) -> &DebugLimits {
        &self.session.limits
    }
}
impl Drop for VmDebugHandle {
    fn drop(&mut self) {
        let _ = self.session.detach();
    }
}

pub(super) struct SignalLease {
    signal: RwLock<Option<Arc<dyn DebugSignal>>>,
}
impl SignalLease {
    fn new() -> Self {
        Self {
            signal: RwLock::new(None),
        }
    }
    pub fn install(&self, signal: Arc<dyn DebugSignal>) {
        *self.signal.write().unwrap_or_else(|e| e.into_inner()) = Some(signal);
    }
    fn wake(&self) -> Result<(), DebugError> {
        // This gate is separate from transport state. Revocation waits for short
        // entered signal calls; no transport mutex is held across native signaling.
        let guard = self.signal.read().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map_or(Ok(()), |signal| signal.wake())
    }
    pub fn revoke(&self) {
        let signal = self
            .signal
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        drop(signal);
    }
}
pub(super) struct ContextControl {
    pub stopping: AtomicBool,
    pub active: Mutex<Weak<Session>>,
    wake_owner: Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync>,
}
impl ContextControl {
    pub fn new(wake_owner: Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            stopping: AtomicBool::new(false),
            active: Mutex::new(Weak::new()),
            wake_owner,
        })
    }
    pub fn stop(&self) -> Result<DebugControlReceipt, DebugError> {
        let already = self.stopping.swap(true, Ordering::AcqRel);
        let session = self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upgrade();
        if let Some(session) = session {
            return session.stop();
        }
        match (self.wake_owner)() {
            Ok(()) => Ok(if already {
                DebugControlReceipt::AlreadyRequested
            } else {
                DebugControlReceipt::Accepted
            }),
            Err(error) => Ok(DebugControlReceipt::AcceptedWakeFailed {
                error: DebugError::WakeFailed(error.bounded()),
            }),
        }
    }
}
struct RequestRecord {
    id: DebugRequestId,
    pause: Option<PauseId>,
    request: Option<DebugRequest>,
    result: Option<Result<DebugReply, DebugError>>,
    evaluation: bool,
    breakpoint_budget: Option<usize>,
}
struct BreakpointBudget {
    origin: String,
    digest: Option<SourceDigest>,
    count: usize,
    bytes: usize,
    pending: Option<(DebugRequestId, usize, usize)>,
}
pub(super) struct State {
    pub execution: DebugExecutionState,
    historical_pause: bool,
    closed: bool,
    entry_gate: bool,
    slots: Vec<Option<RequestRecord>>,
    breakpoint_budgets: Vec<Option<BreakpointBudget>>,
    queued: VecDeque<usize>,
    replies: VecDeque<usize>,
    updates: VecDeque<DebugUpdate>,
    latest: Option<DebugUpdate>,
    closure: Option<DebugUpdate>,
    diagnostic: Option<DebugUpdate>,
    last_sequence: u64,
    pause_requested: bool,
    continuing: Option<(PauseId, DebugContinue, bool)>,
    detach_requested: bool,
    detach_delivered: bool,
    stop_requested: bool,
    stop_delivered: bool,
    evaluation: Option<DebugRequestId>,
    evaluation_control: Option<EvaluationControl>,
    pub fault: Option<DebugError>,
}
pub(super) struct Session {
    pub id: DebugSessionId,
    pub break_on_start: bool,
    pub descriptor: DebugDescriptor,
    pub limits: DebugLimits,
    issuer: RequestIssuer,
    state: Mutex<State>,
    // Published/cleared only while holding state. Acquire readers may skip the
    // mutex when false; the scheduler rechecks pending after arming its poller.
    pending_hint: AtomicBool,
    entry_hint: AtomicBool,
    changed: Condvar,
    pub signal: SignalLease,
    pub control: Arc<ContextControl>,
}
fn reserve<T>(capacity: usize) -> Result<Vec<T>, DebugError> {
    let mut v = Vec::new();
    v.try_reserve_exact(capacity)
        .map_err(|_| DebugError::Capacity)?;
    Ok(v)
}
fn deque<T>(capacity: usize) -> Result<VecDeque<T>, DebugError> {
    let mut v = VecDeque::new();
    v.try_reserve_exact(capacity)
        .map_err(|_| DebugError::Capacity)?;
    Ok(v)
}
impl Session {
    pub fn new(
        id: DebugSessionId,
        break_on_start: bool,
        issuer: RequestIssuer,
        descriptor: DebugDescriptor,
        limits: DebugLimits,
        control: Arc<ContextControl>,
    ) -> Result<Arc<Self>, DebugError> {
        if limits
            .inspection_objects
            .checked_add(limits.write_targets)
            .is_none()
        {
            return Err(DebugError::Capacity);
        }
        if limits.requests == 0
            || limits.request_bytes == 0
            || limits.reply_bytes == 0
            || limits.write_targets == 0
            || limits.inspection_objects == 0
            || limits.expression_units == 0
        {
            return Err(DebugError::Capacity);
        }
        let mut slots = reserve(limits.requests)?;
        slots.resize_with(limits.requests, || None);
        let budget_capacity = limits
            .breakpoints
            .checked_add(limits.requests)
            .ok_or(DebugError::Capacity)?;
        let mut breakpoint_budgets = reserve(budget_capacity)?;
        breakpoint_budgets.resize_with(budget_capacity, || None);
        let queued = deque(limits.requests)?;
        let replies = deque(limits.requests)?;
        let updates = deque(limits.updates)?;
        Ok(Arc::new(Self {
            id,
            break_on_start,
            descriptor,
            limits,
            issuer,
            state: Mutex::new(State {
                execution: DebugExecutionState::Running,
                historical_pause: false,
                closed: false,
                entry_gate: false,
                slots,
                breakpoint_budgets,
                queued,
                replies,
                updates,
                latest: None,
                closure: None,
                diagnostic: None,
                last_sequence: 0,
                pause_requested: false,
                continuing: None,
                detach_requested: false,
                detach_delivered: false,
                stop_requested: false,
                stop_delivered: false,
                evaluation: None,
                evaluation_control: None,
                fault: None,
            }),
            pending_hint: AtomicBool::new(false),
            entry_hint: AtomicBool::new(false),
            changed: Condvar::new(),
            signal: SignalLease::new(),
            control,
        }))
    }
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn refresh_hints(&self, state: &State) {
        // Both directions are published under the authoritative state mutex.
        // A producer that races with a clear must acquire that same mutex and
        // republishes true before notifying either native or poller wait paths.
        self.entry_hint
            .store(state.entry_gate && !state.closed, Ordering::Release);
        let pending = !state.closed
            && (!state.queued.is_empty()
                || state.pause_requested
                || state.continuing.is_some_and(|control| !control.2)
                || (state.detach_requested && !state.detach_delivered)
                || ((state.stop_requested || self.control.stopping.load(Ordering::Acquire))
                    && !state.stop_delivered));
        self.pending_hint.store(pending, Ordering::Release);
    }
    fn open(&self, state: &State) -> Result<(), DebugError> {
        if state.closed
            || state.detach_requested
            || state.stop_requested
            || self.control.stopping.load(Ordering::Acquire)
        {
            Err(DebugError::Closed)
        } else {
            Ok(())
        }
    }
    fn notify(&self) -> Result<(), DebugError> {
        self.changed.notify_all();
        let owner = (self.control.wake_owner)().map_err(|e| DebugError::WakeFailed(e.bounded()));
        let native = self.signal.wake();
        let error = owner.err().or_else(|| native.err());
        if let Some(error) = error {
            let mut state = self.lock();
            state.fault = Some(error.clone());
            state.stop_requested = true;
            self.control.stopping.store(true, Ordering::Release);
            self.refresh_hints(&state);
            drop(state);
            self.changed.notify_all();
            return Err(error);
        }
        Ok(())
    }
    fn control_receipt(&self, already: bool) -> Result<DebugControlReceipt, DebugError> {
        match self.notify() {
            Ok(()) => Ok(if already {
                DebugControlReceipt::AlreadyRequested
            } else {
                DebugControlReceipt::Accepted
            }),
            Err(error) => Ok(DebugControlReceipt::AcceptedWakeFailed { error }),
        }
    }
    #[allow(
        clippy::result_large_err,
        reason = "Rejected debugger commands return their original owned input without adding an allocation."
    )]
    fn request(
        &self,
        request: DebugRequest,
    ) -> Result<DebugRequestReceipt, Rejected<DebugRequest>> {
        let measured = request_footprint(&request, &self.limits, &self.descriptor);
        let mut state = self.lock();
        let admission = self
            .open(&state)
            .and(measured)
            .and_then(|_| validate_request_pause(&request, &state))
            .and_then(|_| {
                state
                    .slots
                    .iter()
                    .position(Option::is_none)
                    .ok_or(DebugError::Capacity)
            });
        let slot = match admission {
            Ok(slot) => slot,
            Err(reason) => {
                return Err(Rejected {
                    value: request,
                    reason: RejectionReason::Debug(reason),
                });
            }
        };
        let budget = match breakpoint_budget(&request, &state, &self.limits) {
            Ok(budget) => budget,
            Err(reason) => {
                return Err(Rejected {
                    value: request,
                    reason: RejectionReason::Debug(reason),
                });
            }
        };
        let id = match self.issuer.issue() {
            Ok(id) => id,
            Err(reason) => {
                return Err(Rejected {
                    value: request,
                    reason: RejectionReason::Debug(reason),
                });
            }
        };
        if let (Some((index, count, bytes)), DebugRequest::SetBreakpoints { selector, .. }) =
            (budget, &request)
        {
            if state.breakpoint_budgets[index].is_none() {
                state.breakpoint_budgets[index] = Some(BreakpointBudget {
                    origin: selector.origin.clone(),
                    digest: selector.expected_digest,
                    count: 0,
                    bytes: 0,
                    pending: None,
                });
            }
            state.breakpoint_budgets[index].as_mut().unwrap().pending = Some((id, count, bytes));
        }
        state.slots[slot] = Some(RequestRecord {
            id,
            pause: request_pause(&request),
            evaluation: matches!(
                request,
                DebugRequest::Evaluate { .. }
                    | DebugRequest::EvaluateWithOptions { .. }
                    | DebugRequest::Write { .. }
            ),
            request: Some(request),
            result: None,
            breakpoint_budget: budget.map(|budget| budget.0),
        });
        state.queued.push_back(slot);
        self.refresh_hints(&state);
        drop(state);
        Ok(match self.notify() {
            Ok(()) => DebugRequestReceipt::Accepted { id },
            Err(error) => DebugRequestReceipt::AcceptedWakeFailed { id, error },
        })
    }
    fn pause(&self) -> Result<DebugControlReceipt, DebugError> {
        let mut state = self.lock();
        self.open(&state)?;
        let already =
            state.pause_requested || matches!(state.execution, DebugExecutionState::Paused(_));
        state.pause_requested = !matches!(state.execution, DebugExecutionState::Paused(_));
        self.refresh_hints(&state);
        drop(state);
        self.control_receipt(already)
    }
    fn continue_execution(
        &self,
        pause: PauseId,
        action: DebugContinue,
    ) -> Result<DebugControlReceipt, DebugError> {
        let mut state = self.lock();
        self.open(&state)?;
        if state.execution != DebugExecutionState::Paused(pause) {
            return Err(DebugError::StalePause);
        }
        if state.historical_pause && action != DebugContinue::Resume {
            return Err(DebugError::Unsupported);
        }
        if state.evaluation.is_some() || state.continuing.is_some() {
            return Err(DebugError::InFlight);
        }
        state.continuing = Some((pause, action, false));
        self.refresh_hints(&state);
        drop(state);
        self.control_receipt(false)
    }
    fn cancel_evaluation(&self, id: DebugRequestId) -> Result<DebugControlReceipt, DebugError> {
        let state = self.lock();
        self.open(&state)?;
        if !self
            .descriptor
            .capabilities
            .contains(DebugCapability::CancelEvaluation)
        {
            return Err(DebugError::Unsupported);
        }
        if id.session != self.id {
            return Err(DebugError::StaleSession);
        }
        if state.evaluation != Some(id) {
            return Err(DebugError::InvalidRequest);
        }
        let control = state
            .evaluation_control
            .as_ref()
            .ok_or(DebugError::InvalidRequest)?;
        let already = !control.cancel();
        if already
            && control.poll_at(std::time::Instant::now()) != Some(EvaluationStatus::Cancelled)
        {
            return Err(DebugError::InvalidRequest);
        }
        drop(state);
        // As for all urgent controls, signaling runs without the transport lock.
        self.control_receipt(already)
    }
    /// Native-owner integration seam. Only the exact dispatched evaluator can
    /// obtain a token; clones are revoked with its terminal transport lifecycle.
    #[allow(
        dead_code,
        reason = "Native evaluator integration is a later Task 4 slice."
    )]
    pub(crate) fn evaluation_control(&self, id: DebugRequestId) -> Option<EvaluationControl> {
        let state = self.lock();
        (state.evaluation == Some(id))
            .then(|| state.evaluation_control.clone())
            .flatten()
    }
    fn detach(&self) -> Result<DebugControlReceipt, DebugError> {
        let mut state = self.lock();
        if state.closed {
            return Ok(DebugControlReceipt::AlreadyRequested);
        }
        let already = state.detach_requested;
        state.detach_requested = true;
        if state.evaluation.is_some()
            && !self
                .descriptor
                .capabilities
                .contains(DebugCapability::CancelEvaluation)
        {
            state.stop_requested = true;
            self.control.stopping.store(true, Ordering::Release);
            state.fault = Some(DebugError::EngineFailure(FailureReport::fault(
                FailureCode::EngineFailure,
                "debug evaluation cannot be safely restored after disconnect",
            )));
        }
        if let Some(control) = &state.evaluation_control {
            control.cancel();
        }
        self.refresh_hints(&state);
        drop(state);
        self.control_receipt(already)
    }
    pub fn stop(&self) -> Result<DebugControlReceipt, DebugError> {
        let mut state = self.lock();
        let already = state.stop_requested;
        state.stop_requested = true;
        self.control.stopping.store(true, Ordering::Release);
        self.refresh_hints(&state);
        drop(state);
        self.control_receipt(already)
    }
    pub fn claim_entry_gate(&self) -> Result<(), DebugError> {
        let mut state = self.lock();
        self.open(&state)?;
        if state.entry_gate {
            return Err(DebugError::InFlight);
        }
        state.entry_gate = true;
        self.refresh_hints(&state);
        Ok(())
    }
    pub fn release_entry_gate(&self) -> Result<(), DebugError> {
        let mut state = self.lock();
        if state.closed {
            return Err(DebugError::Closed);
        }
        state.entry_gate = false;
        self.refresh_hints(&state);
        drop(state);
        self.notify()
    }
    pub fn entry_held(&self) -> bool {
        self.entry_hint.load(Ordering::Acquire)
    }
    pub fn wait_entry_work(&self) {
        let mut state = self.lock();
        while state.entry_gate
            && !state.closed
            && state.queued.is_empty()
            && !state.pause_requested
            && !state.continuing.is_some_and(|control| !control.2)
            && !(state.detach_requested && !state.detach_delivered)
            && !self.control.stopping.load(Ordering::Acquire)
        {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
    pub fn pending(&self) -> bool {
        self.pending_hint.load(Ordering::Acquire)
    }
    pub fn next(&self, wait: bool) -> Option<DebugDirective> {
        let mut state = self.lock();
        loop {
            if (state.stop_requested || self.control.stopping.load(Ordering::Acquire))
                && !state.stop_delivered
            {
                state.stop_delivered = true;
                self.refresh_hints(&state);
                return Some(DebugDirective::Stop);
            }
            if state.closed {
                self.refresh_hints(&state);
                return None;
            }
            if state.detach_requested && !state.detach_delivered {
                state.detach_delivered = true;
                self.refresh_hints(&state);
                return Some(DebugDirective::Detach);
            }
            if let Some((pause, action, false)) = state.continuing {
                state.continuing = Some((pause, action, true));
                self.refresh_hints(&state);
                return Some(DebugDirective::Continue { pause, action });
            }
            if state.pause_requested {
                state.pause_requested = false;
                self.refresh_hints(&state);
                return Some(DebugDirective::Pause);
            }
            while let Some(slot) = state.queued.pop_front() {
                let Some(record) = state.slots[slot].as_mut() else {
                    continue;
                };
                if record.result.is_some() {
                    continue;
                }
                let request = record.request.take()?;
                let error = validate_request_pause(&request, &state).err();
                if let Some(error) = error {
                    finish(&mut state, slot, Err(error));
                    self.changed.notify_all();
                    continue;
                }
                // Validate options at admission, but start the evaluator budget
                // only when dispatched. Queue residence consumes no runtime.
                let control = match &request {
                    DebugRequest::Evaluate { .. } | DebugRequest::Write { .. } => {
                        EvaluationControl::new(EvaluationOptions {
                            deadline_ms: self.limits.evaluation_deadline_ms,
                            ..EvaluationOptions::default()
                        })
                        .map(Some)
                    }
                    DebugRequest::EvaluateWithOptions { options, .. } => {
                        EvaluationControl::new(*options).map(Some)
                    }
                    _ => Ok(None),
                };
                let control = match control {
                    Ok(control) => control,
                    Err(error) => {
                        finish(&mut state, slot, Err(error));
                        self.changed.notify_all();
                        continue;
                    }
                };
                let record = state.slots[slot].as_ref().unwrap();
                let id = record.id;
                if record.evaluation {
                    state.evaluation = Some(id);
                    state.evaluation_control = control;
                }
                self.refresh_hints(&state);
                return Some(DebugDirective::Request(DebugCommand { id, request }));
            }
            if !wait {
                self.refresh_hints(&state);
                return None;
            }
            // Authoritative state is tested under the same mutex used by every
            // publisher before Condvar atomically releases it: no arming race.
            self.refresh_hints(&state);
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
    pub fn publish(&self, update: DebugUpdate) -> Result<(), DebugError> {
        let mut state = self.lock();
        match update {
            DebugUpdate::Reply { id, result } => {
                if id.session != self.id {
                    return Err(DebugError::StaleSession);
                }
                let slot = state
                    .slots
                    .iter()
                    .position(|record| record.as_ref().is_some_and(|record| record.id == id))
                    .ok_or(DebugError::InvalidRequest)?;
                let record = state.slots[slot].as_ref().unwrap();
                if record.result.is_some() {
                    return Err(if state.closed {
                        DebugError::Closed
                    } else {
                        DebugError::InvalidRequest
                    });
                }
                if record.request.is_some() {
                    return Err(DebugError::InvalidRequest);
                }
                let result = match result {
                    Ok(reply)
                        if reply_footprint(&reply)
                            .is_ok_and(|bytes| bytes <= self.limits.reply_bytes) =>
                    {
                        Ok(reply)
                    }
                    Ok(_) => Err(DebugError::Capacity),
                    Err(error) => Err(bounded_error(error)),
                };
                finish(&mut state, slot, result);
            }
            DebugUpdate::Paused {
                sequence,
                pause,
                reason,
                location,
                original,
                breakpoint_ids,
            } => {
                if checked_add(
                    size_of::<DebugUpdate>(),
                    vector_bytes::<u64>(breakpoint_ids.capacity())?,
                )? > self.limits.reply_bytes
                {
                    return Err(DebugError::Capacity);
                }
                if state.closed || pause.session != self.id {
                    return Err(DebugError::StaleSession);
                }
                if sequence <= state.last_sequence {
                    return Err(DebugError::StalePause);
                }
                state.last_sequence = sequence;
                state.execution = DebugExecutionState::Paused(pause);
                state.historical_pause = reason == PauseReason::UnhandledRejection;
                state.continuing = None;
                state.pause_requested = false;
                state.latest = Some(DebugUpdate::Paused {
                    sequence,
                    pause,
                    reason,
                    location,
                    original,
                    breakpoint_ids,
                });
            }
            DebugUpdate::Resumed { sequence, previous } => {
                if state.closed || previous.session != self.id {
                    return Err(DebugError::StaleSession);
                }
                if sequence <= state.last_sequence {
                    return Err(DebugError::StalePause);
                }
                state.last_sequence = sequence;
                state.execution = DebugExecutionState::Running;
                state.historical_pause = false;
                state.continuing = None;
                invalidate_pause(&mut state, previous);
                state.latest = Some(DebugUpdate::Resumed { sequence, previous });
            }
            DebugUpdate::SessionClosed { session, reason } => {
                if session != self.id {
                    return Err(DebugError::StaleSession);
                }
                close_locked(&mut state, session, reason);
            }
            DebugUpdate::Diagnostic { report, dropped } => {
                let dropped = match state.diagnostic.take() {
                    Some(DebugUpdate::Diagnostic { dropped: old, .. }) => {
                        old.saturating_add(dropped).saturating_add(1)
                    }
                    _ => dropped,
                };
                state.diagnostic = Some(DebugUpdate::Diagnostic {
                    report: report.bounded(),
                    dropped,
                });
            }
            update => {
                if state.closed {
                    return Err(DebugError::Closed);
                }
                if state.updates.len() == self.limits.updates {
                    return Err(DebugError::Capacity);
                }
                if update_footprint(&update)? > self.limits.reply_bytes {
                    return Err(DebugError::Capacity);
                }
                state.updates.push_back(update);
            }
        }
        self.refresh_hints(&state);
        drop(state);
        self.changed.notify_all();
        Ok(())
    }
    pub fn close(&self, reason: Option<FailureReport>, permanent: bool) {
        let mut state = self.lock();
        close_locked(&mut state, self.id, reason);
        if permanent {
            state.execution = DebugExecutionState::Closed;
        }
        self.refresh_hints(&state);
        drop(state);
        self.changed.notify_all();
        self.signal.revoke();
    }
    fn recv(&self, wait: bool) -> Option<DebugUpdate> {
        let mut state = self.lock();
        loop {
            if let Some(slot) = state.replies.pop_front()
                && let Some(mut record) = state.slots[slot].take()
            {
                return Some(DebugUpdate::Reply {
                    id: record.id,
                    result: record.result.take().unwrap(),
                });
            }
            if let Some(update) = state.latest.take() {
                return Some(update);
            }
            if let Some(update) = state.updates.pop_front() {
                return Some(update);
            }
            if let Some(update) = state.diagnostic.take() {
                return Some(update);
            }
            if let Some(update) = state.closure.take() {
                return Some(update);
            }
            if state.closed || !wait {
                return None;
            }
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}
fn finish(state: &mut State, slot: usize, result: Result<DebugReply, DebugError>) {
    let record = state.slots[slot].as_mut().unwrap();
    if record.result.is_some() {
        return;
    }
    if let Some(index) = record.breakpoint_budget
        && let Some(budget) = &mut state.breakpoint_budgets[index]
    {
        if let Some((id, count, bytes)) = budget.pending.take()
            && id == record.id
            && matches!(&result, Ok(DebugReply::Breakpoints(_)))
        {
            budget.count = count;
            budget.bytes = bytes;
        }
        if budget.count == 0 && budget.pending.is_none() {
            state.breakpoint_budgets[index] = None;
        }
    }
    record.request = None;
    record.result = Some(result);
    if state.evaluation == Some(record.id) {
        state.evaluation = None;
        if let Some(control) = state.evaluation_control.take() {
            control.revoke();
        }
    }
    state.queued.retain(|queued| *queued != slot);
    state.replies.push_back(slot);
}
fn breakpoint_budget(
    request: &DebugRequest,
    state: &State,
    limits: &DebugLimits,
) -> Result<Option<(usize, usize, usize)>, DebugError> {
    let DebugRequest::SetBreakpoints {
        selector,
        breakpoints,
    } = request
    else {
        return Ok(None);
    };
    let existing = state.breakpoint_budgets.iter().position(|entry| {
        entry.as_ref().is_some_and(|entry| {
            entry.origin == selector.origin && entry.digest == selector.expected_digest
        })
    });
    if existing.is_some_and(|index| {
        state.breakpoint_budgets[index]
            .as_ref()
            .unwrap()
            .pending
            .is_some()
    }) {
        return Err(DebugError::InFlight);
    }
    let mut count = 0;
    let mut bytes = 0;
    for (index, entry) in state.breakpoint_budgets.iter().enumerate() {
        if Some(index) == existing {
            continue;
        }
        if let Some(entry) = entry {
            count = checked_add(
                count,
                entry
                    .pending
                    .map_or(entry.count, |(_, pending, _)| pending.max(entry.count)),
            )?;
            bytes = checked_add(
                bytes,
                entry
                    .pending
                    .map_or(entry.bytes, |(_, _, pending)| pending.max(entry.bytes)),
            )?;
        }
    }
    let new_count = breakpoints.len();
    let mut new_bytes = 0;
    for condition in breakpoints.iter().flat_map(|breakpoint| {
        [
            breakpoint.condition.as_ref(),
            breakpoint.log_message.as_ref(),
        ]
        .into_iter()
        .flatten()
    }) {
        new_bytes = checked_add(new_bytes, condition.capacity_bytes())?;
    }
    let old = existing.and_then(|index| state.breakpoint_budgets[index].as_ref());
    count = checked_add(count, new_count.max(old.map_or(0, |old| old.count)))?;
    bytes = checked_add(bytes, new_bytes.max(old.map_or(0, |old| old.bytes)))?;
    if count > limits.breakpoints
        || bytes > limits.condition_bytes
        || checked_add(size_of::<DebugUpdate>(), vector_bytes::<u64>(count)?)? > limits.reply_bytes
    {
        return Err(DebugError::Capacity);
    }
    let index = existing
        .or_else(|| state.breakpoint_budgets.iter().position(Option::is_none))
        .ok_or(DebugError::Capacity)?;
    Ok(Some((index, new_count, new_bytes)))
}
fn invalidate_pause(state: &mut State, pause: PauseId) {
    for slot in 0..state.slots.len() {
        if state.slots[slot]
            .as_ref()
            .is_some_and(|record| record.pause == Some(pause) && record.result.is_none())
        {
            finish(state, slot, Err(DebugError::StalePause));
        }
    }
}
fn close_locked(state: &mut State, session: DebugSessionId, reason: Option<FailureReport>) {
    if state.closed {
        return;
    }
    state.closed = true;
    state.entry_gate = false;
    state.execution = DebugExecutionState::Detached;
    state.latest = None;
    state.queued.clear();
    state.evaluation = None;
    if let Some(control) = state.evaluation_control.take() {
        control.revoke();
    }
    state.continuing = None;
    for slot in 0..state.slots.len() {
        if state.slots[slot]
            .as_ref()
            .is_some_and(|r| r.result.is_none())
        {
            finish(state, slot, Err(DebugError::Closed));
        }
    }
    let reason = reason.or_else(|| match &state.fault {
        Some(DebugError::EngineFailure(report) | DebugError::WakeFailed(report)) => {
            Some(report.clone())
        }
        _ => None,
    });
    state.closure = Some(DebugUpdate::SessionClosed {
        session,
        reason: reason.map(FailureReport::bounded),
    });
}
fn request_pause(request: &DebugRequest) -> Option<PauseId> {
    match request {
        DebugRequest::ReleaseCursor(cursor) => Some(cursor.pause),
        DebugRequest::ReleaseWriteTargets(targets) => targets.first().map(|target| match target {
            WriteTarget::Binding(r) => r.pause,
            WriteTarget::Property(r) => r.pause,
        }),
        DebugRequest::Stack { pause, .. }
        | DebugRequest::ContinueTo { pause, .. }
        | DebugRequest::ExceptionInfo { pause }
        | DebugRequest::AsyncStack { pause } => Some(*pause),
        DebugRequest::Scopes { frame }
        | DebugRequest::Evaluate { frame, .. }
        | DebugRequest::EvaluateWithOptions { frame, .. }
        | DebugRequest::Write { frame, .. }
        | DebugRequest::LookupPath { frame, .. } => Some(frame.pause),
        DebugRequest::Variables { parent, .. }
        | DebugRequest::Inspect(InspectionRequest { parent, .. }) => Some(match parent {
            VariableParent::Scope(v) => v.pause,
            VariableParent::Object(v) => v.pause,
        }),
        _ => None,
    }
}
fn validate_request_pause(request: &DebugRequest, state: &State) -> Result<(), DebugError> {
    if let Some(pause) = request_pause(request) {
        if state.execution != DebugExecutionState::Paused(pause) {
            return Err(
                if matches!(state.execution, DebugExecutionState::Paused(_)) {
                    DebugError::StalePause
                } else {
                    DebugError::NotPaused
                },
            );
        }
        if state.evaluation.is_some() {
            return Err(DebugError::InFlight);
        }
    }
    Ok(())
}
fn checked_add(a: usize, b: usize) -> Result<usize, DebugError> {
    a.checked_add(b).ok_or(DebugError::Capacity)
}
fn vector_bytes<T>(capacity: usize) -> Result<usize, DebugError> {
    capacity
        .checked_mul(size_of::<T>())
        .ok_or(DebugError::Capacity)
}
pub(super) fn request_footprint(
    request: &DebugRequest,
    limits: &DebugLimits,
    descriptor: &DebugDescriptor,
) -> Result<(), DebugError> {
    let mut bytes = size_of::<DebugRequest>();
    match request {
        DebugRequest::StepPolicy(policy) => {
            if !descriptor
                .capabilities
                .contains(DebugCapability::SkipSources)
            {
                return Err(DebugError::Unsupported);
            }
            super::stepping::validate(policy, limits)?;
            bytes = checked_add(
                bytes,
                vector_bytes::<String>(policy.skip_sources.capacity())?,
            )?;
            for pattern in &policy.skip_sources {
                bytes = checked_add(bytes, pattern.capacity())?;
            }
        }
        DebugRequest::UnhandledRejections(_) | DebugRequest::PromiseRejections { .. } => {
            if !descriptor
                .capabilities
                .contains(DebugCapability::UnhandledRejections)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::AsyncStack { .. } => {
            if !descriptor
                .capabilities
                .contains(DebugCapability::AsyncStacks)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::ExceptionInfo { .. } => {
            if !descriptor
                .capabilities
                .contains(DebugCapability::ExceptionDetails)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::ReleaseCursor(_) => {
            if !descriptor
                .capabilities
                .contains(DebugCapability::LazyInspection)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::Inspect(inspection) => {
            vector_bytes::<InspectionEntry>(inspection.page.count)?;
            let allocation = checked_add(
                size_of::<DebugReply>(),
                vector_bytes::<InspectionEntry>(
                    inspection.page.count.min(limits.inspection_page_items).min(
                        limits.reply_bytes.saturating_sub(size_of::<DebugReply>())
                            / size_of::<InspectionEntry>(),
                    ),
                )?,
            )?;
            if allocation > limits.reply_bytes {
                return Err(DebugError::Capacity);
            }
            if !descriptor
                .capabilities
                .contains(DebugCapability::LazyInspection)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::LookupPath { path, .. } => {
            LookupPath::parse(path)?;
            if path.as_units().len() > limits.expression_units {
                return Err(DebugError::Capacity);
            }
            bytes = checked_add(bytes, path.capacity_bytes())?;
            if !descriptor
                .capabilities
                .contains(DebugCapability::SafeLookup)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::ReleaseWriteTargets(targets) => {
            if targets.len() > limits.write_targets {
                return Err(DebugError::Capacity);
            }
            bytes = checked_add(bytes, vector_bytes::<WriteTarget>(targets.capacity())?)?;
        }
        DebugRequest::Write { expression, .. } => {
            if expression.as_units().len() > limits.expression_units {
                return Err(DebugError::Capacity);
            }
            bytes = checked_add(bytes, expression.capacity_bytes())?;
            if !descriptor
                .capabilities
                .contains(DebugCapability::VariableMutation)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::EvaluateWithOptions {
            expression,
            options,
            ..
        } => {
            EvaluationControl::new(*options)?;
            if expression.as_units().len() > limits.expression_units {
                return Err(DebugError::Capacity);
            }
            bytes = checked_add(bytes, expression.capacity_bytes())?;
            // Structured cancellation/deadline outcomes require native evaluator-only unwind.
            if !descriptor
                .capabilities
                .contains(DebugCapability::CancelEvaluation)
                || (!options.allow_side_effects
                    && !descriptor
                        .capabilities
                        .contains(DebugCapability::ReadOnlyEvaluation))
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::BreakpointLocations {
            selector,
            start,
            end,
        } => {
            if start > end {
                return Err(DebugError::InvalidSource);
            }
            if !descriptor
                .capabilities
                .contains(DebugCapability::ColumnBreakpoints)
            {
                return Err(DebugError::Unsupported);
            }
            bytes = checked_add(bytes, selector.origin.capacity())?;
        }
        DebugRequest::ContinueTo { .. } => {
            if !descriptor
                .capabilities
                .contains(DebugCapability::ContinueTo)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::SetBreakpointEnabled { selector, .. } => {
            bytes = checked_add(bytes, selector.origin.capacity())?;
            if !descriptor
                .capabilities
                .contains(DebugCapability::BreakpointPolicies)
            {
                return Err(DebugError::Unsupported);
            }
        }
        DebugRequest::OriginalSource { selector, .. } => {
            bytes = checked_add(bytes, selector.origin.capacity())?;
        }
        DebugRequest::SetBreakpoints {
            selector,
            breakpoints,
        } => {
            if breakpoints.len() > limits.breakpoints {
                return Err(DebugError::Capacity);
            }
            bytes = checked_add(bytes, selector.origin.capacity())?;
            let specs = vector_bytes::<BreakpointSpec>(breakpoints.capacity())?;
            bytes = checked_add(bytes, specs)?;
            let mut conditions = 0;
            for (index, breakpoint) in breakpoints.iter().enumerate() {
                if breakpoints[..index]
                    .iter()
                    .any(|previous| previous.client_id == breakpoint.client_id)
                {
                    return Err(DebugError::InvalidRequest);
                }

                if breakpoint.column.is_some()
                    && !descriptor
                        .capabilities
                        .contains(DebugCapability::ColumnBreakpoints)
                {
                    return Err(DebugError::Unsupported);
                }
                if (!breakpoint.enabled || breakpoint.hit_condition.is_some())
                    && !descriptor
                        .capabilities
                        .contains(DebugCapability::BreakpointPolicies)
                {
                    return Err(DebugError::Unsupported);
                }
                if let Some(condition) = &breakpoint.condition {
                    if condition.as_units().len() > limits.expression_units {
                        return Err(DebugError::Capacity);
                    }
                    conditions = checked_add(conditions, condition.capacity_bytes())?;
                }
                if let Some(message) = &breakpoint.log_message {
                    if !descriptor.capabilities.contains(DebugCapability::Logpoints) {
                        return Err(DebugError::Unsupported);
                    }
                    conditions = checked_add(conditions, message.capacity_bytes())?;
                    super::logpoint::validate(message.as_units(), limits.expression_units)?;
                }
            }
            if conditions > limits.condition_bytes {
                return Err(DebugError::Capacity);
            }
            bytes = checked_add(bytes, conditions)?;
        }
        DebugRequest::Evaluate {
            expression,
            allow_side_effects,
            ..
        } => {
            if expression.as_units().len() > limits.expression_units {
                return Err(DebugError::Capacity);
            }
            if !allow_side_effects
                && !descriptor
                    .capabilities
                    .contains(DebugCapability::ReadOnlyEvaluation)
            {
                return Err(DebugError::Unsupported);
            }
            bytes = checked_add(bytes, expression.capacity_bytes())?;
        }
        _ => {}
    }
    if bytes > limits.request_bytes {
        Err(DebugError::Capacity)
    } else {
        Ok(())
    }
}
fn value_bytes(value: &DebugValue) -> Result<usize, DebugError> {
    checked_add(
        value.preview.capacity_bytes(),
        value.unavailable.as_ref().map_or(0, String::capacity),
    )
}
fn breakpoints_bytes(values: &Vec<BreakpointInfo>) -> Result<usize, DebugError> {
    let mut bytes = vector_bytes::<BreakpointInfo>(values.capacity())?;
    for value in values {
        let payload = match &value.state {
            BreakpointState::Pending => 0,
            BreakpointState::Verified(locations) => {
                vector_bytes::<DebugLocation>(locations.capacity())?
            }
            BreakpointState::Rejected(reason) => reason.capacity(),
        };
        bytes = checked_add(bytes, payload)?;
    }
    Ok(bytes)
}
pub(super) fn reply_footprint(reply: &DebugReply) -> Result<usize, DebugError> {
    let payload = match reply {
        DebugReply::Sources { scripts, .. } => {
            let mut total = vector_bytes::<ScriptInfo>(scripts.capacity())?;
            for script in scripts {
                total = checked_add(total, script.origin.capacity())?;
            }
            total
        }
        DebugReply::Source { text, .. } => text.capacity_bytes(),
        DebugReply::Breakpoints(values) => breakpoints_bytes(values)?,
        DebugReply::Locations(values) => vector_bytes::<DebugLocation>(values.capacity())?,
        DebugReply::OriginalLocations(values) => {
            let mut bytes = vector_bytes::<OriginalLocation>(values.capacity())?;
            for value in values {
                bytes = checked_add(bytes, value.origin.capacity())?;
            }
            bytes
        }
        DebugReply::PromiseRejections { records, .. } => {
            let mut bytes = vector_bytes::<PromiseRejectionInfo>(records.capacity())?;
            for record in records {
                bytes = checked_add(bytes, record.description.capacity_bytes())?;
                bytes = checked_add(
                    bytes,
                    record.original.as_ref().map_or(0, |o| o.origin.capacity()),
                )?;
            }
            bytes
        }
        DebugReply::UnhandledRejections => 0,
        DebugReply::AsyncStack(trace) => trace.heap_bytes()?,
        DebugReply::ExceptionInfo(info) => {
            let mut total =
                checked_add(value_bytes(&info.value)?, info.description.capacity_bytes())?;
            total = checked_add(
                total,
                vector_bytes::<ExceptionFrame>(info.stack.capacity())?,
            )?;
            for frame in &info.stack {
                total = checked_add(total, frame.name.capacity_bytes())?;
                total = checked_add(
                    total,
                    frame.original.as_ref().map_or(0, |o| o.origin.capacity()),
                )?;
            }
            total
        }
        DebugReply::StepPolicy
        | DebugReply::ExceptionPolicy(_)
        | DebugReply::ContinueTo
        | DebugReply::CursorReleased
        | DebugReply::WriteTargetsReleased => 0,
        DebugReply::Stack { frames, .. } => {
            let mut total = vector_bytes::<DebugFrame>(frames.capacity())?;
            for frame in frames {
                total = checked_add(total, frame.name.capacity_bytes())?;
                let original = frame
                    .original
                    .as_ref()
                    .map_or(0, |original| original.origin.capacity());
                total = checked_add(total, original)?;
            }
            total
        }
        DebugReply::Scopes(scopes) => {
            let mut total = vector_bytes::<DebugScope>(scopes.capacity())?;
            for scope in scopes {
                total = checked_add(total, scope.name.capacity_bytes())?;
            }
            total
        }
        DebugReply::Variables { variables, .. } => {
            let mut total = vector_bytes::<DebugVariable>(variables.capacity())?;
            for variable in variables {
                total = checked_add(total, variable.name.capacity_bytes())?;
                let value = value_bytes(&variable.value)?;
                total = checked_add(total, value)?;
            }
            total
        }
        DebugReply::Inspection(page) => {
            let mut total = vector_bytes::<InspectionEntry>(page.items.capacity())?;
            for entry in &page.items {
                total = checked_add(total, entry.variable.name.capacity_bytes())?;
                total = checked_add(total, entry.type_name.capacity_bytes())?;
                total = checked_add(total, value_bytes(&entry.variable.value)?)?;
            }
            total
        }
        DebugReply::EvaluationOutcome(outcome) => {
            outcome.value.as_ref().map_or(Ok(0), value_bytes)?
        }
        DebugReply::Evaluation { value, .. }
        | DebugReply::Write(value)
        | DebugReply::Lookup(value) => value_bytes(value)?,
    };
    checked_add(size_of::<DebugReply>(), payload)
}
fn update_footprint(update: &DebugUpdate) -> Result<usize, DebugError> {
    checked_add(
        size_of::<DebugUpdate>(),
        match update {
            DebugUpdate::PromiseRejection(info) => checked_add(
                info.description.capacity_bytes(),
                info.original.as_ref().map_or(0, |o| o.origin.capacity()),
            )?,
            DebugUpdate::ScriptRegistered(script) => script.origin.capacity(),
            DebugUpdate::BreakpointsChanged(values) => breakpoints_bytes(values)?,
            DebugUpdate::Output { message } => message.capacity_bytes(),
            DebugUpdate::Paused { breakpoint_ids, .. } => {
                vector_bytes::<u64>(breakpoint_ids.capacity())?
            }
            _ => 0,
        },
    )
}
fn bounded_error(error: DebugError) -> DebugError {
    match error {
        DebugError::WakeFailed(report) => DebugError::WakeFailed(report.bounded()),
        DebugError::EngineFailure(report) => DebugError::EngineFailure(report.bounded()),
        error => error,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/debug_transport.rs"]
mod tests;

#[cfg(test)]
mod evaluation_transport_tests {
    use super::*;
    use crate::engine::{ContextLimits, EngineIdentity, JsString, ValueStore};
    use std::time::{Duration, Instant};

    fn fixture(cancellable: bool) -> (VmDebugHandle, FrameRef) {
        let identity = EngineIdentity::new().unwrap();
        let store = ValueStore::<()>::new(&identity, ContextLimits::default()).unwrap();
        let ids = DebugIdentities::new(std::num::NonZeroU64::new(1).unwrap(), store.context(), 8)
            .unwrap();
        let id = ids.open_session().unwrap();
        let capabilities = if cancellable {
            DebugCapabilities::baseline()
                .with(DebugCapability::CancelEvaluation)
                .with(DebugCapability::VariableMutation)
        } else {
            DebugCapabilities::baseline()
        };
        let session = Session::new(
            id,
            false,
            ids.request_issuer(id).unwrap(),
            DebugDescriptor {
                api_version: 1,
                capabilities,
                native_requirements: vec![],
                interrupt_mechanism: "transport fixture".into(),
            },
            DebugLimits::default(),
            ContextControl::new(Arc::new(|| Ok(()))),
        )
        .unwrap();
        let pause = PauseId {
            session: id,
            generation: 1,
        };
        session
            .publish(DebugUpdate::Paused {
                sequence: 1,
                pause,
                reason: PauseReason::Requested,
                location: None,
                original: None,
                breakpoint_ids: vec![],
            })
            .unwrap();
        (
            VmDebugHandle { session },
            FrameRef {
                pause,
                generation: 1,
                slot: 0,
            },
        )
    }
    fn admit(handle: &VmDebugHandle, frame: FrameRef) -> DebugRequestId {
        match handle
            .try_request(DebugRequest::Evaluate {
                frame,
                expression: JsString::from_utf8("1"),
                allow_side_effects: true,
            })
            .unwrap()
        {
            DebugRequestReceipt::Accepted { id }
            | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
        }
    }
    fn dispatch(handle: &VmDebugHandle, id: DebugRequestId) -> EvaluationControl {
        assert!(
            matches!(handle.session.next(false), Some(DebugDirective::Request(DebugCommand { id: actual, .. })) if actual == id)
        );
        handle.session.evaluation_control(id).unwrap()
    }
    fn complete(handle: &VmDebugHandle, id: DebugRequestId) {
        handle
            .session
            .publish(DebugUpdate::Reply {
                id,
                result: Err(DebugError::Unsupported),
            })
            .unwrap();
    }
    #[test]
    fn cancel_requires_capability_and_exact_current_evaluator() {
        let (unsupported, frame) = fixture(false);
        let id = admit(&unsupported, frame);
        dispatch(&unsupported, id);
        assert_eq!(
            unsupported.cancel_evaluation(id).unwrap_err(),
            DebugError::Unsupported
        );
        let (handle, frame) = fixture(true);
        let id = admit(&handle, frame);
        assert_eq!(
            handle.cancel_evaluation(id).unwrap_err(),
            DebugError::InvalidRequest
        );
        let token = dispatch(&handle, id);
        let mut foreign = id;
        foreign.session.generation += 1;
        assert_eq!(
            handle.cancel_evaluation(foreign).unwrap_err(),
            DebugError::StaleSession
        );
        let mut wrong = id;
        wrong.sequence += 1;
        assert_eq!(
            handle.cancel_evaluation(wrong).unwrap_err(),
            DebugError::InvalidRequest
        );
        assert_eq!(token.poll_at(Instant::now()), None);
        assert!(matches!(
            handle.cancel_evaluation(id).unwrap(),
            DebugControlReceipt::Accepted
        ));
        assert!(matches!(
            handle.cancel_evaluation(id).unwrap(),
            DebugControlReceipt::AlreadyRequested
        ));
        assert_eq!(
            token.poll_at(Instant::now()),
            Some(EvaluationStatus::Cancelled)
        );
        assert!(!handle.session.control.stopping.load(Ordering::Acquire));
    }
    #[test]
    fn completion_and_replacement_revoke_tokens_without_rewriting_results() {
        let (handle, frame) = fixture(true);
        let first = admit(&handle, frame);
        let old = dispatch(&handle, first);
        complete(&handle, first);
        assert!(!old.cancel());
        assert_eq!(old.poll_at(Instant::now() + Duration::from_secs(10)), None);
        assert_eq!(
            handle.cancel_evaluation(first).unwrap_err(),
            DebugError::InvalidRequest
        );
        let second = admit(&handle, frame);
        let current = dispatch(&handle, second);
        assert_eq!(
            handle.cancel_evaluation(first).unwrap_err(),
            DebugError::InvalidRequest
        );
        assert_eq!(current.poll_at(Instant::now()), None);
        complete(&handle, second);
        for expected in [first, second] {
            assert!(
                matches!(handle.try_recv(), Some(DebugUpdate::Reply { id, result: Err(DebugError::Unsupported) }) if id == expected)
            );
        }
        assert!(!matches!(
            handle.try_recv(),
            Some(DebugUpdate::Reply { .. })
        ));
    }
    #[test]
    fn close_and_pause_end_revoke_pending_cancellation() {
        for close in [false, true] {
            let (handle, frame) = fixture(true);
            let id = admit(&handle, frame);
            let token = dispatch(&handle, id);
            handle.cancel_evaluation(id).unwrap();
            if close {
                handle.session.close(None, true);
            } else {
                handle
                    .session
                    .publish(DebugUpdate::Resumed {
                        sequence: 2,
                        previous: frame.pause,
                    })
                    .unwrap();
            }
            assert!(!token.cancel());
            assert_eq!(
                token.poll_at(Instant::now() + Duration::from_secs(10)),
                None
            );
            assert!(handle.session.evaluation_control(id).is_none());
            assert!(
                matches!(handle.try_recv(), Some(DebugUpdate::Reply { id: actual, result: Err(_) }) if actual == id)
            );
            assert!(!matches!(
                handle.try_recv(),
                Some(DebugUpdate::Reply { .. })
            ));
        }
    }
    #[test]
    fn cancel_bypasses_full_request_and_reply_capacity() {
        let (handle, frame) = fixture(true);
        let id = admit(&handle, frame);
        let token = dispatch(&handle, id);
        for _ in 1..handle.session.limits.requests {
            let request = DebugRequest::Sources {
                page: Page { start: 0, count: 1 },
            };
            handle.try_request(request).unwrap();
        }
        assert!(
            handle
                .try_request(DebugRequest::Sources {
                    page: Page { start: 0, count: 1 }
                })
                .is_err()
        );
        assert!(matches!(
            handle.cancel_evaluation(id).unwrap(),
            DebugControlReceipt::Accepted
        ));
        assert_eq!(
            token.poll_at(Instant::now()),
            Some(EvaluationStatus::Cancelled)
        );
        assert!(!matches!(
            handle.try_recv(),
            Some(DebugUpdate::Reply { .. })
        ));
    }
    #[test]
    fn invalid_options_reject_before_publication() {
        let (handle, frame) = fixture(true);
        for deadline_ms in [0, u64::MAX / 1_000_000 + 1, u64::MAX] {
            assert!(matches!(
                handle.try_request(DebugRequest::EvaluateWithOptions {
                    frame,
                    expression: JsString::from_utf8("1"),
                    options: EvaluationOptions {
                        deadline_ms,
                        ..EvaluationOptions::default()
                    }
                }),
                Err(Rejected {
                    reason: RejectionReason::Debug(DebugError::InvalidRequest),
                    ..
                })
            ));
        }
        assert!(handle.session.next(false).is_none());
        assert!(!handle.session.pending());
    }
    #[test]
    fn cancel_finish_race_keeps_one_reply_and_disarms_old_token() {
        for _ in 0..32 {
            let (handle, frame) = fixture(true);
            let id = admit(&handle, frame);
            let token = dispatch(&handle, id);
            std::thread::scope(|scope| {
                let cancel = scope.spawn(|| handle.cancel_evaluation(id));
                complete(&handle, id);
                assert!(matches!(
                    cancel.join().unwrap(),
                    Ok(DebugControlReceipt::Accepted) | Err(DebugError::InvalidRequest)
                ));
            });
            assert!(!token.cancel());
            assert_eq!(
                token.poll_at(Instant::now() + Duration::from_secs(10)),
                None
            );
            assert!(
                matches!(handle.try_recv(), Some(DebugUpdate::Reply { id: actual, .. }) if actual == id)
            );
            assert!(!matches!(
                handle.try_recv(),
                Some(DebugUpdate::Reply { .. })
            ));
        }
    }
    #[test]
    fn cancellation_notifies_without_transport_lock_and_reports_wake_failure() {
        struct ObserveSignal {
            session: Weak<Session>,
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl DebugSignal for ObserveSignal {
            fn wake(&self) -> Result<(), DebugError> {
                let session = self.session.upgrade().unwrap();
                let state = session
                    .state
                    .try_lock()
                    .expect("notify must release transport mutex");
                assert_eq!(
                    state
                        .evaluation_control
                        .as_ref()
                        .unwrap()
                        .poll_at(Instant::now()),
                    Some(EvaluationStatus::Cancelled)
                );
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let (handle, frame) = fixture(true);
        let id = admit(&handle, frame);
        let token = dispatch(&handle, id);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        handle.session.signal.install(Arc::new(ObserveSignal {
            session: Arc::downgrade(&handle.session),
            calls: calls.clone(),
        }));
        // Inject a failing native wake only after successful request admission.
        let failure = DebugError::WakeFailed(FailureReport::fault(
            FailureCode::EngineFailure,
            "cancel wake",
        ));
        struct FailedSignal(DebugError);
        impl DebugSignal for FailedSignal {
            fn wake(&self) -> Result<(), DebugError> {
                Err(self.0.clone())
            }
        }
        assert!(matches!(
            handle.cancel_evaluation(id).unwrap(),
            DebugControlReceipt::Accepted
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        handle
            .session
            .signal
            .install(Arc::new(FailedSignal(failure.clone())));
        assert!(
            matches!(handle.cancel_evaluation(id).unwrap(), DebugControlReceipt::AcceptedWakeFailed { error } if error == failure)
        );
        assert_eq!(
            token.poll_at(Instant::now()),
            Some(EvaluationStatus::Cancelled)
        );
        assert!(handle.session.control.stopping.load(Ordering::Acquire));
        assert!(matches!(
            handle.session.next(false),
            Some(DebugDirective::Stop)
        ));
        complete(&handle, id);
        assert!(
            matches!(handle.try_recv(), Some(DebugUpdate::Reply { id: actual, result: Err(DebugError::Unsupported) }) if actual == id)
        );
        // Remove the test signal before handle drop's urgent detach notification.
        handle.session.signal.revoke();
    }
    #[test]
    fn dispatch_starts_options_budget_and_write_has_cancellable_token() {
        let (handle, frame) = fixture(true);
        let id = match handle
            .try_request(DebugRequest::EvaluateWithOptions {
                frame,
                expression: JsString::from_utf8("1"),
                options: EvaluationOptions {
                    deadline_ms: 1,
                    ..EvaluationOptions::default()
                },
            })
            .unwrap()
        {
            DebugRequestReceipt::Accepted { id }
            | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
        };
        assert!(handle.session.evaluation_control(id).is_none());
        std::thread::sleep(Duration::from_millis(5));
        let dispatch_start = Instant::now();
        let token = dispatch(&handle, id);
        assert_eq!(token.poll_at(dispatch_start), None);
        // Transport preserves the owner's legacy success even when metadata expires.
        assert_eq!(
            token.poll_at(dispatch_start + Duration::from_secs(1)),
            Some(EvaluationStatus::DeadlineExceeded)
        );
        handle
            .session
            .publish(DebugUpdate::Reply {
                id,
                result: Ok(DebugReply::Sources {
                    scripts: vec![],
                    total: 0,
                }),
            })
            .unwrap();
        assert!(
            matches!(handle.try_recv(), Some(DebugUpdate::Reply { id: actual, result: Ok(_) }) if actual == id)
        );
        // Write RHS shares evaluator cancellation and excludes other paused work.
        let write = handle.try_request(DebugRequest::Write {
            target: WriteTarget::Binding(BindingRef {
                pause: frame.pause,
                generation: 1,
                slot: 0,
            }),
            frame,
            expression: JsString::from_utf8("1"),
        });
        let id = match write.unwrap() {
            DebugRequestReceipt::Accepted { id }
            | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
        };
        {
            assert!(matches!(
                handle.session.next(false),
                Some(DebugDirective::Request(_))
            ));
            assert!(handle.session.evaluation_control(id).is_some());
            assert!(matches!(
                handle.cancel_evaluation(id).unwrap(),
                DebugControlReceipt::Accepted
            ));
        }
    }
    #[test]
    fn dispatch_errors_wake_reserved_reply_waiters_without_other_publication() {
        for invalid_options in [false, true] {
            let (handle, frame) = fixture(true);
            let id = match handle
                .try_request(DebugRequest::EvaluateWithOptions {
                    frame,
                    expression: JsString::from_utf8("1"),
                    options: EvaluationOptions::default(),
                })
                .unwrap()
            {
                DebugRequestReceipt::Accepted { id }
                | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
            };
            // Remove the pause update, then arm the same reply predicate/Condvar
            // used by recv. The channel is sent with State locked; reacquiring
            // that lock below proves the waiter has atomically begun waiting.
            handle.try_recv();
            let (armed_tx, armed_rx) = std::sync::mpsc::sync_channel(1);
            std::thread::scope(|scope| {
                let waiter = scope.spawn(|| {
                    let mut state = handle.session.lock();
                    armed_tx.send(()).unwrap();
                    while state.replies.is_empty() {
                        let (next, timeout) = handle.session.changed.wait_timeout(state, Duration::from_secs(1)).unwrap();
                        state = next;
                        assert!(!timeout.timed_out(), "dispatch terminal reply did not notify its waiting consumer");
                    }
                    drop(state);
                    assert!(matches!(handle.wait_update(), Some(DebugUpdate::Reply { id: actual, result: Err(error) })
                        if actual == id && error == if invalid_options { DebugError::InvalidRequest } else { DebugError::NotPaused }));
                });
                armed_rx.recv().unwrap();
                let mut state = handle.session.lock();
                if invalid_options {
                    // Deterministically reach the otherwise rare dispatch-time
                    // Instant range failure branch, without a public test API.
                    let record = state
                        .slots
                        .iter_mut()
                        .flatten()
                        .find(|record| record.id == id)
                        .unwrap();
                    let Some(DebugRequest::EvaluateWithOptions { options, .. }) =
                        record.request.as_mut()
                    else {
                        panic!("queued evaluator");
                    };
                    options.deadline_ms = 0;
                } else {
                    state.execution = DebugExecutionState::Running;
                }
                drop(state);
                assert!(handle.session.next(false).is_none());
                waiter.join().unwrap();
            });
        }
    }
}
