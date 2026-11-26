//! Independent bounded application lanes and reserved completion publication.
//!
//! Producers publish under one short-held mutex. Only an armed owner needs an
//! OS wake; queues remain authoritative across coalescing and partial drains.
use crate::{
    arena::{Arena, Key},
    config::SchedulerConfig,
    engine::*,
    ready::ReadyQueue,
};
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};

/// Metadata-only cross-thread controls and ordinary owned-message admission.
pub struct LoopHandle<M: Send + 'static> {
    shared: Arc<Shared<M>>,
}
impl<M: Send + 'static> Clone for LoopHandle<M> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}
/// Snapshot of admission usage and publication wake calls, without native values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportStats {
    /// Ordinary message permits, including messages transferred to ready state.
    pub messages: usize,
    /// Reserved, pending, ready or dispatching operations.
    pub operations: usize,
    /// Aggregate caller-declared operation payload allowances.
    pub reserved_payload_bytes: usize,
    /// Calls to the OS wake function; coalesced publications do not add a call.
    pub wake_calls: u64,
    /// Monotonic notification that delivery capacity was released.
    pub capacity_epoch: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Message(Key),
    Completion(Key),
}
#[path = "application_lanes/sleep_gate.rs"]
mod gate;
use gate::Gate;
/// Producer-side admission policy containing copied metadata, never live engine
/// values. Validation runs before acquiring the transport publication mutex.
pub type MessageValidator<M> = Arc<dyn Fn(&M) -> Result<(), RejectionReason> + Send + Sync>;
pub(crate) type StopHook = Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync>;
#[cfg(test)]
#[path = "../tests/unit/transport.rs"]
mod tests;
pub(crate) trait Wake: Send + Sync {
    fn wake(&self) -> io::Result<()>;
}
pub(crate) trait HostControl {
    fn stopping(&self) -> bool;
    fn reserve(&self, bytes: usize) -> Result<OperationReservation, FailureReport>;
    fn bind(&self, reservation: OperationReservation) -> Result<CompletionTicket, FailureReport>;
    fn cancel(&self, id: OperationId) -> CancelOutcome;
    fn stop(&self) -> Result<(), FailureReport>;
}
struct MessageSlot<M> {
    value: Option<M>,
}
enum OperationState {
    Reserved,
    Pending,
    Ready(Option<CompletionPayload>),
    Dispatching,
}
impl OperationState {
    /// Decide a reserved terminal result once; callers hold the publication lock.
    fn decide(&mut self, payload: Option<CompletionPayload>) -> bool {
        if !matches!(self, Self::Pending) {
            return false;
        }
        *self = Self::Ready(payload);
        true
    }
}
/// Builds the reserved terminal failure payload for `code` at the moment a terminal path wins.
///
/// Fix: every `reserve` pre-built three of these (a `String` plus a `JsString` each) for
/// outcomes that almost never happen. Decision: build the one that is needed when
/// cancel, abandon or an oversize publication decides the operation. The payload is identical
/// to the pre-built one; each operation decides exactly once, so nothing is lost by not
/// holding spares.
///
/// # Arguments
///
/// * `code` - the stable failure code, also used as the message (`Canceled`, `Abandoned`,
///   `PayloadTooLarge`)
///
/// # Returns
///
/// A failure payload without details.
fn terminal_payload(code: &str) -> Option<CompletionPayload> {
    Some(CompletionPayload::Failure(OperationFailure {
        code: code.into(),
        message: JsString::from_utf8(code),
        details: None,
    }))
}
struct OperationSlot {
    state: OperationState,
    bytes: usize,
    canceled: bool,
}
struct State<M> {
    epoch: u64,
    gate: Gate,
    stop: bool,
    fault: Option<FailureReport>,
    messages: Arena<MessageSlot<M>>,
    operations: Arena<OperationSlot>,
    notifications: ReadyQueue<Delivery>,
    controls: VecDeque<TaskId>,
    control_limit: usize,
    reserved_bytes: usize,
    max_bytes: usize,
    message_capacity: usize,
    wakes: u64,
    capacity_epoch: u64,
    /// One task registration per outstanding capacity wait, not per poll.
    capacity_waiters: HashMap<usize, Waker>,
}
/// Resolves once delivery capacity was released after the epoch it was created with, or once
/// the transport stops or closes. Producers that got [`RejectionReason::Full`] await it
/// instead of polling.
pub struct CapacityChange<M: Send + 'static> {
    shared: Arc<Shared<M>>,
    seen: u64,
    identity: Option<Arc<()>>,
}
impl<M: Send + 'static> Future for CapacityChange<M> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let waker = cx.waker().clone();
        let mut state = this.shared.lock();
        if state.capacity_epoch != this.seen
            || state.stop
            || matches!(state.gate, Gate::Closed | Gate::Faulted)
        {
            return Poll::Ready(());
        }
        // The private allocation keeps its identity even when the future moves.
        // Its address is only a map key, never dereferenced or given to a producer.
        let identity = this.identity.get_or_insert_with(|| Arc::new(()));
        let previous = state
            .capacity_waiters
            .insert(Arc::as_ptr(identity) as usize, waker);
        drop(state);
        drop(previous);
        Poll::Pending
    }
}
impl<M: Send + 'static> Drop for CapacityChange<M> {
    fn drop(&mut self) {
        if let Some(identity) = &self.identity {
            let waker = self
                .shared
                .lock()
                .capacity_waiters
                .remove(&(Arc::as_ptr(identity) as usize));
            drop(waker);
        }
    }
}
/// Wakes the waiters taken out of the state, outside its lock.
fn wake_all(waiters: HashMap<usize, Waker>) {
    for waiter in waiters.into_values() {
        waiter.wake();
    }
}
struct Shared<M> {
    stop_hook: Mutex<Option<StopHook>>,
    validator: Option<MessageValidator<M>>,
    state: Mutex<State<M>>,
    waker: Arc<dyn Wake>,
    stop_requested: AtomicBool,
    /// Live ordinary-message permits, mirrored from `State::messages` under the state lock so
    /// the owner can ask whether anything is admitted without taking the lock.
    messages_live: AtomicUsize,
    /// Live operations (reserved, pending, ready or dispatching), mirrored like `messages_live`.
    operations_live: AtomicUsize,
    /// A fault may be stored in `State::fault`. Set at every site that stores one.
    ///
    /// Fix: the owner locked the state once per loop slice only to find no fault.
    /// Decision: `Transport::take_fault` reads this flag and locks only when it is set.
    faulted: AtomicBool,
    /// The owner's last `Transport::arm` succeeded and no `Transport::awake` followed.
    ///
    /// Fix: the owner locked the state in every slice to move the gate from `Armed` to
    /// `Awake`, also in slices that never armed it. Decision: only the owner arms, so it
    /// records that here and `awake` locks only after a successful `arm`. Tried and
    /// dropped: the gate as an atomic with compare-and-swap; producers read the gate
    /// together with the queues under the state lock, and that pairing is what makes a
    /// publication either see `Armed` or be seen by `arm`.
    armed: AtomicBool,
    /// Notifications or controls may be queued for the owner; set by every publication
    /// and recomputed by `Transport::import`. Read without the lock by the owner's spin
    /// before its idle wait (`Transport::published`).
    published: AtomicBool,
}
pub(crate) struct Transport<M: Send + 'static> {
    shared: Arc<Shared<M>>,
    /// Payloads of imported messages, indexed by message slot, each with the generation
    /// of its key.
    ///
    /// Fix: the owner took the state lock once more per message to fetch the payload of a
    /// delivery it had just imported under that same lock. Decision: `import` moves the
    /// payload here while it holds the lock, and `message` takes it without locking.
    /// The vector grows to the highest slot imported.
    imported: std::cell::RefCell<Vec<Option<(u64, M)>>>,
}
struct Endpoint<M: Send + 'static>(Arc<Shared<M>>);
struct Publisher<M: Send + 'static> {
    shared: Arc<Shared<M>>,
    id: OperationId,
    allowance: usize,
}
fn fault(message: &str) -> FailureReport {
    FailureReport::new(FailureClass::Host, FailureCode::EngineFailure, message)
}
impl<M> Shared<M> {
    fn lock(&self) -> MutexGuard<'_, State<M>> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.gate = Gate::Faulted;
                state.fault.get_or_insert_with(|| {
                    FailureReport::new(
                        FailureClass::Host,
                        FailureCode::HostPanic,
                        "transport mutex poisoned",
                    )
                });
                self.faulted.store(true, Ordering::Release);
                state
            }
        }
    }
    /// Mirrors the arena lengths into the lock-free counters.
    ///
    /// Fix: `has_tracked_work` took the lock for a full `stats()` every loop tick. Decision:
    /// call this at every site that inserts into or removes from `messages` or `operations`,
    /// while still holding the lock, so a counter never disagrees with the arena for longer
    /// than the critical section.
    ///
    /// # Arguments
    ///
    /// * `state` - the locked state whose arena lengths are published
    fn sync_counts(&self, state: &State<M>) {
        self.messages_live
            .store(state.messages.len(), Ordering::Release);
        self.operations_live
            .store(state.operations.len(), Ordering::Release);
    }
    /// Claims the wake of an armed owner: moves the gate to `Awake` and counts the wake.
    ///
    /// Fix: producers made the wake system call while holding the state mutex, so every
    /// other producer and the owner's own import, finish and arm queued behind a kernel
    /// call. Decision: the gate transition stays under the lock (it is what makes
    /// exactly one producer wake an armed owner), and the call itself is made by
    /// [`Self::deliver_wake`] after the guard is dropped.
    ///
    /// # Arguments
    /// * `state` - The locked state.
    ///
    /// # Returns
    /// Whether the caller must call [`Self::deliver_wake`] after unlocking.
    fn claim_wake(&self, state: &mut State<M>) -> bool {
        // Every publication calls this right after it queued its entry.
        self.published.store(true, Ordering::Release);
        if state.gate != Gate::Armed {
            return false;
        }
        state.wakes = state.wakes.saturating_add(1);
        state.gate = Gate::Awake;
        true
    }
    /// Makes the wake call claimed by [`Self::claim_wake`]; call it without the state lock.
    ///
    /// # Arguments
    /// * `claimed` - The result of `claim_wake`.
    ///
    /// # Errors
    /// The waker failed. The gate is `Armed` again unless the owner or a stop moved it
    /// meanwhile, so a later publication retries the wake; the caller undoes or faults its
    /// own publication.
    fn deliver_wake(&self, claimed: bool) -> Result<(), FailureReport> {
        if !claimed {
            return Ok(());
        }
        self.waker.wake().map_err(|error| {
            let mut state = self.lock();
            if state.gate == Gate::Awake {
                state.gate = Gate::Armed;
            }
            fault(&format!("transport wake failed: {error}"))
        })
    }
    /// Stores `error` as the transport fault and closes the gate.
    ///
    /// # Arguments
    /// * `error` - The fault the owner loop reports.
    fn latch_fault(&self, error: FailureReport) {
        let mut state = self.lock();
        state.fault = Some(error);
        state.gate = Gate::Faulted;
        self.faulted.store(true, Ordering::Release);
    }
    fn operation_key(state: &State<M>, id: OperationId) -> Option<Key> {
        (id.epoch == state.epoch && id.context == state.epoch).then_some(Key {
            slot: id.slot,
            generation: id.generation,
        })
    }
    fn rollback(&self, id: OperationId) {
        let removed = {
            let mut state = self.lock();
            Self::operation_key(&state, id).and_then(|key| {
                if !state
                    .operations
                    .get(key)
                    .is_some_and(|slot| matches!(slot.state, OperationState::Reserved))
                {
                    return None;
                }
                let slot = state.operations.remove(key)?;
                state.reserved_bytes -= slot.bytes;
                self.sync_counts(&state);
                Some(slot)
            })
        };
        drop(removed);
    }
    fn terminal(&self, id: OperationId, kind: Terminal) -> CancelOutcome {
        let mut state = self.lock();
        let Some(key) = Self::operation_key(&state, id) else {
            return CancelOutcome::WrongOwner;
        };
        if matches!(state.gate, Gate::Closed | Gate::Faulted) {
            return CancelOutcome::NotFound;
        }
        let Some(slot) = state.operations.get_mut(key) else {
            return CancelOutcome::NotFound;
        };
        match slot.state {
            OperationState::Reserved => return CancelOutcome::NotFound,
            OperationState::Ready(_) => return CancelOutcome::TooLate,
            OperationState::Dispatching => return CancelOutcome::Running,
            OperationState::Pending => {}
        }
        let payload = match kind {
            Terminal::Canceled => {
                slot.canceled = true;
                terminal_payload("Canceled")
            }
            Terminal::Abandoned => terminal_payload("Abandoned"),
        };
        assert!(slot.state.decide(payload));
        let node = state.message_capacity + key.slot as usize;
        state.notifications.push(node, Delivery::Completion(key));
        let claimed = self.claim_wake(&mut state);
        drop(state);
        if let Err(error) = self.deliver_wake(claimed) {
            self.latch_fault(error);
        }
        CancelOutcome::Canceled
    }
}
#[derive(Clone, Copy)]
enum Terminal {
    Canceled,
    Abandoned,
}
impl<M: Send + 'static> LoopHandle<M> {
    /// Loop identity used by the native backend to reject foreign delivery targets.
    pub(crate) fn epoch(&self) -> u64 {
        self.shared.lock().epoch
    }
    /// Publish ownership once, returning the original message on rejection.
    /// A failed armed wake rolls this ordinary enqueue back under the same lock.
    pub fn try_send(&self, message: M) -> Result<(), Rejected<M>> {
        self.try_send_observed(message, None, true)
    }
    /// [`Self::try_send`] without the admission validator, for a caller inside this crate
    /// that already checked the message against the same limits.
    ///
    /// # Arguments
    /// * `message` - The checked message.
    ///
    /// # Errors
    /// As `try_send`, except the validator's rejections.
    #[allow(
        clippy::result_large_err,
        reason = "a rejection hands the refused value back with its report"
    )]
    pub(crate) fn try_send_unvalidated(&self, message: M) -> Result<(), Rejected<M>> {
        self.try_send_observed(message, None, false)
    }
    // Capture the epoch under the same lock that decided Full. This avoids an
    // extra lock on the successful path and closes the race before waiting.
    fn try_send_observed(
        &self,
        message: M,
        on_full: Option<&mut u64>,
        validated: bool,
    ) -> Result<(), Rejected<M>> {
        if self.shared.stop_requested.load(Ordering::Acquire) {
            return Err(Rejected {
                value: message,
                reason: RejectionReason::Closed,
            });
        }
        if validated
            && let Some(validate) = &self.shared.validator
            && let Err(reason) = validate(&message)
        {
            return Err(Rejected {
                value: message,
                reason,
            });
        }
        let mut state = self.shared.lock();
        if state.stop || matches!(state.gate, Gate::Closed | Gate::Faulted) {
            return Err(Rejected {
                value: message,
                reason: RejectionReason::Closed,
            });
        }
        let key = state
            .messages
            .insert(MessageSlot {
                value: Some(message),
            })
            .map_err(|slot| {
                if let Some(epoch) = on_full {
                    *epoch = state.capacity_epoch;
                }
                Rejected {
                    value: slot.value.expect("new message"),
                    reason: RejectionReason::Full,
                }
            })?;
        self.shared.sync_counts(&state);
        state
            .notifications
            .push(key.slot as usize, Delivery::Message(key));
        let claimed = self.shared.claim_wake(&mut state);
        drop(state);
        if let Err(error) = self.shared.deliver_wake(claimed) {
            // The wake ran unlocked: the message is taken back only while it is still
            // queued. An owner that woke for another reason and imported it has it.
            let mut state = self.shared.lock();
            if state.notifications.remove(key.slot as usize).is_some()
                && let Some(message) = state.messages.remove(key).and_then(|slot| slot.value)
            {
                self.shared.sync_counts(&state);
                return Err(Rejected {
                    value: message,
                    reason: RejectionReason::WakeFailed(error),
                });
            }
        }
        Ok(())
    }
    /// Moves an ordinary message to the owner, waiting without polling while full.
    ///
    /// Success means admitted, not dispatched. A rejection other than `Full`
    /// returns the original message, including when stop closes the inbox.
    /// Canceling a pending future drops its not-yet-admitted message.
    pub async fn send(&self, mut message: M) -> Result<(), Rejected<M>> {
        loop {
            let mut seen = 0;
            match self.try_send_observed(message, Some(&mut seen), true) {
                Ok(()) => return Ok(()),
                Err(Rejected {
                    value,
                    reason: RejectionReason::Full,
                }) => {
                    message = value;
                    self.capacity(seen).await;
                }
                Err(rejected) => return Err(rejected),
            }
        }
    }
    /// Admit cancellation on its separate bounded control lane; this is not an
    /// acknowledgement that the owner has applied it yet.
    #[allow(
        clippy::result_large_err,
        reason = "a rejection hands the refused value back with its report"
    )]
    pub fn request_cancel(&self, task: TaskId) -> Result<(), Rejected<TaskId>> {
        let mut state = self.shared.lock();
        let reason = if state.stop || matches!(state.gate, Gate::Closed | Gate::Faulted) {
            Some(RejectionReason::Closed)
        } else if task.epoch != state.epoch {
            Some(RejectionReason::WrongOwner)
        } else if state.controls.len() == state.control_limit {
            Some(RejectionReason::Full)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(Rejected {
                value: task,
                reason,
            });
        }
        state.controls.push_back(task);
        let claimed = self.shared.claim_wake(&mut state);
        drop(state);
        if let Err(error) = self.shared.deliver_wake(claimed) {
            // The wake ran unlocked: the request is taken back only while it is still
            // the newest queued one.
            let mut state = self.shared.lock();
            if state.controls.back() == Some(&task) {
                state.controls.pop_back();
                return Err(Rejected {
                    value: task,
                    reason: RejectionReason::WakeFailed(error),
                });
            }
        }
        Ok(())
    }
    /// Close admission and request stop without consuming any queue capacity.
    /// A wake or native stop-hook error is returned after the stop flag is
    /// published and retained for the owner pump, including callers whose local
    /// stop interface cannot return an error. Hooks run outside publication locks.
    pub fn request_stop(&self) -> Result<(), FailureReport> {
        let mut state = self.shared.lock();
        state.stop = true;
        self.shared.stop_requested.store(true, Ordering::Release);
        let claimed = self.shared.claim_wake(&mut state);
        let waiters = std::mem::take(&mut state.capacity_waiters);
        drop(state);
        let wake = self.shared.deliver_wake(claimed);
        wake_all(waiters);
        let hook = self
            .shared
            .stop_hook
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let native = hook.map_or(Ok(()), |hook| hook());
        let result = wake.and(native);
        if let Err(error) = &result {
            let report = error.clone().bounded();
            self.shared.lock().fault.get_or_insert(report);
            self.shared.faulted.store(true, Ordering::Release);
        }
        result
    }
    /// Observe urgent stop without acquiring the application-publication lock.
    pub(crate) fn stop_requested(&self) -> bool {
        self.shared.stop_requested.load(Ordering::Acquire)
    }
    /// Install copied/revocable debug control metadata before application entry.
    pub(crate) fn install_stop_hook(&self, hook: StopHook) {
        *self
            .shared
            .stop_hook
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(hook);
    }
    /// A weak OS-wake callback avoids a transport/debug-control Arc cycle.
    pub(crate) fn wake_callback(&self) -> Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync> {
        let shared = Arc::downgrade(&self.shared);
        Arc::new(move || {
            let shared = shared
                .upgrade()
                .ok_or_else(|| fault("owner transport no longer exists"))?;
            let claimed = shared.claim_wake(&mut shared.lock());
            shared.deliver_wake(claimed)
        })
    }
    /// A future that resolves once the capacity epoch differs from `seen` (read it from
    /// [`Self::stats`] before the `try_send` that came back `Full`), or the transport stops.
    pub fn capacity(&self, seen: u64) -> CapacityChange<M> {
        CapacityChange {
            shared: self.shared.clone(),
            seen,
            identity: None,
        }
    }
    /// Read transport counters without retaining any native engine state.
    pub fn stats(&self) -> TransportStats {
        let state = self.shared.lock();
        TransportStats {
            messages: state.messages.len(),
            operations: state.operations.len(),
            reserved_payload_bytes: state.reserved_bytes,
            wake_calls: state.wakes,
            capacity_epoch: state.capacity_epoch,
        }
    }
}
impl<M: Send + 'static> Transport<M> {
    pub fn new(
        epoch: u64,
        config: &SchedulerConfig,
        waker: Arc<dyn Wake>,
    ) -> Result<Self, FailureReport> {
        let fail = || FailureReport::fault(FailureCode::Capacity, "transport storage unavailable");
        let capacity = config
            .max_messages
            .checked_add(config.max_operations)
            .ok_or_else(fail)?;
        let mut controls = VecDeque::new();
        controls
            .try_reserve_exact(config.max_controls)
            .map_err(|_| fail())?;
        Ok(Self {
            shared: Arc::new(Shared {
                stop_hook: Mutex::new(None),
                validator: None,
                waker,
                stop_requested: AtomicBool::new(false),
                messages_live: AtomicUsize::new(0),
                operations_live: AtomicUsize::new(0),
                faulted: AtomicBool::new(false),
                armed: AtomicBool::new(false),
                published: AtomicBool::new(false),
                state: Mutex::new(State {
                    epoch,
                    gate: Gate::Awake,
                    stop: false,
                    fault: None,
                    messages: Arena::new(config.max_messages).map_err(|_| fail())?,
                    operations: Arena::new(config.max_operations).map_err(|_| fail())?,
                    notifications: ReadyQueue::new(capacity).map_err(|_| fail())?,
                    controls,
                    control_limit: config.max_controls,
                    reserved_bytes: 0,
                    max_bytes: config.reserved_payload_bytes,
                    message_capacity: config.max_messages,
                    wakes: 0,
                    capacity_epoch: 0,
                    capacity_waiters: HashMap::new(),
                }),
            }),
            imported: std::cell::RefCell::new(Vec::new()),
        })
    }
    pub fn handle(&self) -> LoopHandle<M> {
        LoopHandle {
            shared: self.shared.clone(),
        }
    }
    /// Set immutable admission metadata before exposing any handles or endpoints.
    pub fn with_validator(mut self, validator: Option<MessageValidator<M>>) -> Self {
        Arc::get_mut(&mut self.shared)
            .expect("unshared transport construction")
            .validator = validator;
        self
    }
    pub fn control(&self) -> std::rc::Rc<dyn HostControl> {
        std::rc::Rc::new(Endpoint(self.shared.clone()))
    }
    pub fn stopped(&self) -> bool {
        self.shared.stop_requested.load(Ordering::Acquire)
    }
    /// Takes the stored transport fault, if any.
    ///
    /// # Returns
    /// The fault, or `None`; without locking when no site has stored one.
    pub fn take_fault(&self) -> Option<FailureReport> {
        if !self.shared.faulted.swap(false, Ordering::AcqRel) {
            return None;
        }
        self.shared.lock().fault.take()
    }
    /// Move scalar notification keys in one lock acquisition into reused buffers.
    pub fn import(&self, batch: usize, deliveries: &mut Vec<Delivery>, controls: &mut Vec<TaskId>) {
        let mut state = self.shared.lock();
        for _ in 0..batch {
            let Some(control) = state.controls.pop_front() else {
                break;
            };
            controls.push(control);
        }
        for _ in 0..batch {
            let Some(delivery) = state.notifications.pop() else {
                break;
            };
            if let Delivery::Message(key) = delivery
                && let Some(value) = state
                    .messages
                    .get_mut(key)
                    .and_then(|slot| slot.value.take())
            {
                let mut imported = self.imported.borrow_mut();
                let slot = key.slot as usize;
                if slot >= imported.len() {
                    imported.resize_with(slot + 1, || None);
                }
                imported[slot] = Some((key.generation, value));
            }
            deliveries.push(delivery);
        }
        self.shared.published.store(
            state.notifications.len() != 0 || !state.controls.is_empty(),
            Ordering::Release,
        );
    }
    /// Whether a publication may be waiting for [`Self::import`], without locking.
    ///
    /// # Returns
    /// `true` after a publication that no import has taken yet; may also be `true` with
    /// nothing queued (a bare wake request), never `false` with something queued.
    pub fn published(&self) -> bool {
        self.shared.published.load(Ordering::Acquire)
    }
    /// Takes the payload of an imported message.
    ///
    /// # Arguments
    /// * `key` - The key of a `Delivery::Message` returned by [`Self::import`].
    ///
    /// # Returns
    /// The payload, or `None` for a stale key or a payload already taken.
    pub fn message(&self, key: Key) -> Option<M> {
        let mut imported = self.imported.borrow_mut();
        let entry = imported.get_mut(key.slot as usize)?;
        if entry.as_ref()?.0 != key.generation {
            return None;
        }
        entry.take().map(|(_, value)| value)
    }
    pub fn completion(&self, key: Key) -> Option<(OperationId, CompletionPayload)> {
        let mut state = self.shared.lock();
        let epoch = state.epoch;
        let slot = state.operations.get_mut(key)?;
        let OperationState::Ready(payload) = &mut slot.state else {
            return None;
        };
        let payload = payload.take()?;
        slot.state = OperationState::Dispatching;
        Some((
            OperationId {
                epoch,
                context: epoch,
                slot: key.slot,
                generation: key.generation,
            },
            payload,
        ))
    }
    /// Reports whether any ordinary message or operation is admitted, without taking the lock.
    ///
    /// Fix: the owner's tracked-work query locked the state for a full [`TransportStats`]
    /// snapshot on every loop tick. Decision: read the counters `Shared::sync_counts` keeps
    /// current under the lock. The answer is a snapshot like the old one: a producer that
    /// publishes concurrently may be seen on the next query.
    ///
    /// # Returns
    ///
    /// `true` when `messages` or `operations` of [`TransportStats`] would be nonzero.
    pub fn has_admitted_work(&self) -> bool {
        self.shared.messages_live.load(Ordering::Acquire) != 0
            || self.shared.operations_live.load(Ordering::Acquire) != 0
    }
    /// Releases a delivered message permit or operation slot and signals capacity waiters.
    ///
    /// Fix: every delivery `mem::take`d the waiter map, building a fresh `HashMap` (and its
    /// `RandomState`) even with no waiter. Decision: take it only when it is non-empty. The
    /// capacity epoch is still bumped unconditionally: a producer that read the epoch at `Full`
    /// and has not yet registered a waiter relies on the epoch differing to resolve at its
    /// first poll, so bumping only when a waiter exists would lose that wakeup.
    ///
    /// # Arguments
    ///
    /// * `delivery` - the delivery whose slot is released
    pub fn finish(&self, delivery: Delivery) {
        let (message, operation, waiters) = {
            let mut state = self.shared.lock();
            let result = match delivery {
                Delivery::Message(key) => (state.messages.remove(key), None),
                Delivery::Completion(key) => {
                    let slot = state.operations.remove(key);
                    if let Some(slot) = &slot {
                        state.reserved_bytes -= slot.bytes;
                    }
                    (None, slot)
                }
            };
            if let Some(next) = state.capacity_epoch.checked_add(1) {
                state.capacity_epoch = next;
            } else {
                state.fault = Some(fault("delivery capacity epoch exhausted"));
                state.gate = Gate::Faulted;
                self.shared.faulted.store(true, Ordering::Release);
            }
            self.shared.sync_counts(&state);
            let waiters = if state.capacity_waiters.is_empty() {
                None
            } else {
                Some(std::mem::take(&mut state.capacity_waiters))
            };
            (result.0, result.1, waiters)
        };
        drop(message);
        drop(operation);
        if let Some(waiters) = waiters {
            wake_all(waiters);
        }
    }
    /// Recheck shared backlog and atomically arm the owner immediately before poll.
    pub fn arm(&self) -> bool {
        let mut state = self.shared.lock();
        let backlog = state.stop || state.notifications.len() != 0 || !state.controls.is_empty();
        let armed = state.gate.arm(backlog);
        if armed {
            self.shared.armed.store(true, Ordering::Relaxed);
        }
        armed
    }
    /// Returns the gate from `Armed` to `Awake` after the owner's wait.
    ///
    /// Locks only when the owner armed the gate since the last call; see `Shared::armed`.
    pub fn awake(&self) {
        if self.shared.armed.swap(false, Ordering::Relaxed) {
            self.shared.lock().gate.awake();
        }
    }
    pub fn close(&self) {
        let waiters = {
            let mut state = self.shared.lock();
            state.stop = true;
            self.shared.stop_requested.store(true, Ordering::Release);
            state.gate = Gate::Closed;
            state.controls.clear();
            std::mem::take(&mut state.capacity_waiters)
        };
        wake_all(waiters);
        let hook = self
            .shared
            .stop_hook
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        drop(hook);
        // Imported and undelivered payloads are queued messages too: each is dropped on
        // its own, outside the lock, and a panicking destructor becomes the same fault.
        let imported = std::mem::take(&mut *self.imported.borrow_mut());
        for payload in imported.into_iter().flatten() {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload))).is_err() {
                let mut state = self.shared.lock();
                state.fault.get_or_insert_with(|| {
                    FailureReport::new(
                        FailureClass::Host,
                        FailureCode::HostPanic,
                        "queued message destructor panicked",
                    )
                });
                self.shared.faulted.store(true, Ordering::Release);
            }
        }
        loop {
            let removed = {
                let mut state = self.shared.lock();
                let key = state.messages.keys().next();
                key.and_then(|key| {
                    state.notifications.remove(key.slot as usize);
                    let removed = state.messages.remove(key);
                    self.shared.sync_counts(&state);
                    removed
                })
            };
            if removed.is_none() {
                break;
            }
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(removed))).is_err() {
                let mut state = self.shared.lock();
                state.fault.get_or_insert_with(|| {
                    FailureReport::new(
                        FailureClass::Host,
                        FailureCode::HostPanic,
                        "queued message destructor panicked",
                    )
                });
                self.shared.faulted.store(true, Ordering::Release);
            }
        }
        loop {
            let removed = {
                let mut state = self.shared.lock();
                let key = state.operations.keys().next();
                key.and_then(|key| {
                    let node = state.message_capacity + key.slot as usize;
                    state.notifications.remove(node);
                    let slot = state.operations.remove(key)?;
                    state.reserved_bytes -= slot.bytes;
                    self.shared.sync_counts(&state);
                    Some(slot)
                })
            };
            if removed.is_none() {
                break;
            }
            drop(removed);
        }
    }
}
impl<M: Send + 'static> HostControl for Endpoint<M> {
    fn stopping(&self) -> bool {
        self.0.stop_requested.load(Ordering::Acquire)
    }
    fn reserve(&self, bytes: usize) -> Result<OperationReservation, FailureReport> {
        let slot = OperationSlot {
            state: OperationState::Reserved,
            bytes,
            canceled: false,
        };
        let id = {
            let mut state = self.0.lock();
            if state.stop || matches!(state.gate, Gate::Closed | Gate::Faulted) {
                return Err(FailureReport::fault(
                    FailureCode::ClosedContext,
                    "transport closed",
                ));
            }
            let Some(total) = state
                .reserved_bytes
                .checked_add(bytes)
                .filter(|&total| bytes != 0 && total <= state.max_bytes)
            else {
                return Err(FailureReport::fault(
                    FailureCode::Capacity,
                    "operation payload reservation full",
                ));
            };
            let key = state
                .operations
                .insert(slot)
                .map_err(|_| FailureReport::fault(FailureCode::Capacity, "operation slots full"))?;
            state.reserved_bytes = total;
            self.0.sync_counts(&state);
            OperationId {
                epoch: state.epoch,
                context: state.epoch,
                slot: key.slot,
                generation: key.generation,
            }
        };
        let shared = self.0.clone();
        Ok(OperationReservation {
            id,
            rollback: Some(Box::new(move || shared.rollback(id))),
        })
    }
    fn bind(
        &self,
        mut reservation: OperationReservation,
    ) -> Result<CompletionTicket, FailureReport> {
        let id = reservation.id();
        let allowance = {
            let mut state = self.0.lock();
            if state.stop || matches!(state.gate, Gate::Closed | Gate::Faulted) {
                return Err(FailureReport::fault(
                    FailureCode::ClosedContext,
                    "transport closed",
                ));
            }
            let key = Shared::operation_key(&state, id).ok_or_else(|| {
                FailureReport::fault(FailureCode::WrongOwner, "foreign reservation")
            })?;
            let slot = state
                .operations
                .get_mut(key)
                .filter(|slot| matches!(slot.state, OperationState::Reserved))
                .ok_or_else(|| {
                    FailureReport::fault(FailureCode::InvalidHandle, "stale reservation")
                })?;
            slot.state = OperationState::Pending;
            slot.bytes
        };
        reservation.rollback.take();
        Ok(CompletionTicket {
            id,
            publisher: Some(Box::new(Publisher {
                shared: self.0.clone(),
                id,
                allowance,
            })),
        })
    }
    fn cancel(&self, id: OperationId) -> CancelOutcome {
        self.0.terminal(id, Terminal::Canceled)
    }
    fn stop(&self) -> Result<(), FailureReport> {
        LoopHandle {
            shared: self.0.clone(),
        }
        .request_stop()
    }
}
fn payload_fits(payload: &CompletionPayload, allowance: usize) -> bool {
    let limits = ExportLimits {
        max_bytes: allowance,
        ..Default::default()
    };
    match payload {
        CompletionPayload::Success(data) => data.footprint(limits).is_ok(),
        CompletionPayload::Failure(error) => {
            let own = std::mem::size_of::<OperationFailure>()
                .checked_add(error.code.capacity())
                .and_then(|n| n.checked_add(error.message.capacity_bytes()));
            own.and_then(|own| {
                error.details.as_ref().map_or(Some(own), |data| {
                    data.footprint(limits)
                        .ok()
                        .and_then(|bytes| own.checked_add(bytes))
                })
            })
            .is_some_and(|bytes| bytes <= allowance)
        }
    }
}
impl<M: Send + 'static> CompletionPublisher for Publisher<M> {
    fn complete(self: Box<Self>, payload: CompletionPayload) -> PublishOutcome {
        // Native/user data traversal happens before acquiring the publication lock.
        let valid = payload_fits(&payload, self.allowance);
        let mut state = self.shared.lock();
        let reject = if state.stop || matches!(state.gate, Gate::Closed | Gate::Faulted) {
            Some(PublishRejection::Closed)
        } else if Shared::operation_key(&state, self.id)
            .and_then(|key| state.operations.get(key))
            .is_none()
        {
            Some(PublishRejection::Stale)
        } else {
            None
        };
        if let Some(reason) = reject {
            return PublishOutcome::Rejected { reason, payload };
        }
        let key = Shared::operation_key(&state, self.id).expect("validated operation");
        let slot = state.operations.get_mut(key).expect("validated operation");
        if !matches!(slot.state, OperationState::Pending) {
            return PublishOutcome::Rejected {
                reason: if slot.canceled {
                    PublishRejection::Canceled
                } else {
                    PublishRejection::Stale
                },
                payload,
            };
        }
        let original = if valid {
            assert!(slot.state.decide(Some(payload)));
            None
        } else {
            assert!(slot.state.decide(terminal_payload("PayloadTooLarge")));
            Some(payload)
        };
        let node = state.message_capacity + key.slot as usize;
        state.notifications.push(node, Delivery::Completion(key));
        let claimed = self.shared.claim_wake(&mut state);
        drop(state);
        let wake = self.shared.deliver_wake(claimed);
        if let Err(error) = &wake {
            self.shared.latch_fault(error.clone());
        }
        if let Some(payload) = original {
            return PublishOutcome::Rejected {
                reason: PublishRejection::PayloadTooLarge,
                payload,
            };
        }
        match wake {
            Ok(()) => PublishOutcome::Published,
            Err(error) => PublishOutcome::PublishedWakeFailed { error },
        }
    }
    fn abandon(self: Box<Self>) {
        self.shared.terminal(self.id, Terminal::Abandoned);
    }
}
