use super::*;
use crate::debug::{DebugBackend, DebugDescriptor, DebugMode, DebugRequirements};
use std::{fmt, rc::Rc, sync::Arc, thread::ThreadId, time::Duration};

/// Thread-safe request that JavaScript running in one context abort at its next
/// interrupt check. It raises an uncatchable native interruption and runs no application code.
pub type AbortSignal = Arc<dyn Fn() + Send + Sync>;

/// Positive owner-local retention, reentry, and data-conversion bounds for a context.
///
/// Defaults permit 8,192 persistent roots, depth 128 host reentry, and the
/// default copied-data limits. These are not a native heap quota.
#[derive(Clone, Copy, Debug)]
pub struct ContextLimits {
    /// Maximum live persistent ownership slots, reserved before roots are exposed.
    pub max_persistent_roots: usize,
    /// Maximum synchronous host/native reentry depth.
    pub max_host_depth: usize,
    /// Default import/export and copied host-data traversal limits.
    pub export: ExportLimits,
}
impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            max_persistent_roots: 8192,
            max_host_depth: 128,
            export: ExportLimits::default(),
        }
    }
}
impl ContextLimits {
    /// Rejects any zero limit with a `Capacity` contract failure.
    pub fn validate(&self) -> Result<(), FailureReport> {
        if self.max_persistent_roots == 0
            || self.max_host_depth == 0
            || self.export.max_depth == 0
            || self.export.max_nodes == 0
            || self.export.max_bytes == 0
        {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "context limits must be positive",
            ));
        }
        Ok(())
    }
}
/// Immutable adapter/native build identity and owning thread.
///
/// Optional interfaces are discovered through scopes and the debug backend;
/// this metadata does not duplicate their capability flags.
#[derive(Clone, Debug)]
pub struct EngineDescriptor {
    /// Stable name identifying the engine adapter.
    pub adapter_id: String,
    /// Adapter implementation version.
    pub adapter_version: String,
    /// Exact native engine version or revision.
    pub native_version: String,
    /// Supported version of the engine-neutral contract.
    pub contract_version: u32,
    /// Human-readable description of supported language semantics.
    pub language_features: String,
    /// Target platform/architecture identity for compatibility.
    pub target_fingerprint: String,
    /// Native build/options identity, including debug policy.
    pub build_fingerprint: String,
    /// Required native flags and patch/revision identities.
    pub native_build_requirements: Vec<String>,
    /// Thread permitted to access native engine state.
    pub owner: ThreadId,
}
/// Discovered optional interfaces and module semantics for a context.
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineFeatures {
    /// A source-evaluation interface is available.
    pub source: bool,
    /// A trusted native compiler interface is available.
    pub compiler: bool,
    /// A trusted-artifact execution interface is available.
    pub artifacts: bool,
    /// Native module preparation/linking/evaluation is available.
    pub modules: bool,
    /// The module interface supports top-level `await`.
    pub top_level_await: bool,
    /// The module interface supports dynamic import with host resolution.
    pub dynamic_import: bool,
}
/// Application entry capability required before admitting work; defaults to source.
#[derive(Clone, Copy, Debug, Default)]
pub enum EntryRequirement {
    /// Use already-created values, including host callables; require no source loader.
    ExistingCallables,
    /// Require source evaluation.
    #[default]
    Source,
    /// Require trusted native artifact execution.
    Artifact,
    /// Require native module preparation/linking/evaluation.
    Module,
}
/// Capabilities a bridge must validate before application jobs or native work.
///
/// Defaults require source entry and the complete development debugger baseline.
/// Unsupported requirements fail rather than selecting a different adapter.
#[derive(Clone, Debug, Default)]
pub struct EngineRequirements {
    /// Required application entry path.
    pub entry: EntryRequirement,
    /// Require native compilation in addition to the entry path.
    pub compilation: bool,
    /// Require native modules and top-level await support.
    pub top_level_await: bool,
    /// Require native modules and dynamic import support.
    pub dynamic_import: bool,
    /// Required debugging profile and optional additional capabilities.
    pub debug: DebugRequirements,
}
impl EngineRequirements {
    /// Checks discovered language interfaces and debugger capabilities.
    ///
    /// Returns `Unsupported` for a missing requirement. Development always
    /// requires the full baseline; Disabled rejects nonempty extra requirements.
    /// This check consumes metadata only and executes no JavaScript.
    pub fn validate(
        &self,
        features: &EngineFeatures,
        debug: Option<&DebugDescriptor>,
    ) -> Result<(), FailureReport> {
        let entry = match self.entry {
            EntryRequirement::ExistingCallables => true,
            EntryRequirement::Source => features.source,
            EntryRequirement::Artifact => features.artifacts,
            EntryRequirement::Module => features.modules,
        };
        if !entry
            || (self.compilation && !features.compiler)
            || (self.top_level_await && !(features.modules && features.top_level_await))
            || (self.dynamic_import && !(features.modules && features.dynamic_import))
        {
            return Err(FailureReport::fault(
                FailureCode::Unsupported,
                "requested engine entry/language capability unavailable",
            ));
        }
        if self.debug.mode == DebugMode::Disabled {
            if !self.debug.additional.is_empty() {
                return Err(FailureReport::fault(
                    FailureCode::Unsupported,
                    "disabled debugging cannot require extensions",
                ));
            }
        } else {
            let required = crate::debug::DebugCapabilities::baseline().union(self.debug.additional);
            if !debug.is_some_and(|d| d.capabilities.contains_all(required)) {
                return Err(FailureReport::fault(
                    FailureCode::Unsupported,
                    "development debugger baseline unavailable",
                ));
            }
        }
        Ok(())
    }
}

