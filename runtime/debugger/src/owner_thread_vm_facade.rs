//! One owner-thread VM facade over the existing engine bridge and event loop.
#[path = "owner_thread_vm_facade/command_transaction.rs"]
pub(crate) mod command;
#[path = "owner_thread_vm_facade/native_execution_state.rs"]
mod modules;
pub use crate::debug::{DebugSessionOptions, VmDebugHandle};
use crate::{
    debug::{DebugDescriptor, DebugExecutionState, DebugLimits},
    engine::*,
    scheduler::*,
    transport::LoopHandle,
};
use command::{Capture, Captured, Kind};
use std::{
    cell::{Cell, RefCell},
    fmt,
    rc::Rc,
    sync::Arc,
    thread::{self, ThreadId},
};

/// Public owner lifecycle; debug execution state is an independent query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmState {
    /// Constructed, live and outside a pump.
    Ready,
    /// One synchronous command or pump is active, including a native debug pause.
    Running,
    /// Stop is published but owner teardown has not yet completed.
    Stopping,
    /// Owner resources were disposed after explicit/cooperative stop.
    Stopped,
    /// Owner teardown completed after a detached stopping failure.
    Faulted,
}
/// Detached VM outcome; no variant contains a scoped or rooted engine value.
#[derive(Clone, Debug, PartialEq)]
pub enum VmError {
    /// Invalid configuration or construction rollback failure.
    Configuration(FailureReport),
    /// A requested optional engine/debug interface is unavailable.
    Unsupported,
    /// The requested operation is invalid in the current owner state.
    InvalidState,
    /// A pump/execution operation is already active.
    ReentrantEntry,
    /// Entry occurred outside the owning thread.
    WrongOwner,
    /// A borrowed input belongs to a foreign or invalid context.
    InvalidHandle,
    /// The command was rejected before pump entry; owned submission stays rejected.
    Admission(RejectionReason),
    /// The queued synchronous target was removed before its turn.
    Canceled,
    /// Stop completed or takes precedence over new execution.
    Stopped,
    /// A prior stopping failure prevents further execution.
    Faulted,
    /// This command or its checkpoint returned a detached execution failure.
    Execution(FailureReport),
}
impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for VmError {}
/// Configuration validated before application code or native operations begin.
pub struct VmConfig {
    /// Independent scheduler/transport/native-source capacities and batch sizes.
    pub scheduler: SchedulerConfig,
    /// Owner-local rooting, synchronous depth and copied-data limits.
    pub context: ContextLimits,
    /// Required source/module/artifact entry and native-debugging profile.
    pub requirements: EngineRequirements,
    /// Application exception policy; Continue requires report_sink.
    pub script_error_policy: ScriptErrorPolicy,
    /// Owner-local detached diagnostics receiver, never called under native entry.
    pub report_sink: Option<ReportSink>,
    /// One immutable ordinary-message route for this VM session.
    pub message_handler: Option<Rc<dyn HostMessageHandler>>,
    /// Independent debugger request/reply/source/inspection budgets.
    pub debug_limits: DebugLimits,
    /// Execution-session module budgets, independent of any platform Program's admission budgets.
    pub module_limits: crate::modules::ModuleLimits,
    /// Node's and Deno's program rule: a rejection still unhandled once the microtask queue
    /// drains fails the checkpoint with `UnhandledRejection`. Off, rejections stay observable
    /// through the debugger and `HostCall::take_rejection` without stopping the VM.
    pub fail_unhandled_rejections: bool,
}
impl Default for VmConfig {
    fn default() -> Self {
        Self {
            scheduler: SchedulerConfig::default(),
            context: ContextLimits::default(),
            requirements: EngineRequirements::default(),
            script_error_policy: ScriptErrorPolicy::Stop,
            report_sink: None,
            message_handler: None,
            debug_limits: DebugLimits::default(),
            module_limits: crate::modules::ModuleLimits::default(),
            fail_unhandled_rejections: false,
        }
    }
}
/// One completed cooperative owner slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VmTick {
    /// Owner state after that slice, independent of native debug execution state.
    pub state: VmState,
    /// Whether application/maintenance/checkpoint work progressed.
    pub made_progress: bool,
}
/// Cloneable producer/control metadata; contains no engine, value or owner facade.
#[derive(Clone)]
pub struct VmHandle {
    inner: LoopHandle<JsData>,
    /// The bridge's memory request mailbox.
    memory: Arc<crate::engine::MemoryRequests>,
    /// The limits the VM's message validator enforces, when it has a message handler.
    message_limits: Option<crate::engine::ExportLimits>,
}
impl VmHandle {
    /// The limits a message must meet to be admitted by [`Self::try_send`].
    ///
    /// # Returns
    /// The limits, or `None` when the VM has no message handler and refuses every message.
    pub(crate) fn message_limits(&self) -> Option<crate::engine::ExportLimits> {
        self.message_limits
    }
    /// [`Self::try_send`] for data the caller already measured with
    /// [`JsData::footprint`] under [`Self::message_limits`].
    ///
    /// Fix: a worker message was walked twice, once by the sender's mailbox for its byte
    /// credit and once by this lane's validator. Decision: the mailbox makes the
    /// validator's walk itself and sends through here, which skips the second one.
    ///
    /// # Arguments
    /// * `data` - The measured data.
    ///
    /// # Errors
    /// As [`Self::try_send`], except the validator's rejections.
    #[allow(
        clippy::result_large_err,
        reason = "as try_send: rejection returns the original owned data"
    )]
    pub(crate) fn try_send_measured(&self, data: JsData) -> Result<(), Rejected<JsData>> {
        self.inner.try_send_unvalidated(data)
    }
    /// Asks the engine to collect garbage at its next owner-safe boundary (between turns,
    /// never inside running JavaScript), then measure its heap. A program spinning in one
    /// turn answers once that turn ends. Workers have their own engines and are not included.
    ///
    /// # Returns
    /// A receiver for the heap usage after the collection. It disconnects without an answer
    /// when the VM ends first, or receives [`FailureCode::Capacity`] if the request mailbox is full.
    pub fn collect_garbage(&self) -> std::sync::mpsc::Receiver<Result<HeapUsage, FailureReport>> {
        self.memory.request(true)
    }
    /// Asks for the engine heap's size and ceiling at its next owner-safe boundary.
    ///
    /// # Returns
    /// A receiver for the heap usage; it disconnects without an answer when the VM ends first.
    /// A full request mailbox answers with [`FailureCode::Capacity`].
    pub fn heap_usage(&self) -> std::sync::mpsc::Receiver<Result<HeapUsage, FailureReport>> {
        self.memory.request(false)
    }
    /// Exact scheduler identity for debugger target ownership, never a pointer.
    pub(crate) fn runtime_epoch(&self) -> u64 {
        self.inner.epoch()
    }
    /// Publish cooperative stop independently of every queue's capacity.
    pub fn request_stop(&self) -> Result<(), FailureReport> {
        self.inner.request_stop()
    }
    /// Admit a cancellation request; processing by the owner decides its effect.
    #[allow(
        clippy::result_large_err,
        reason = "a rejection hands the refused value back with its report"
    )]
    pub fn request_cancel(&self, task: TaskId) -> Result<(), Rejected<TaskId>> {
        self.inner.request_cancel(task)
    }
    /// Validate and move ordinary copied data to the configured fixed handler.
    /// Rejection returns ownership; accepted messages are not yet executed.
    #[allow(
        clippy::result_large_err,
        reason = "The specified admission API returns original owned data without allocating on rejection."
    )]
    pub fn try_send(&self, data: JsData) -> Result<(), Rejected<JsData>> {
        self.inner.try_send(data)
    }
    /// Moves ordinary copied data to the configured handler, waiting for inbox capacity.
    ///
    /// Success means admitted, not dispatched. Non-capacity rejections return the
    /// original data. Canceling a pending future drops its not-yet-admitted data.
    #[allow(
        clippy::result_large_err,
        reason = "As with try_send, rejection returns the original owned data without boxing it."
    )]
    pub async fn send(&self, data: JsData) -> Result<(), Rejected<JsData>> {
        self.inner.send(data).await
    }
    /// How often delivery capacity was released so far. Read it before a [`Self::try_send`]
    /// that may come back `Full`, then await [`Self::capacity`] with it.
    pub fn capacity_epoch(&self) -> u64 {
        self.inner.stats().capacity_epoch
    }
    /// Resolves once capacity was released after epoch `seen`, or the VM stops; a producer
    /// whose message was refused as `Full` waits on it instead of polling.
    pub fn capacity(&self, seen: u64) -> crate::transport::CapacityChange<JsData> {
        self.inner.capacity(seen)
    }
}
/// Single owner of one event loop, one bridge, one engine and one active context.
///
/// Public synchronous work uses the same FIFO and checkpoint path as queued work.
/// No worker, second pump, implicit Promise await or live-value migration exists.
pub struct VirtualMachine<E: JsEngine> {
    runner: EventLoop<EngineBridge<E>>,
    owner: ThreadId,
    state: Cell<VmState>,
    active: Cell<bool>,
    capture: Capture,
    descriptor: EngineDescriptor,
    features: EngineFeatures,
    context: ContextId,
    failure: Option<FailureReport>,
    debug_limits: DebugLimits,
    development: bool,
    modules: modules::ExecutionModules,
    module_limits: crate::modules::ModuleLimits,
    /// The bridge's memory request mailbox, handed out with every [`VmHandle`].
    memory: Arc<crate::engine::MemoryRequests>,
    /// See [`VmHandle::message_limits`].
    message_limits: Option<crate::engine::ExportLimits>,
}
impl<E: JsEngine> VirtualMachine<E> {
    /// Construct and validate one bridge/context and its loop before entering Ready.
    /// All completed stages roll back on failure without executing application jobs.
    pub fn new(engine: E, config: VmConfig) -> Result<Self, VmError> {
        config
            .scheduler
            .validate()
            .map_err(VmError::Configuration)?;
        config.context.validate().map_err(VmError::Configuration)?;
        config.module_limits.validate().map_err(|e| {
            VmError::Configuration(FailureReport::fault(FailureCode::Capacity, &e.to_string()))
        })?;
        let limits = &config.debug_limits;
        if limits
            .inspection_objects
            .checked_add(limits.write_targets)
            .is_none()
        {
            return Err(VmError::Configuration(FailureReport::fault(
                FailureCode::Capacity,
                "debug reference capacity overflow",
            )));
        }
        if [
            limits.requests,
            limits.updates,
            limits.request_bytes,
            limits.reply_bytes,
            limits.inspection_objects,
            limits.write_targets,
            limits.inspection_page_items,
            limits.inspection_scan_slots,
            limits.location_scan_slots,
            limits.scripts,
            limits.source_bytes,
            limits.breakpoints,
            limits.condition_bytes,
            limits.expression_units,
            limits.dap_output_bytes,
        ]
        .contains(&0)
            || config.script_error_policy == ScriptErrorPolicy::Continue
                && config.report_sink.is_none()
        {
            return Err(VmError::Configuration(FailureReport::fault(
                FailureCode::Capacity,
                "invalid VM/debug configuration",
            )));
        }
        for deadline_ms in [limits.evaluation_deadline_ms, limits.condition_deadline_ms] {
            crate::debug::EvaluationControl::new(crate::debug::EvaluationOptions {
                deadline_ms,
                ..crate::debug::EvaluationOptions::default()
            })
            .map_err(|_| {
                VmError::Configuration(FailureReport::fault(
                    FailureCode::Capacity,
                    "invalid debugger evaluation budget",
                ))
            })?;
        }
        let development = config.requirements.debug.mode == crate::debug::DebugMode::Development;
        let message_limits = config
            .message_handler
            .is_some()
            .then_some(config.context.export);
        let bridge = EngineBridge::new(
            engine,
            config.context,
            config.requirements,
            config.message_handler,
            config.scheduler.max_operations,
        )
        .map_err(|report| {
            if report.code == FailureCode::Unsupported {
                VmError::Unsupported
            } else {
                VmError::Configuration(report)
            }
        })?;
        let bridge = bridge
            .with_debug_limits(config.debug_limits.clone())
            .with_unhandled_rejections_failing(config.fail_unhandled_rejections);
        let descriptor = bridge.descriptor().clone();
        let features = *bridge.features();
        let context = bridge.context();
        let memory = bridge.memory_requests();
        let runner = EventLoop::with_policy(
            bridge,
            config.scheduler,
            config.report_sink,
            config.script_error_policy,
        )
        .map_err(VmError::Configuration)?;
        Ok(Self {
            runner,
            owner: thread::current().id(),
            state: Cell::new(VmState::Ready),
            active: Cell::new(false),
            capture: Rc::new(RefCell::new(Captured::Empty)),
            descriptor,
            features,
            context,
            failure: None,
            debug_limits: config.debug_limits,
            development,
            modules: modules::ExecutionModules::default(),
            module_limits: config.module_limits,
            memory,
            message_limits,
        })
    }
    /// Query owner lifecycle, surfacing a published stop before teardown runs.
    pub fn state(&self) -> VmState {
        let state = self.state.get();
        if matches!(state, VmState::Ready | VmState::Running) && self.runner.is_stop_requested() {
            VmState::Stopping
        } else {
            state
        }
    }
    /// Cached immutable adapter/build metadata, valid after shutdown.
    pub fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }
    /// Cached scoped interface discovery, valid after shutdown.
    pub fn features(&self) -> &EngineFeatures {
        &self.features
    }
    /// Last bounded detached stopping failure, valid after context destruction.
    pub fn last_failure(&self) -> Option<&FailureReport> {
        self.failure.as_ref().or_else(|| self.runner.last_failure())
    }
    /// Cached native debug metadata, independent of context lifetime.
    pub fn debug_descriptor(&self) -> Option<&DebugDescriptor> {
        self.runner.driver().debug_descriptor()
    }
    /// Query native debug state separately from pump ownership.
    pub fn debug_state(&self) -> DebugExecutionState {
        self.runner.driver().debug_state()
    }
    /// Establish one native session at Ready. Its controller progresses independently
    /// while the owner is synchronously executing or suspended at a native pause.
    pub fn attach_debugger(
        &mut self,
        options: DebugSessionOptions,
    ) -> Result<VmDebugHandle, VmError> {
        self.ready()?;
        if !self.development {
            return Err(VmError::Unsupported);
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.runner.driver().attach_debugger(options)
        })) {
            Ok(result) => result.map_err(|error| match error {
                crate::debug::DebugError::Unsupported => VmError::Unsupported,
                _ => VmError::InvalidState,
            }),
            Err(_) => {
                let report = FailureReport::new(
                    FailureClass::Host,
                    FailureCode::HostPanic,
                    "debugger attachment panicked",
                );
                self.failure = Some(report.clone());
                let _ = self.runner.shutdown();
                self.state.set(VmState::Faulted);
                Err(VmError::Execution(report))
            }
        }
    }
    fn ready(&mut self) -> Result<(), VmError> {
        if thread::current().id() != self.owner {
            return Err(VmError::WrongOwner);
        }
        if self.active.get() {
            return Err(VmError::ReentrantEntry);
        }
        match self.state() {
            VmState::Ready => Ok(()),
            VmState::Stopping => {
                self.shutdown()?;
                Err(VmError::Stopped)
            }
            VmState::Stopped => Err(VmError::Stopped),
            VmState::Faulted => Err(VmError::Faulted),
            VmState::Running => Err(VmError::ReentrantEntry),
        }
    }
    fn refresh(&mut self) {
        self.active.set(false);
        if let Some(failure) = self.runner.last_failure() {
            self.failure = Some(failure.clone());
            self.state.set(VmState::Faulted);
        } else if self.failure.is_some() {
            self.state.set(VmState::Faulted);
        } else if self.runner.is_stopped() {
            self.state.set(VmState::Stopped);
        } else {
            self.state.set(VmState::Ready);
        }
    }
    fn command(&mut self, kind: Kind) -> Result<Captured, VmError> {
        self.ready()?;
        *self.capture.borrow_mut() = Captured::Empty;
        let task = self
            .runner
            .host()
            .post(EngineJob::Command(EngineCommand::new(
                kind,
                self.capture.clone(),
            )))
            .map_err(|rejected| VmError::Admission(rejected.reason))?;
        self.active.set(true);
        self.state.set(VmState::Running);
        let pumped = self.runner.run_until(task);
        self.refresh();
        let captured = std::mem::replace(&mut *self.capture.borrow_mut(), Captured::Empty);
        let outcome = match pumped {
            Err(report) => {
                drop(captured);
                return Err(VmError::Execution(report));
            }
            Ok(outcome) => outcome,
        };
        if outcome == TurnOutcome::Stopped || self.runner.is_stop_requested() {
            drop(captured);
            self.shutdown()?;
            return Err(VmError::Stopped);
        }
        if outcome == TurnOutcome::Canceled {
            return Err(VmError::Canceled);
        }
        if matches!(captured, Captured::Empty) {
            let report = FailureReport::fault(
                FailureCode::EngineFailure,
                "engine omitted the command body",
            );
            self.failure = Some(report.clone());
            let _ = self.shutdown();
            self.state.set(VmState::Faulted);
            Err(VmError::Execution(report))
        } else {
            Ok(captured)
        }
    }
    fn value(&mut self, kind: Kind) -> Result<RootedValue, VmError> {
        match self.command(kind)? {
            Captured::Value(root) => Ok(root),
            _ => Err(VmError::InvalidState),
        }
    }
    /// Create and install a native host callable using normal global property rules.
    pub fn bind_function(
        &mut self,
        name: JsString,
        arity: u16,
        function: Rc<dyn HostFunction>,
    ) -> Result<RootedValue, VmError> {
        self.value(Kind::Bind {
            name,
            arity,
            function,
        })
    }
    /// Evaluate owned source as one FIFO turn; a returned Promise is not awaited.
    pub fn evaluate(&mut self, source: ScriptSource) -> Result<RootedValue, VmError> {
        self.ready()?;
        if !self.features.source {
            return Err(VmError::Unsupported);
        }
        self.validate_source(&source).map_err(VmError::Execution)?;
        self.value(Kind::Evaluate(source))
    }
    /// Invoke existing values through a queued command, retaining private local
    /// leases until its turn/checkpoint ends without copying JavaScript values.
    pub fn call(
        &mut self,
        function: &RootedValue,
        receiver: Option<&RootedValue>,
        arguments: &[RootedValue],
    ) -> Result<RootedValue, VmError> {
        self.ready()?;
        if function.context_id() != self.context
            || receiver.is_some_and(|root| root.context_id() != self.context)
            || arguments
                .iter()
                .any(|root| root.context_id() != self.context)
        {
            return Err(VmError::InvalidHandle);
        }
        self.value(Kind::Call {
            function: function.local_lease(),
            receiver: receiver.map(RootedValue::local_lease),
            arguments: arguments.iter().map(RootedValue::local_lease).collect(),
        })
    }
    /// Compile owned source through the same FIFO/checkpoint path as execution.
    pub fn compile(
        &mut self,
        source: ScriptSource,
        options: CompileOptions,
    ) -> Result<CompiledArtifact, VmError> {
        self.ready()?;
        if !self.features.compiler || self.development && options.debug_info != DebugInfo::Required
        {
            return Err(VmError::Unsupported);
        }
        self.validate_source(&source).map_err(VmError::Execution)?;
        match self.command(Kind::Compile(source, options))? {
            Captured::Artifact(artifact) => Ok(artifact),
            _ => Err(VmError::InvalidState),
        }
    }
    /// Execute only opaque compiler-produced artifacts compatible with this adapter.
    /// Native format/options validation remains mandatory before native loading.
    pub fn execute_artifact(&mut self, artifact: CompiledArtifact) -> Result<RootedValue, VmError> {
        self.ready()?;
        if !self.features.artifacts {
            return Err(VmError::Unsupported);
        }
        let metadata = artifact.metadata();
        if metadata.adapter_id != self.descriptor.adapter_id
            || metadata.native_version != self.descriptor.native_version
            || metadata.target_fingerprint != self.descriptor.target_fingerprint
            || metadata.build_fingerprint != self.descriptor.build_fingerprint
            || self.development && metadata.options.debug_info != DebugInfo::Required
        {
            return Err(VmError::Execution(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "artifact belongs to another native build/profile",
            )));
        }
        self.value(Kind::Artifact(artifact))
    }
    /// Run owned advanced scoped work as a normal turn; it cannot leak scoped values.
    pub fn execute(&mut self, body: Box<dyn ScopeCall>) -> Result<(), VmError> {
        match self.command(Kind::Work(body))? {
            Captured::Unit => Ok(()),
            _ => Err(VmError::InvalidState),
        }
    }
    /// Enqueue source without retaining its result, preserving ownership on rejection.
    #[allow(
        clippy::result_large_err,
        reason = "Rejected source returns the original owned buffers without another allocation."
    )]
    pub fn submit(&mut self, source: ScriptSource) -> Result<TaskId, Rejected<ScriptSource>> {
        let reason = self.ready().err().map(|error| match error {
            VmError::WrongOwner => RejectionReason::WrongOwner,
            _ => RejectionReason::Closed,
        });
        if let Some(reason) = reason {
            return Err(Rejected {
                value: source,
                reason,
            });
        }
        if !self.features.source {
            return Err(Rejected {
                value: source,
                reason: RejectionReason::Unsupported,
            });
        }
        if let Err(report) = self.validate_source(&source) {
            return Err(Rejected {
                value: source,
                reason: RejectionReason::InvalidSource(report),
            });
        }
        self.runner
            .host()
            .post(EngineJob::Submit(source))
            .map_err(|rejected| Rejected {
                value: rejected
                    .value
                    .into_submit()
                    .expect("scheduler returns the original submitted source"),
                reason: rejected.reason,
            })
    }
    /// Validates copied source metadata before admission and returns its detached
    /// failure, leaving each public entry point to select its own error envelope.
    fn validate_source(&self, source: &ScriptSource) -> Result<(), FailureReport> {
        crate::debug::validate_source(source, &self.debug_limits).map_err(|error| {
            FailureReport::fault(
                FailureCode::InvalidArtifact,
                &format!("invalid source: {error}"),
            )
        })
    }
    /// Pump one existing scheduler slice; false forbids idle ordinary I/O waits, and the
    /// slice that runs the startup checkpoint never waits.
    pub fn tick(&mut self, block: bool) -> Result<VmTick, VmError> {
        self.ready()?;
        self.active.set(true);
        self.state.set(VmState::Running);
        let result = self.runner.tick(block && !self.runner.startup_pending());
        self.refresh();
        result
            .map(|tick| VmTick {
                state: self.state(),
                made_progress: tick.made_progress,
            })
            .map_err(VmError::Execution)
    }
    /// Run the existing loop until stop or detached failure; idle does not exit.
    pub fn run(&mut self) -> Result<(), VmError> {
        self.ready()?;
        self.active.set(true);
        self.state.set(VmState::Running);
        let result = self.runner.run();
        self.refresh();
        result.map_err(VmError::Execution)
    }
    /// Pumps the existing loop until tracked jobs, checkpoints, timers, sources and operations quiesce.
    /// A pending module promise with no remaining work must be diagnosed by the caller.
    pub fn run_until_idle(&mut self) -> Result<(), VmError> {
        loop {
            self.ready()?;
            if !self.runner.has_tracked_work() {
                return Ok(());
            }
            self.active.set(true);
            self.state.set(VmState::Running);
            let result = self.runner.tick(!self.runner.startup_pending());
            self.refresh();
            result.map_err(VmError::Execution)?;
            if matches!(self.state(), VmState::Stopped) {
                return Ok(());
            }
        }
    }
    /// What keeps the scheduler alive, for the end-of-run trace report.
    pub fn pending_work(&self) -> serde_json::Value {
        self.runner.pending_work()
    }
    /// Reports scheduler work. Platform orchestration separately drains native import requests.
    pub fn has_tracked_work(&self) -> bool {
        self.runner.has_tracked_work()
    }
    /// Clone only the existing producer/control metadata, never native values.
    pub fn handle(&self) -> VmHandle {
        VmHandle {
            inner: self.runner.handle(),
            memory: self.memory.clone(),
            message_limits: self.message_limits,
        }
    }
    /// Idempotent owner teardown without another application turn or checkpoint.
    pub fn shutdown(&mut self) -> Result<(), VmError> {
        if thread::current().id() != self.owner {
            return Err(VmError::WrongOwner);
        }
        if self.active.get() {
            return Err(VmError::ReentrantEntry);
        }
        let result = self.runner.shutdown();
        self.refresh();
        result.map_err(VmError::Execution)
    }
}
impl<E: JsEngine> Drop for VirtualMachine<E> {
    fn drop(&mut self) {
        let _ = self.runner.shutdown();
    }
}

// Reuse the actual Rust contract adapter from integration tests. The crate-root
// cfg(test) alias resolves its public imports without a production test hook.
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
pub(crate) mod contract_fixture;
#[cfg(test)]
#[path = "../tests/support/allocator.rs"]
pub(crate) mod test_allocator;

#[cfg(all(test, not(miri)))]
#[path = "../tests/unit/vm_bench.rs"]
mod benchmarks;
#[cfg(test)]
#[path = "../tests/unit/vm_faults.rs"]
pub(crate) mod faults;
#[cfg(test)]
#[path = "../tests/unit/vm.rs"]
mod tests;
