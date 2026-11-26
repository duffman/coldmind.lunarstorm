//! Owner-thread adapter bridge: normal turns, retained jobs and reserved bindings.
use super::*;
use crate::{
    debug::{
        AsyncScheduleKind, AsyncStackId, DebugBackend, DebugControlReceipt, DebugCoordinator,
        DebugDescriptor, DebugError, DebugExecutionState, DebugLimits, DebugMode,
        DebugSessionOptions, VmDebugHandle,
    },
    scheduler::{LocalHost, LoopDriver},
    transport::{LoopHandle, MessageValidator},
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    fmt,
    num::NonZeroUsize,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

/// One host request for the engine's memory, answered at the next owner-safe boundary.
struct MemoryRequest {
    /// Collect garbage before measuring.
    collect: bool,
    /// Where the heap usage (or the failure) goes.
    reply: mpsc::Sender<Result<HeapUsage, FailureReport>>,
}

const MAX_MEMORY_REQUESTS: usize = 128;
const MEMORY_REQUEST_BATCH: usize = 16;

/// Wakes the owner loop from another thread.
type Wake = Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync>;

/// Mailbox state, guarded by [`MemoryRequests::state`].
#[derive(Default)]
struct MemoryState {
    /// Requests not yet served, oldest first.
    queue: VecDeque<MemoryRequest>,
    /// Set by [`MemoryRequests::close`]; later requests are refused.
    closed: bool,
    /// The owner loop's wake, connected in `initialize`; `None` before the loop starts.
    wake: Option<Wake>,
}

/// Cross-thread mailbox for host memory requests (`VmHandle::collect_garbage`,
/// `VmHandle::heap_usage`).
///
/// Decisions:
/// - The owning bridge serves requests in `service_control`, between turns and never inside
///   running JavaScript: engines are owner-thread only, and an owner-safe boundary is where
///   the debugger's cross-thread requests are served too.
/// - Closing the mailbox (bridge shutdown or drop) drops every waiting reply, so a host
///   blocked on one sees `Disconnected` instead of waiting forever.
#[derive(Default)]
pub(crate) struct MemoryRequests {
    state: Mutex<MemoryState>,
    /// Requests are queued; read without the lock by `has_control_work`.
    pending: AtomicBool,
}
impl MemoryRequests {
    /// Queues one request and wakes the owner loop.
    ///
    /// # Arguments
    /// * `collect` - Collect garbage before measuring the heap.
    ///
    /// # Returns
    /// The receiver for the answer, or `Capacity` if the bounded mailbox is full.
    /// It disconnects without an answer when the VM has ended.
    pub(crate) fn request(
        &self,
        collect: bool,
    ) -> mpsc::Receiver<Result<HeapUsage, FailureReport>> {
        let (reply, answer) = mpsc::channel();
        let wake = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.closed {
                // `reply` drops here: the host sees `Disconnected`.
                return answer;
            }
            if state.queue.len() == MAX_MEMORY_REQUESTS || state.queue.try_reserve(1).is_err() {
                let _ = reply.send(Err(FailureReport::fault(
                    FailureCode::Capacity,
                    "memory request mailbox is full",
                )));
                return answer;
            }
            state.queue.push_back(MemoryRequest { collect, reply });
            self.pending.store(true, Ordering::Release);
            state.wake.clone()
        };
        // Before the loop starts there is no wake yet; `has_control_work` picks it up then.
        if let Some(wake) = wake {
            let _ = wake();
        }
        answer
    }
    /// Takes one bounded FIFO batch, retaining the pending flag for any backlog.
    ///
    /// # Returns
    /// At most `MEMORY_REQUEST_BATCH` queued requests, oldest first.
    fn take(&self) -> Vec<MemoryRequest> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let count = state.queue.len().min(MEMORY_REQUEST_BATCH);
        let requests = state.queue.drain(..count).collect();
        self.pending
            .store(!state.queue.is_empty(), Ordering::Release);
        requests
    }
    /// Refuses further requests and drops the queued ones' replies, so every waiting host
    /// sees `Disconnected`.
    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        state.queue.clear();
        state.wake = None;
        self.pending.store(false, Ordering::Release);
    }
}