/// Owner-thread adapter boundary for context lifetime and scoped native entry.
///
/// Implementations must validate identity before native access and contain native
/// ABI panics/exceptions. Shared receivers support synchronous reentry but do not
/// permit independently overlapping mutable references. Adapters must document
/// their native aliasing, callback-frame, and rooting safety arguments.
pub trait JsEngine {
    /// Borrows immutable metadata that remains valid for the engine’s Rust lifetime.
    fn descriptor(&self) -> &EngineDescriptor;
    /// Creates one owner-local context after validating positive limits.
    ///
    /// Returns detached owner/capacity/native failures before exposing an ID.
    /// Every returned ID must include this engine epoch and a fresh generation.
    fn create_context(&self, limits: ContextLimits) -> Result<ContextId, FailureReport>;
    /// Closes an inactive context’s native resources and invalidates external tokens.
    ///
    /// Active contexts return `ActiveContext` unchanged; foreign/invalid identities
    /// fail before native access. The bridge first removes jobs/operations and
    /// flushes releases. Teardown must not run JavaScript or microtask checkpoints.
    fn close_context(&self, context: ContextId) -> Result<(), FailureReport>;
    /// Discovers an optional shared owner-local backend outside application scopes.
    ///
    /// Validate owner/context before returning. Retained backend Rust storage and
    /// its immutable descriptor survive context close, but native operations then
    /// return `Closed`. Back-references must not retain the native context or create
    /// coordinator ownership cycles.
    fn debug_backend(
        &self,
        context: ContextId,
    ) -> Result<Option<Rc<dyn DebugBackend>>, FailureReport>;
    /// Enters a local native root frame, supplies host services, and runs `body` once.
    ///
    /// Local roots remain protected until return/unwind; uncaught scoped errors
    /// must be detached before returning. Validate identity/liveness first and
    /// release registry borrows before reentry. No automatic checkpoint runs.
    fn with_scope(
        &self,
        context: ContextId,
        host: &dyn HostServices,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport>;
    /// Returns a cross-thread abort request for a live context, when the adapter has one.
    ///
    /// The VM installs it so that a stop requested from another thread also ends busy
    /// JavaScript; without one, stop remains cooperative between turns.
    fn abort_signal(&self, context: ContextId) -> Result<Option<AbortSignal>, FailureReport> {
        let _ = context;
        Ok(None)
    }
    /// Runs a full native collection now; scoped and rooted values stay alive.
    ///
    /// Call only at an owner-safe boundary (no application turn running). On a
    /// reference-counting engine (QuickJS-NG) this collects cycles; acyclic garbage was
    /// already freed when its last reference went away.
    ///
    /// # Arguments
    /// * `context` - A live context of this engine.
    ///
    /// # Errors
    /// `Unsupported` for an adapter without a host-triggered collection; identity,
    /// owner or native failures otherwise.
    fn collect_garbage(&self, context: ContextId) -> Result<(), FailureReport> {
        let _ = context;
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "engine has no host-triggered garbage collection",
        ))
    }
    /// Reports the engine heap's current size and ceiling.
    ///
    /// # Arguments
    /// * `context` - A live context of this engine.
    ///
    /// # Returns
    /// Bytes the engine's heap holds now and its configured ceiling, if any.
    ///
    /// # Errors
    /// `Unsupported` for an adapter that cannot measure its heap; identity, owner or
    /// native failures otherwise.
    fn heap_usage(&self, context: ContextId) -> Result<HeapUsage, FailureReport> {
        let _ = context;
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "engine cannot report heap usage",
        ))
    }
}
/// An engine heap's size, as [`JsEngine::heap_usage`] reports it.
///
/// Decision: only the two numbers every engine defines the same way. Engine-specific
/// statistics (object counts, generations) are left out so hosts never depend on one engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapUsage {
    /// Bytes the engine's heap holds now, as the engine counts them.
    pub used_bytes: u64,
    /// The heap ceiling set when the engine was created (`EngineLimits::heap_bytes`);
    /// `None` when the engine runs without one.
    pub limit_bytes: Option<u64>,
}
impl<T: JsEngine + ?Sized> JsEngine for Box<T> {
    fn descriptor(&self) -> &EngineDescriptor {
        (**self).descriptor()
    }
    fn create_context(&self, limits: ContextLimits) -> Result<ContextId, FailureReport> {
        (**self).create_context(limits)
    }
    fn close_context(&self, context: ContextId) -> Result<(), FailureReport> {
        (**self).close_context(context)
    }
    fn debug_backend(
        &self,
        context: ContextId,
    ) -> Result<Option<Rc<dyn DebugBackend>>, FailureReport> {
        (**self).debug_backend(context)
    }
    fn with_scope(
        &self,
        context: ContextId,
        host: &dyn HostServices,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        (**self).with_scope(context, host, body)
    }
    fn abort_signal(&self, context: ContextId) -> Result<Option<AbortSignal>, FailureReport> {
        (**self).abort_signal(context)
    }
    /// Forwards to the boxed engine, so a `Box<dyn JsEngine>` (the embed runtime's engine)
    /// keeps the adapter's collection instead of the `Unsupported` default.
    ///
    /// # Arguments
    /// * `context` - A live context of the boxed engine.
    ///
    /// # Errors
    /// The boxed engine's failure.
    fn collect_garbage(&self, context: ContextId) -> Result<(), FailureReport> {
        (**self).collect_garbage(context)
    }
    /// Forwards to the boxed engine; see [`JsEngine::heap_usage`].
    ///
    /// # Arguments
    /// * `context` - A live context of the boxed engine.
    ///
    /// # Returns
    /// The boxed engine's heap usage.
    ///
    /// # Errors
    /// The boxed engine's failure.
    fn heap_usage(&self, context: ContextId) -> Result<HeapUsage, FailureReport> {
        (**self).heap_usage(context)
    }
}
/// Object-safe owner-entry callback whose scope and local values cannot escape.
pub trait ScopeCall {
    /// Runs within an active native local-root frame.
    ///
    /// Detach any unhandled scoped exception before returning an owned failure;
    /// returning does not request a checkpoint by itself.
    fn run(&mut self, scope: &dyn JsScope) -> Result<(), FailureReport>;
}
impl<F: FnMut(&dyn JsScope) -> Result<(), FailureReport>> ScopeCall for F {
    fn run(&mut self, scope: &dyn JsScope) -> Result<(), FailureReport> {
        self(scope)
    }
}
/// Owner-local native callable registration supporting synchronous recursive entry.
pub trait HostFunction {
    /// Executes for one native callback frame with protected receiver/arguments.
    ///
    /// Return values and thrown exceptions borrow `call`; no argument buffer may
    /// be overwritten by nested calls. The native trampoline contains panics before
    /// crossing its foreign ABI, and registry borrows must be released before entry.
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>>;
}
/// Active host-function frame exposing scoped values and the current host services.
pub trait HostCall: JsScope {
    /// Returns this invocation’s native receiver protected for the call frame.
    fn receiver<'s>(&'s self) -> ScopedValue<'s>;
    /// Borrows this invocation’s stable argument slice, independent of nested frames.
    fn arguments<'s>(&'s self) -> &'s [ScopedValue<'s>];
    /// Borrows services associated with this active engine entry.
    fn host(&self) -> &dyn HostServices;
}
/// Owner-local immutable message routing policy for a VM session.
pub trait HostMessageHandler {
    /// Attaches the reactor before the first application turn.
    #[cfg(feature = "builtin-facades")]
    #[doc(hidden)]
    fn attach_owner_io(&self, _: crate::owner_io::OwnerIoHandle<JsData>) {}

    /// Consumes copied payload as one ordinary scoped message turn.
    ///
    /// The bridge requests a checkpoint afterward. Recursive host-call and panic
    /// containment rules match other host entry; no separate engine or scheduler
    /// is implied by this handler interface.
    fn deliver<'s>(
        &self,
        scope: &'s dyn JsScope,
        host: &dyn HostServices,
        payload: JsData,
    ) -> JsResult<'s, ()>;
}
/// Owned persistent roots transferred into an ordinary callback or timer turn.
#[derive(Debug)]
pub struct CallbackJob {
    /// Rooted callable validated before admission.
    pub function: RootedValue,
    /// Optional rooted receiver; absent means `undefined`.
    pub receiver: Option<RootedValue>,
    /// Rooted arguments in native call order.
    pub arguments: Vec<RootedValue>,
}
/// Optional nonblocking owner-thread request to stop still-active native work.
pub trait CancelNative {
    /// Requests native cancellation without executing JavaScript.
    ///
    /// The hook must tolerate a native-finish race and may be invoked at most once
    /// for cancellation/abandonment/teardown. Failure becomes a detached diagnostic
    /// and does not change or retry the already-decided delivery.
    fn cancel(&self) -> Result<(), OperationFailure>;
}
/// Owner-local bridge services exposed to synchronous host callbacks.
///
/// Ordinary jobs/timers and reserved completion delivery use separate admission
/// capacity. These methods define a bridge contract; this trait creates no VM or
/// transport on its own. Native work starts only after a successful operation bind.
pub trait HostServices {
    #[allow(
        clippy::result_large_err,
        reason = "Queue rejection returns the already-owned callback roots without allocating a new box."
    )]
    /// Transfers callback roots into the ordinary FIFO queue.
    ///
    /// On failed admission, returns the entire job in `Rejected`; validates live
    /// context/callability and capacity before accepting ownership.
    fn post(&self, job: CallbackJob) -> Result<TaskId, Rejected<CallbackJob>>;
    #[allow(
        clippy::result_large_err,
        reason = "Queue rejection returns the already-owned callback roots without allocating a new box."
    )]
    /// Transfers roots into a one-shot or repeating monotonic timer.
    ///
    /// `None` is one-shot; a repeating period must be valid and representable.
    /// Validation/capacity/deadline failure returns the whole unaccepted job.
    fn timer(
        &self,
        delay: Duration,
        period: Option<Duration>,
        job: CallbackJob,
    ) -> Result<TaskId, Rejected<CallbackJob>>;
    /// Decides task cancellation on the owner thread using checked task identity.
    ///
    /// A running repeating callback may finish but must not be rearmed.
    fn cancel(&self, task: TaskId) -> CancelOutcome;
    /// Sets whether a queued job or timer keeps the loop alive; `false` when the task is gone.
    fn set_ref(&self, task: TaskId, keep_alive: bool) -> bool {
        let _ = (task, keep_alive);
        false
    }
    /// Closes ordinary admission and requests cooperative owner-loop shutdown.
    ///
    /// This does not synchronously execute callbacks or promise native host-call
    /// preemption; transport failure is returned as detached data.
    fn request_stop(&self) -> Result<(), FailureReport>;
    /// Reserves dedicated delivery capacity and the declared owned payload allowance.
    ///
    /// Must succeed before native work starts. Failure allocates no live operation;
    /// dropping the returned unbound reservation rolls back without terminal delivery.
    fn reserve_operation(
        &self,
        max_payload_bytes: usize,
    ) -> Result<OperationReservation, FailureReport>;
    /// Consumes a reservation and live context-bound resolver, committing one delivery.
    ///
    /// Validates loop/context/capability before commit; failure rolls back and
    /// releases consumed roots safely. Success returns the sole transferable sender.
    /// Only then may native work begin; optional cancellation never executes JS.
    fn bind_deferred(
        &self,
        reservation: OperationReservation,
        resolver: RootedDeferred,
        cancel: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport>;
    /// Consumes a reservation and callable root, committing one reserved completion.
    ///
    /// Validates live context and callability before native work. Failure rolls back
    /// and releases consumed roots. Success delivery calls `(error, value)` with
    /// receiver `undefined`; callback exceptions are never retried.
    fn bind_callback(
        &self,
        reservation: OperationReservation,
        callback: RootedValue,
        cancel: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport>;
    /// Decides owner-thread cancellation against completion under shared operation state.
    ///
    /// Winning cancellation schedules a reserved `Canceled` delivery; already
    /// decided or dispatching completion returns `TooLate`, not another invocation.
    fn cancel_operation(&self, operation: OperationId) -> CancelOutcome;
    /// Registers the platform's tick drain, replacing an earlier one. Every checkpoint calls
    /// it before engine jobs and again after them until it returns `false`: it runs the
    /// queued `nextTick` callbacks and returns whether it ran any.
    fn set_tick_drain(&self, function: RootedValue) -> Result<(), FailureReport> {
        drop(function);
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "tick drain unavailable",
        ))
    }
    /// Tells the host a `nextTick` callback was queued, so the next checkpoint must call the
    /// tick drain. Once a platform has called this, the host may skip the drain while no
    /// tick is queued; a platform that never calls it is drained at every checkpoint.
    /// Hosts without a tick drain ignore it.
    fn tick_queued(&self) {}
    /// Tells the host the async context was set by script, so host events must start by
    /// clearing it. A host that never sees this call may skip that clearing.
    /// Hosts without an async context ignore it.
    fn async_context_set(&self) {}
}
/// Non-cloneable owner-thread reservation of completion capacity and payload budget.
///
/// Binding consumes it exactly once. Drop before binding runs rollback and emits
/// no terminal delivery; it is not the transferable producer capability.
pub struct OperationReservation {
    pub(crate) id: OperationId,
    pub(crate) rollback: Option<Box<dyn FnOnce()>>,
}
impl OperationReservation {
    /// Returns the reserved operation identity without committing the reservation.
    pub fn id(&self) -> OperationId {
        self.id
    }
}
impl fmt::Debug for OperationReservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OperationReservation")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
impl Drop for OperationReservation {
    fn drop(&mut self) {
        if let Some(rollback) = self.rollback.take() {
            rollback();
        }
    }
}
/// Owned transferable native-operation failure, never a live JavaScript exception.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationFailure {
    /// Stable application/host failure code.
    pub code: String,
    /// Lossless human-readable failure message.
    pub message: JsString,
    /// Optional bounded copied details imported into the common failure record.
    pub details: Option<JsData>,
}
/// Owned producer result moved into a reserved completion mailbox.
#[derive(Debug)]
pub enum CompletionPayload {
    /// Successful data imported on the owner thread.
    Success(JsData),
    /// Failure imported as an own-field `code`/`message`/`details` record.
    Failure(OperationFailure),
}
/// Reason a producer retains ownership of an unaccepted completion payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishRejection {
    /// Cancellation or abandonment already decided the operation result.
    Canceled,
    /// Context teardown has closed delivery.
    Closed,
    /// The operation epoch/slot/generation is no longer live.
    Stale,
    /// Payload validation failed its allowance; a pending operation gets a reserved size error.
    PayloadTooLarge,
}
/// Linearized result of consuming a completion sender.
///
/// Accepted publication owns the payload even if a later wake fails. A producer
/// must never retry `PublishedWakeFailed` as another completion.
#[derive(Debug)]
pub enum PublishOutcome {
    /// The reserved mailbox accepted the terminal payload.
    Published,
    /// Publication succeeded, but signaling failed fatally; ownership is not returned.
    PublishedWakeFailed {
        /// Detached signaling failure after accepted publication.
        error: FailureReport,
    },
    /// The original payload was not accepted and is returned to its producer.
    Rejected {
        /// Why the completion payload was not accepted.
        reason: PublishRejection,
        /// Unaccepted producer-owned payload, safe to inspect or drop on this thread.
        payload: CompletionPayload,
    },
}
/// Sendable one-use transport hook containing no engine values.
///
/// Implementations publish only into reserved bounded operation state; they may
/// not access native engine state or require ordinary queue capacity.
pub trait CompletionPublisher: Send {
    /// Consumes the sender, validates/measures payload before transport locking,
    /// and linearizes publication against cancellation/close.
    ///
    /// Oversized input is returned while a still-pending operation receives its
    /// reserved size-error outcome. Accepted-but-wake-failed must not be retried.
    fn complete(self: Box<Self>, payload: CompletionPayload) -> PublishOutcome;
    /// Consumes an unused sender; a still-pending active operation receives its
    /// reserved `Abandoned` failure. Closed delivery is discarded during teardown.
    fn abandon(self: Box<Self>);
}
/// Transferable non-cloneable producer capability for exactly one reserved result.
///
/// Contains no engine value and does not keep an engine alive. Completing consumes
/// the ticket; dropping it unfinished asks the publisher for reserved abandonment
/// delivery while the context is active.
pub struct CompletionTicket {
    pub(crate) id: OperationId,
    pub(crate) publisher: Option<Box<dyn CompletionPublisher>>,
}
impl CompletionTicket {
    /// Returns the checked operation identity associated with this ticket.
    pub fn id(&self) -> OperationId {
        self.id
    }
    /// Consumes the ticket and moves its payload to the reserved publisher.
    ///
    /// Publication races are decided by transport state. Inspect the returned
    /// outcome: `PublishedWakeFailed` already transferred ownership and is not retryable.
    pub fn complete(mut self, payload: CompletionPayload) -> PublishOutcome {
        self.publisher
            .take()
            .expect("owned ticket publisher")
            .complete(payload)
    }
}
impl fmt::Debug for CompletionTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionTicket")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
impl Drop for CompletionTicket {
    fn drop(&mut self) {
        if let Some(publisher) = self.publisher.take() {
            publisher.abandon();
        }
    }
}