/// Owner-local work dispatched through the single application scheduler.
pub enum EngineJob {
    /// Invoke a retained callback with its retained receiver and arguments.
    Callback(CallbackJob),
    /// Callback with copied debugger scheduling metadata; owns the same callback roots.
    TracedCallback {
        /// Original admitted callback, preserved on rejection.
        job: CallbackJob,
        /// Context-checked scheduling history, never a native frame or JS value.
        history: AsyncStackId,
    },
    /// Execute owned scoped host work as one turn, without recursively pumping.
    Work(Box<dyn ScopeCall>),
    /// Opaque synchronous command created by the public VM facade.
    Command(EngineCommand),
    /// Source execution with detached result disposal, admitted by VM.submit.
    Submit(ScriptSource),
}
#[allow(
    clippy::result_large_err,
    reason = "Failed projections preserve the original owned job without allocating or dropping its roots."
)]
impl EngineJob {
    /// Recovers callback ownership, returning other job variants unchanged.
    fn into_callback(self) -> Result<CallbackJob, Self> {
        match self {
            Self::Callback(job) | Self::TracedCallback { job, .. } => Ok(job),
            other => Err(other),
        }
    }
    /// Recovers submitted source ownership, returning other variants unchanged.
    pub(crate) fn into_submit(self) -> Result<ScriptSource, Self> {
        match self {
            Self::Submit(source) => Ok(source),
            other => Err(other),
        }
    }
}
impl fmt::Debug for EngineJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Callback(job) => f.debug_tuple("Callback").field(job).finish(),
            Self::TracedCallback { job, history } => f
                .debug_struct("TracedCallback")
                .field("job", job)
                .field("history", history)
                .finish(),
            Self::Work(_) => f.write_str("Work(owned scoped body)"),
            Self::Command(_) => f.write_str("Command(owner capture)"),
            Self::Submit(source) => f.debug_tuple("Submit").field(&source.origin).finish(),
        }
    }
}
// Restore debugger metadata even if a user adapter panics during the turn.
struct AsyncHistoryGuard {
    backend: Rc<dyn DebugBackend>,
    previous: Option<AsyncStackId>,
}
impl AsyncHistoryGuard {
    fn enter(backend: &Rc<dyn DebugBackend>, parent: Option<AsyncStackId>) -> Self {
        let previous = backend.async_parent();
        backend.set_async_parent(parent);
        Self {
            backend: backend.clone(),
            previous,
        }
    }
}
impl Drop for AsyncHistoryGuard {
    fn drop(&mut self) {
        self.backend.set_async_parent(self.previous);
    }
}
enum Target {
    Callback(RootedValue),
    Deferred(RootedDeferred),
}
struct Binding {
    target: Target,
    cancel: Option<Rc<dyn CancelNative>>,
    history: Option<AsyncStackId>,
}
/// Owner-local table of reserved-operation bindings, keyed by [`OperationId`].
///
/// Fix: a `HashMap<OperationId, Binding>` hashed the 28-byte id with SipHash on every bind
/// and every completion. Decision: the id carries the dense arena slot, so the table is a
/// `Vec` indexed by slot that stores the full id beside the binding and compares it on every
/// access (the generation check). A different id landing on an occupied slot (a stale
/// binding not yet removed when the slot is reused) goes to a small `spill` map, which keeps
/// the exact semantics of the old map: distinct ids never overwrite each other. Dense slots
/// grow on demand, bounded by the transport's operation capacity.
struct BindingTable {
    /// Binding by arena slot, with the id that owns it.
    dense: Vec<Option<(OperationId, Binding)>>,
    /// Bindings whose slot is held by another live id; empty in normal operation.
    spill: HashMap<OperationId, Binding>,
}
impl BindingTable {
    /// Creates an empty table with room for `capacity` slots.
    ///
    /// # Arguments
    /// * `capacity` - Slots to reserve up front.
    ///
    /// # Errors
    /// `Capacity` when the storage cannot be reserved.
    fn new(capacity: usize) -> Result<Self, FailureReport> {
        let mut dense = Vec::new();
        dense.try_reserve_exact(capacity).map_err(|_| {
            FailureReport::fault(FailureCode::Capacity, "binding storage unavailable")
        })?;
        Ok(Self {
            dense,
            spill: HashMap::new(),
        })
    }
    /// Stores the binding for `id`, replacing and dropping an earlier binding of the same id.
    ///
    /// # Arguments
    /// * `id` - The operation that owns the binding.
    /// * `binding` - The binding to keep.
    ///
    /// # Errors
    /// `Capacity` when the slot table cannot grow; the binding is dropped.
    fn insert(&mut self, id: OperationId, binding: Binding) -> Result<(), FailureReport> {
        let slot = id.slot as usize;
        if slot >= self.dense.len() {
            let extra = slot + 1 - self.dense.len();
            self.dense.try_reserve(extra).map_err(|_| {
                FailureReport::fault(FailureCode::Capacity, "binding storage unavailable")
            })?;
            self.dense.resize_with(slot + 1, || None);
        }
        let entry = &mut self.dense[slot];
        match entry {
            Some((owner, held)) if *owner == id => *held = binding,
            Some(_) => {
                self.spill.insert(id, binding);
            }
            None => *entry = Some((id, binding)),
        }
        Ok(())
    }
    /// Takes the binding of `id`.
    ///
    /// # Arguments
    /// * `id` - The operation whose binding to take.
    ///
    /// # Returns
    /// The binding, or `None` when `id` has none (including a stale generation).
    fn remove(&mut self, id: &OperationId) -> Option<Binding> {
        if let Some(entry) = self.dense.get_mut(id.slot as usize)
            && entry.as_ref().is_some_and(|(owner, _)| owner == id)
        {
            return entry.take().map(|(_, binding)| binding);
        }
        self.spill.remove(id)
    }
    /// Takes any one binding, for teardown.
    ///
    /// # Returns
    /// A binding, or `None` when the table is empty.
    fn take_any(&mut self) -> Option<Binding> {
        if let Some(entry) = self.dense.iter_mut().find(|entry| entry.is_some()) {
            return entry.take().map(|(_, binding)| binding);
        }
        let id = self.spill.keys().next().copied()?;
        self.spill.remove(&id)
    }
    /// Whether no binding is held; the unit tests' check that a closed bridge kept none.
    ///
    /// # Returns
    /// `true` when the table holds no binding.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.spill.is_empty() && self.dense.iter().all(Option::is_none)
    }
}
/// The one owning engine/context bridge beneath a VM or generic EventLoop.
///
/// Engine values and bindings remain owner-local. Only copied payloads and opaque
/// operation metadata enter the shared transport. Construction validates required
/// interfaces before application entry; failure closes its new context.
pub struct EngineBridge<E: JsEngine> {
    engine: E,
    context: ContextId,
    descriptor: EngineDescriptor,
    features: EngineFeatures,
    debug: Option<Rc<dyn DebugBackend>>,
    debug_descriptor: Option<DebugDescriptor>,
    limits: ContextLimits,
    handler: Option<Rc<dyn HostMessageHandler>>,
    bindings: RefCell<BindingTable>,
    closed: Cell<bool>,
    debug_enabled: bool,
    debug_limits: DebugLimits,
    fail_unhandled_rejections: bool,
    coordinator: Option<Rc<DebugCoordinator>>,
    control: Option<crate::transport::LoopHandle<JsData>>,
    /// The platform's `nextTick` drain, called around engine jobs at every checkpoint.
    tick_drain: RefCell<Option<RootedValue>>,
    /// Host memory requests, served in `service_control`.
    memory: Arc<MemoryRequests>,
    /// Set when script first stores an async context (`async_context_set`). Until then every
    /// host event would read `undefined` back, so `clear_async_context` is skipped.
    async_context_used: Cell<bool>,
    /// Set once the platform reports queued ticks (`tick_queued`): from then on the drain is
    /// called only while `tick_pending`. Unset, the drain runs at every checkpoint.
    tick_gated: Cell<bool>,
    /// A tick was queued since the drain last started; only meaningful while `tick_gated`.
    tick_pending: Cell<bool>,
}
impl<E: JsEngine> EngineBridge<E> {
    /// Create exactly one context and validate the requested profile.
    ///
    /// # Errors
    /// Returns detached owner, allocation, context or capability failures. Any
    /// created context is closed before a rejected construction returns.
    pub fn new(
        engine: E,
        limits: ContextLimits,
        requirements: EngineRequirements,
        handler: Option<Rc<dyn HostMessageHandler>>,
        max_operations: usize,
    ) -> Result<Self, FailureReport> {
        limits.validate()?;
        if engine.descriptor().owner != thread::current().id() {
            return Err(FailureReport::fault(
                FailureCode::WrongOwner,
                "engine belongs to another owner",
            ));
        }
        if max_operations == 0 {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "operation limit must be positive",
            ));
        }
        let bindings = BindingTable::new(max_operations)?;
        let debug_enabled = requirements.debug.mode == DebugMode::Development;
        let descriptor = engine.descriptor().clone();
        let context = engine.create_context(limits)?;
        let mut bridge = Self {
            engine,
            context,
            descriptor,
            features: EngineFeatures::default(),
            debug: None,
            debug_descriptor: None,
            limits,
            handler,
            bindings: RefCell::new(bindings),
            closed: Cell::new(false),
            debug_enabled,
            debug_limits: DebugLimits::default(),
            fail_unhandled_rejections: false,
            coordinator: None,
            control: None,
            tick_drain: RefCell::new(None),
            memory: Arc::default(),
            async_context_used: Cell::new(false),
            tick_gated: Cell::new(false),
            tick_pending: Cell::new(false),
        };
        bridge.debug = bridge.engine.debug_backend(context)?;
        bridge.debug_descriptor = bridge
            .debug
            .as_ref()
            .map(|backend| backend.descriptor().clone());
        let facade = BridgeHost {
            bridge: &bridge,
            local: None,
        };
        let mut features = EngineFeatures::default();
        bridge
            .engine
            .with_scope(context, &facade, &mut |scope: &dyn JsScope| {
                features.source = scope.source().is_some();
                features.compiler = scope.compiler().is_some();
                features.artifacts = scope.artifacts().is_some();
                if let Some(modules) = scope.modules() {
                    features.modules = true;
                    let optional = modules.features();
                    features.top_level_await = optional.top_level_await;
                    features.dynamic_import = optional.dynamic_import;
                }
                Ok(())
            })?;
        requirements.validate(&features, bridge.debug_descriptor.as_ref())?;
        bridge.features = features;
        Ok(bridge)
    }
    /// Immutable descriptor cached independently of native context lifetime.
    pub fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }
    /// Capabilities discovered through the same scoped API used for execution.
    pub fn features(&self) -> &EngineFeatures {
        &self.features
    }
    /// Owner-local context identity used to validate retained inputs.
    pub fn context(&self) -> ContextId {
        self.context
    }
    /// The mailbox other threads use to ask for a collection or the heap usage.
    ///
    /// # Returns
    /// A shared handle to this bridge's memory request mailbox.
    pub(crate) fn memory_requests(&self) -> Arc<MemoryRequests> {
        self.memory.clone()
    }
    /// Answers one bounded batch of host memory requests on the owner thread.
    ///
    /// A failed collection or measurement is the answer, not a loop failure.
    ///
    /// # Returns
    /// Whether any request was served.
    fn serve_memory(&self) -> bool {
        // Fix: the mailbox mutex was taken at every control service, about three times
        // per turn, with nothing queued. Decision: `request` sets `pending` under that
        // mutex before it wakes the loop, so an unset flag means an empty queue.
        if !self.memory.pending.load(Ordering::Acquire) {
            return false;
        }
        let requests = self.memory.take();
        let served = !requests.is_empty();
        for request in requests {
            if self
                .control
                .as_ref()
                .is_some_and(LoopHandle::stop_requested)
            {
                // Shutdown disconnects remaining replies; never begin another
                // optional engine operation after an urgent stop was published.
                break;
            }
            let collected = if request.collect {
                self.engine.collect_garbage(self.context)
            } else {
                Ok(())
            };
            let _ = request
                .reply
                .send(collected.and_then(|()| self.engine.heap_usage(self.context)));
        }
        served
    }
    /// Cached native-debug metadata, available after close.
    pub fn debug_descriptor(&self) -> Option<&DebugDescriptor> {
        self.debug_descriptor.as_ref()
    }
    /// Retained owner-local backend metadata/service interface for coordination.
    pub(crate) fn debug_backend(&self) -> Option<Rc<dyn DebugBackend>> {
        self.debug.clone()
    }
    /// Report whether ordinary messages have a fixed routing handler.
    pub fn accepts_messages(&self) -> bool {
        self.handler.is_some()
    }
    pub(crate) fn with_debug_limits(mut self, limits: DebugLimits) -> Self {
        self.debug_limits = limits;
        self
    }
    pub(crate) fn with_unhandled_rejections_failing(mut self, fail: bool) -> Self {
        self.fail_unhandled_rejections = fail;
        self
    }
    /// Native debug execution state, independent of owner pump lifecycle.
    pub fn debug_state(&self) -> DebugExecutionState {
        if self.closed.get() {
            DebugExecutionState::Closed
        } else {
            self.coordinator
                .as_ref()
                .map_or(DebugExecutionState::Detached, |coordinator| {
                    coordinator.state()
                })
        }
    }
    /// Attach one controller at an owner-safe setup boundary before execution.
    pub fn attach_debugger(
        &self,
        options: DebugSessionOptions,
    ) -> Result<VmDebugHandle, DebugError> {
        self.coordinator
            .as_ref()
            .ok_or(DebugError::Unsupported)?
            .attach(options)
    }
    fn traced_callback(&self, job: CallbackJob, kind: AsyncScheduleKind) -> EngineJob {
        match self
            .debug
            .as_ref()
            .and_then(|debug| debug.capture_async(kind))
        {
            Some(history) => EngineJob::TracedCallback { job, history },
            None => EngineJob::Callback(job),
        }
    }
    fn callback_turn(
        &self,
        host: &dyn LocalHost<EngineJob>,
        job: &CallbackJob,
        history: Option<AsyncStackId>,
    ) -> Result<(), FailureReport> {
        let _history = self
            .debug
            .as_ref()
            .map(|backend| AsyncHistoryGuard::enter(backend, history));
        self.enter(host, &mut |scope: &dyn JsScope| {
            clear_async_context(scope, self.async_context_used.get())?;
            let function = scope.local(&job.function).map_err(|e| scope.detach(e))?;
            let receiver = job
                .receiver
                .as_ref()
                .map(|value| scope.local(value))
                .transpose()
                .map_err(|e| scope.detach(e))?;
            let args = job
                .arguments
                .iter()
                .map(|value| scope.local(value))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| scope.detach(e))?;
            scope
                .call(function, receiver, &args)
                .map_err(|e| scope.detach(e))?;
            Ok(())
        })
    }
    fn enter(
        &self,
        local: &dyn LocalHost<EngineJob>,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        if self.closed.get() {
            return Err(FailureReport::fault(
                FailureCode::ClosedContext,
                "bridge closed",
            ));
        }
        if let Some(coordinator) = &self.coordinator {
            coordinator.before_entry().map_err(debug_failure)?;
        }
        let facade = BridgeHost {
            bridge: self,
            local: Some(local),
        };
        let result = self.engine.with_scope(self.context, &facade, body);
        if result
            .as_ref()
            .is_err_and(|report| report.code == FailureCode::Interrupted)
            && self
                .coordinator
                .as_ref()
                .is_some_and(|debug| debug.is_stopping())
            && let Some(control) = &self.control
        {
            // A debugger disconnect may request native stop without an ordinary
            // loop stop yet. Publish it before the typed unwind reaches the pump.
            control.request_stop()?;
        }
        result
    }
    /// Close pending bindings before closing the context. Repeated close is safe.
    /// Cleanup continues after a cancellation-hook failure and returns the first
    /// detached fault; hooks never run with a binding-table borrow held.
    pub fn close(&self) -> Result<(), FailureReport> {
        if self.closed.replace(true) {
            return Ok(());
        }
        drop(self.tick_drain.borrow_mut().take());
        let mut failure = None;
        if let Some(coordinator) = &self.coordinator
            && let Err(error) = cleanup("debugger cleanup panicked", || {
                coordinator.shutdown().map_err(debug_failure)
            })
        {
            failure.get_or_insert(error);
        }
        loop {
            let binding = self.bindings.borrow_mut().take_any();
            let Some(binding) = binding else {
                break;
            };
            if let Err(error) = cancel_native(binding.cancel.as_ref()) {
                failure.get_or_insert(error);
            }
            if let Err(error) = cleanup("binding destructor panicked", || {
                drop(binding);
                Ok(())
            }) {
                failure.get_or_insert(error);
            }
        }
        if let Err(error) = cleanup("context close panicked", || {
            self.engine.close_context(self.context)
        }) {
            failure.get_or_insert(error);
        }
        failure.map_or(Ok(()), Err)
    }
}
/// Contains one owner teardown step and bounds its detached error before later
/// steps continue. Every caller supplies the diagnostic for a contained panic.
fn cleanup(
    panic_message: &str,
    body: impl FnOnce() -> Result<(), FailureReport>,
) -> Result<(), FailureReport> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result.map_err(FailureReport::bounded),
        Err(_) => Err(FailureReport::new(
            FailureClass::Host,
            FailureCode::HostPanic,
            panic_message,
        )),
    }
}
fn debug_failure(error: DebugError) -> FailureReport {
    match error {
        DebugError::EngineFailure(report) | DebugError::WakeFailed(report) => report.bounded(),
        other => FailureReport::new(
            FailureClass::Host,
            FailureCode::EngineFailure,
            &format!("debug control failed: {other}"),
        ),
    }
}
/// Converts urgent debugger stop results to the loop's detached fault channel.
/// A backend already detached or closed needs no further native stop work.
fn stop_outcome(outcome: Result<DebugControlReceipt, DebugError>) -> Result<(), FailureReport> {
    match outcome {
        Ok(DebugControlReceipt::AcceptedWakeFailed { error }) => Err(debug_failure(error)),
        Ok(_) | Err(DebugError::Closed | DebugError::NotAttached) => Ok(()),
        Err(error) => Err(debug_failure(error)),
    }
}
fn cancel_native(hook: Option<&Rc<dyn CancelNative>>) -> Result<(), FailureReport> {
    if let Some(hook) = hook {
        match catch_unwind(AssertUnwindSafe(|| hook.cancel())) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let mut report = FailureReport::new(
                    FailureClass::Host,
                    FailureCode::EngineFailure,
                    "native cancellation hook failed",
                );
                report.thrown = Some(failure_data(error));
                return Err(report.bounded());
            }
            Err(_) => {
                return Err(FailureReport::new(
                    FailureClass::Host,
                    FailureCode::HostPanic,
                    "native cancellation hook panicked",
                ));
            }
        }
    }
    Ok(())
}
fn failure_data(error: OperationFailure) -> JsData {
    JsData::Record(vec![
        (
            JsString::from_utf8("code"),
            JsData::String(JsString::from_utf8(&error.code)),
        ),
        (
            JsString::from_utf8("message"),
            JsData::String(error.message),
        ),
        (
            JsString::from_utf8("details"),
            error.details.unwrap_or(JsData::Undefined),
        ),
    ])
}
/// A host event (callback job, message, completion) starts with an empty async context, so a
/// store entered with `AsyncLocalStorage.enterWith` during one event never reaches the next.
/// Timers restore the context they captured themselves (`timers.ts`). Engines without the
/// hook have no context to clear.
///
/// Fix: this read the context through the engine on every host event even when script never
/// set one. Decision: `used` is false until the `setAsyncContext` op ran, and then the context
/// is still `undefined`, so the read and the clear are skipped without a visible difference.
///
/// # Arguments
/// * `scope` - The active scope.
/// * `used` - Whether script has ever set an async context in this bridge.
///
/// # Errors
/// Engine failures while reading or clearing the context.
fn clear_async_context(scope: &dyn JsScope, used: bool) -> Result<(), FailureReport> {
    if !used {
        return Ok(());
    }
    let Ok(current) = scope.async_context() else {
        return Ok(());
    };
    if scope.kind(current)? == ValueKind::Undefined {
        return Ok(());
    }
    let undefined = scope
        .import_data(JsData::Undefined)
        .map_err(|e| scope.detach(e))?;
    scope
        .set_async_context(undefined)
        .map_err(|e| scope.detach(e))
}

fn materialization(scope: &dyn JsScope, error: ScopedError<'_>) -> FailureReport {
    let mut report = scope.detach(error);
    report.class = FailureClass::Engine;
    report.code = FailureCode::EngineFailure;
    report.bounded()
}
struct BridgeHost<'a, E: JsEngine> {
    bridge: &'a EngineBridge<E>,
    local: Option<&'a dyn LocalHost<EngineJob>>,
}
impl<E: JsEngine> BridgeHost<'_, E> {
    fn validate_job(&self, job: &CallbackJob) -> Result<(), FailureReport> {
        self.bridge
            .engine
            .with_scope(self.bridge.context, self, &mut |scope: &dyn JsScope| {
                let function = scope.local(&job.function).map_err(|e| scope.detach(e))?;
                if scope.kind(function)? != ValueKind::Function {
                    return Err(FailureReport::fault(
                        FailureCode::InvalidHandle,
                        "callback is not callable",
                    ));
                }
                if let Some(receiver) = &job.receiver {
                    scope.local(receiver).map_err(|e| scope.detach(e))?;
                }
                for argument in &job.arguments {
                    scope.local(argument).map_err(|e| scope.detach(e))?;
                }
                Ok(())
            })
    }
    fn local(&self) -> Result<&dyn LocalHost<EngineJob>, FailureReport> {
        self.local.ok_or_else(|| {
            FailureReport::fault(
                FailureCode::ClosedContext,
                "scheduler admission is unavailable during capability discovery",
            )
        })
    }
    fn bind(
        &self,
        reservation: OperationReservation,
        binding: Binding,
    ) -> Result<CompletionTicket, FailureReport> {
        let local = self.local()?;
        let id = reservation.id();
        self.bridge.bindings.borrow_mut().insert(id, binding)?;
        match local.bind_operation(reservation) {
            Ok(ticket) => Ok(ticket),
            Err(error) => {
                let removed = self.bridge.bindings.borrow_mut().remove(&id);
                drop(removed);
                Err(error)
            }
        }
    }
}
impl<E: JsEngine> HostServices for BridgeHost<'_, E> {
    #[allow(
        clippy::result_large_err,
        reason = "Rejected admission preserves owned callback roots without another allocation."
    )]
    fn post(&self, job: CallbackJob) -> Result<TaskId, Rejected<CallbackJob>> {
        let Some(local) = self.local else {
            return Err(Rejected {
                value: job,
                reason: RejectionReason::Closed,
            });
        };
        if let Err(error) = self.validate_job(&job) {
            return Err(Rejected {
                value: job,
                reason: RejectionReason::InvalidData(error),
            });
        }
        let job = self
            .bridge
            .traced_callback(job, AsyncScheduleKind::Callback);
        local.post(job).map_err(|rejected| Rejected {
            value: rejected
                .value
                .into_callback()
                .expect("scheduler returns the original posted callback"),
            reason: rejected.reason,
        })
    }
    #[allow(
        clippy::result_large_err,
        reason = "Rejected admission preserves owned callback roots without another allocation."
    )]
    fn timer(
        &self,
        delay: std::time::Duration,
        period: Option<std::time::Duration>,
        job: CallbackJob,
    ) -> Result<TaskId, Rejected<CallbackJob>> {
        let Some(local) = self.local else {
            return Err(Rejected {
                value: job,
                reason: RejectionReason::Closed,
            });
        };
        if let Err(error) = self.validate_job(&job) {
            return Err(Rejected {
                value: job,
                reason: RejectionReason::InvalidData(error),
            });
        }
        let job = self.bridge.traced_callback(job, AsyncScheduleKind::Timer);
        local
            .timer(delay, period, job)
            .map_err(|rejected| Rejected {
                value: rejected
                    .value
                    .into_callback()
                    .expect("scheduler returns the original timer callback"),
                reason: rejected.reason,
            })
    }
    fn cancel(&self, id: TaskId) -> CancelOutcome {
        self.local
            .map_or(CancelOutcome::NotFound, |local| local.cancel(id))
    }
    fn set_ref(&self, task: TaskId, keep_alive: bool) -> bool {
        self.local
            .is_some_and(|local| local.set_ref(task, keep_alive))
    }
    fn request_stop(&self) -> Result<(), FailureReport> {
        self.local()?.request_stop();
        Ok(())
    }
    fn reserve_operation(&self, bytes: usize) -> Result<OperationReservation, FailureReport> {
        self.local()?.reserve_operation(bytes)
    }
    fn bind_deferred(
        &self,
        reservation: OperationReservation,
        resolver: RootedDeferred,
        cancel: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        if resolver.context() != self.bridge.context {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign deferred resolver",
            ));
        }
        self.bind(
            reservation,
            Binding {
                target: Target::Deferred(resolver),
                history: self
                    .bridge
                    .debug
                    .as_ref()
                    .and_then(|debug| debug.capture_async(AsyncScheduleKind::Callback)),
                cancel,
            },
        )
    }
    fn bind_callback(
        &self,
        reservation: OperationReservation,
        callback: RootedValue,
        cancel: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        self.local()?;
        self.bridge
            .engine
            .with_scope(self.bridge.context, self, &mut |scope: &dyn JsScope| {
                let value = scope
                    .local(&callback)
                    .map_err(|error| scope.detach(error))?;
                if scope.kind(value)? != ValueKind::Function {
                    return Err(FailureReport::fault(
                        FailureCode::InvalidHandle,
                        "operation callback is not callable",
                    ));
                }
                Ok(())
            })?;
        self.bind(
            reservation,
            Binding {
                target: Target::Callback(callback),
                history: self
                    .bridge
                    .debug
                    .as_ref()
                    .and_then(|debug| debug.capture_async(AsyncScheduleKind::Callback)),
                cancel,
            },
        )
    }
    fn cancel_operation(&self, id: OperationId) -> CancelOutcome {
        self.local
            .map_or(CancelOutcome::NotFound, |local| local.cancel_operation(id))
    }
    fn set_tick_drain(&self, function: RootedValue) -> Result<(), FailureReport> {
        *self.bridge.tick_drain.borrow_mut() = Some(function);
        // A replacement drain starts ungated: its queue is unknown until it reports a tick.
        self.bridge.tick_gated.set(false);
        Ok(())
    }
    fn tick_queued(&self) {
        self.bridge.tick_gated.set(true);
        self.bridge.tick_pending.set(true);
    }
    fn async_context_set(&self) {
        self.bridge.async_context_used.set(true);
    }
}
impl<E: JsEngine> LoopDriver for EngineBridge<E> {
    #[cfg(feature = "builtin-facades")]
    fn attach_owner_io(&mut self, io: crate::owner_io::OwnerIoHandle<JsData>) {
        if let Some(handler) = &self.handler {
            handler.attach_owner_io(io);
        }
    }
    type Job = EngineJob;
    type Message = JsData;
    fn initialize(
        &mut self,
        handle: crate::transport::LoopHandle<JsData>,
    ) -> Result<(), FailureReport> {
        if self.debug_enabled {
            let backend = self.debug_backend().ok_or_else(|| {
                FailureReport::fault(
                    FailureCode::Unsupported,
                    "native debugger baseline unavailable",
                )
            })?;
            let coordinator = DebugCoordinator::new(
                backend,
                self.context,
                self.debug_limits.clone(),
                handle.wake_callback(),
                std::num::NonZeroU64::new(handle.epoch()).expect("nonzero loop identity"),
            )
            .map_err(debug_failure)?;
            let stop = coordinator.stop_signal();
            handle.install_stop_hook(Arc::new(move || stop_outcome(stop())));
            self.coordinator = Some(coordinator);
        } else if let Some(abort) = self.engine.abort_signal(self.context)? {
            // A stop from another thread must also end JavaScript that never yields. A stop
            // requested on the owner thread comes from the running code itself, which returns
            // to the loop on its own; aborting it would turn a clean stop into a failure.
            let owner = thread::current().id();
            handle.install_stop_hook(Arc::new(move || {
                if thread::current().id() != owner {
                    abort();
                }
                Ok(())
            }));
        }
        self.memory
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .wake = Some(handle.wake_callback());
        self.control = Some(handle);
        Ok(())
    }
    fn service_control(&mut self) -> Result<bool, FailureReport> {
        let mut served = false;
        if let Some(coordinator) = &self.coordinator {
            let pending = coordinator.pending();
            coordinator.service().map_err(debug_failure)?;
            if coordinator.is_stopping() {
                self.control
                    .as_ref()
                    .expect("debug coordinator and application control initialize together")
                    .request_stop()?;
                return Ok(true);
            }
            served = pending;
        }
        if self
            .control
            .as_ref()
            .is_some_and(LoopHandle::stop_requested)
        {
            return Ok(served);
        }
        Ok(self.serve_memory() || served)
    }
    fn has_control_work(&self) -> bool {
        self.memory.pending.load(Ordering::Acquire)
            || self
                .coordinator
                .as_ref()
                .is_some_and(|coordinator| coordinator.has_control_work())
    }
    fn shutdown(&mut self) -> Result<(), FailureReport> {
        self.memory.close();
        self.close()
    }
    fn message_validator(&self) -> Option<MessageValidator<JsData>> {
        let configured = self.handler.is_some();
        let limits = self.limits.export;
        Some(Arc::new(move |data| {
            if !configured {
                return Err(RejectionReason::Unsupported);
            }
            data.footprint(limits)
                .map(|_| ())
                .map_err(RejectionReason::InvalidData)
        }))
    }
    fn execute_job(
        &mut self,
        job: &mut EngineJob,
        host: &dyn LocalHost<EngineJob>,
    ) -> Result<(), FailureReport> {
        match job {
            EngineJob::Command(command) => {
                self.enter(host, &mut |scope: &dyn JsScope| command.run(scope))
            }
            EngineJob::Submit(source) => self.enter(host, &mut |scope: &dyn JsScope| {
                scope
                    .source()
                    .ok_or_else(|| {
                        FailureReport::fault(FailureCode::Unsupported, "source entry unavailable")
                    })?
                    .evaluate(source)
                    .map_err(|error| scope.detach(error))?;
                Ok(())
            }),
            EngineJob::Work(body) => self.enter(host, body.as_mut()),
            EngineJob::Callback(job) => self.callback_turn(host, job, None),
            EngineJob::TracedCallback { job, history } => {
                self.callback_turn(host, job, Some(*history))
            }
        }
    }
    fn deliver_message(
        &mut self,
        payload: JsData,
        host: &dyn LocalHost<EngineJob>,
    ) -> Result<(), FailureReport> {
        let handler = self
            .handler
            .as_ref()
            .ok_or_else(|| FailureReport::fault(FailureCode::Unsupported, "no message handler"))?;
        let facade = BridgeHost {
            bridge: self,
            local: Some(host),
        };
        let mut payload = Some(payload);
        self.enter(host, &mut |scope: &dyn JsScope| {
            clear_async_context(scope, self.async_context_used.get())?;
            handler
                .deliver(
                    scope,
                    &facade,
                    payload.take().expect("one message delivery"),
                )
                .map_err(|e| scope.detach(e))
        })
    }
    fn deliver_completion(
        &mut self,
        id: OperationId,
        payload: CompletionPayload,
        host: &dyn LocalHost<EngineJob>,
    ) -> Result<(), FailureReport> {
        let binding = self.bindings.borrow_mut().remove(&id).ok_or_else(|| {
            FailureReport::fault(
                FailureCode::InvalidHandle,
                "completion has no owner binding",
            )
        })?;
        let canceled = matches!(&payload,CompletionPayload::Failure(error) if matches!(error.code.as_str(),"Canceled"|"Abandoned"));
        let hook_failure = if canceled {
            cancel_native(binding.cancel.as_ref()).err()
        } else {
            None
        };
        let _history = self
            .debug
            .as_ref()
            .map(|backend| AsyncHistoryGuard::enter(backend, binding.history));
        let mut delivery = Some((binding.target, payload));
        let outcome = self.enter(host, &mut |scope: &dyn JsScope| {
            clear_async_context(scope, self.async_context_used.get())?;
            let (target, payload) = delivery.take().expect("one terminal invocation");
            let (success, data) = match payload {
                CompletionPayload::Success(data) => (true, data),
                CompletionPayload::Failure(error) => (false, failure_data(error)),
            };
            let value = scope
                .import_data(data)
                .map_err(|e| materialization(scope, e))?;
            match target {
                Target::Deferred(resolver) => scope
                    .settle(
                        resolver,
                        if success {
                            Settlement::Resolve(value)
                        } else {
                            Settlement::Reject(value)
                        },
                    )
                    .map_err(|e| materialization(scope, e))?,
                Target::Callback(callback) => {
                    let function = scope
                        .local(&callback)
                        .map_err(|e| materialization(scope, e))?;
                    let empty = scope
                        .import_data(if success {
                            JsData::Null
                        } else {
                            JsData::Undefined
                        })
                        .map_err(|e| materialization(scope, e))?;
                    let args = if success {
                        [empty, value]
                    } else {
                        [value, empty]
                    };
                    scope
                        .call(function, None, &args)
                        .map_err(|e| scope.detach(e))?;
                }
            }
            Ok(())
        });
        hook_failure.map_or(outcome, Err)
    }
    fn checkpoint(
        &mut self,
        hint: NonZeroUsize,
        host: &dyn LocalHost<EngineJob>,
    ) -> Result<CheckpointReport, FailureReport> {
        let mut report = None;
        // Taken for the checkpoint so a drain may register a replacement without a borrow
        // conflict; restored afterwards unless it was replaced.
        let drain = self.tick_drain.borrow_mut().take();
        let entered = self.enter(host, &mut |scope: &dyn JsScope| {
            let mut total = CheckpointReport::empty();
            // Runs the queued ticks; `true` when it ran any. A throw is a script failure of
            // this checkpoint and ends the tick pass.
            let tick = |total: &mut CheckpointReport| -> Result<bool, FailureReport> {
                let Some(root) = drain.as_ref() else {
                    return Ok(false);
                };
                // Fix: the drain was called (and its result exported) at least twice per
                // checkpoint even with no tick queued. Decision: once the platform reports
                // queued ticks (`tick_queued`), call it only while one is pending; the flag
                // is cleared before the call so a tick queued by a tick re-arms it, and set
                // again if the drain fails, since its remaining ticks are still queued.
                if self.tick_gated.get() && !self.tick_pending.replace(false) {
                    return Ok(false);
                }
                let function = scope.local(root).map_err(|e| {
                    self.tick_pending.set(true);
                    scope.detach(e)
                })?;
                match scope.call(function, None, &[]) {
                    Ok(value) => Ok(matches!(
                        scope.export_data(value, scope.data_limits()),
                        Ok(JsData::Bool(true))
                    )),
                    Err(error) => {
                        self.tick_pending.set(true);
                        let failure = scope.detach(error);
                        if failure.class != FailureClass::Script {
                            return Err(failure);
                        }
                        total.failures.push(failure);
                        Ok(false)
                    }
                }
            };
            // Node's order: every queued tick, then the engine's jobs, again while ticks ran.
            tick(&mut total)?;
            loop {
                let drained = scope.drain_microtasks(hint)?;
                total.failures.extend(drained.failures);
                total.suppressed_failures = total
                    .suppressed_failures
                    .saturating_add(drained.suppressed_failures);
                total.state = drained.state;
                if drained.state == CheckpointState::Pending || !tick(&mut total)? {
                    break;
                }
            }
            if let Some(debug) = &self.debug {
                debug
                    .checkpoint(total.state == CheckpointState::Empty)
                    .map_err(debug_failure)?;
            }
            // Node's and Deno's rule: a promise still rejected without a handler once the
            // microtask queue has drained fails the program with its reason. The engine
            // cancels events whose promise gained a handler before this point; a pending
            // checkpoint leaves them queued for the drain that empties the queue.
            // An engine without a rejection tracker reports Unsupported: no events.
            // VmConfig::fail_unhandled_rejections enables the rule.
            while self.fail_unhandled_rejections && total.state != CheckpointState::Pending {
                let rejection = match scope.take_rejection() {
                    Ok(Some(rejection)) => rejection,
                    Ok(None) => break,
                    Err(error) => {
                        let failure = scope.detach(error);
                        if failure.code == FailureCode::Unsupported {
                            break;
                        }
                        return Err(failure);
                    }
                };
                if !rejection.handled {
                    let mut failure = scope.detach(ScopedError::Exception(rejection.reason));
                    if failure.class == FailureClass::Script {
                        failure.code = FailureCode::UnhandledRejection;
                    }
                    total.failures.push(failure);
                }
            }
            report = Some(total);
            Ok(())
        });
        let mut slot = self.tick_drain.borrow_mut();
        if slot.is_none() {
            *slot = drain;
        }
        drop(slot);
        entered?;
        Ok(report.expect("checkpoint body ran"))
    }
}
impl<E: JsEngine> Drop for EngineBridge<E> {
    fn drop(&mut self) {
        self.memory.close();
        let _ = self.close();
    }
}

#[cfg(all(test, not(miri)))]
#[path = "../../tests/unit/bridge.rs"]
mod tests;
