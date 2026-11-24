//! Private bridge guards backed by the existing Rust ValueStore fixture and scheduler.
use super::*;
use crate::vm::contract_fixture as support;
use crate::{
    debug::DebugRequirements,
    scheduler::{EventLoop, SchedulerConfig},
};
use std::time::Duration;
use support::{Inline, RustFixture};

type Bridge = EngineBridge<RustFixture<Inline>>;

#[test]
fn memory_control_requests_report_capacity_instead_of_growing_without_bound() {
    let requests = MemoryRequests::default();
    let accepted: Vec<_> = (0..128).map(|_| requests.request(false)).collect();
    let rejected = requests.request(true);
    assert_eq!(
        rejected.try_recv().unwrap().unwrap_err().code,
        FailureCode::Capacity
    );
    assert!(
        accepted
            .iter()
            .all(|answer| matches!(answer.try_recv(), Err(mpsc::TryRecvError::Empty)))
    );
    requests.close();
    assert!(
        accepted
            .iter()
            .all(|answer| matches!(answer.try_recv(), Err(mpsc::TryRecvError::Disconnected)))
    );
}

#[test]
fn memory_control_batches_preserve_fifo_and_the_remaining_pending_flag() {
    let requests = MemoryRequests::default();
    let answers: Vec<_> = (0..20).map(|index| requests.request(index >= 16)).collect();
    let first = requests.take();
    assert_eq!(first.len(), 16);
    assert!(first.iter().all(|request| !request.collect));
    assert!(requests.pending.load(Ordering::Acquire));
    let second = requests.take();
    assert_eq!(second.len(), 4);
    assert!(second.iter().all(|request| request.collect));
    assert!(!requests.pending.load(Ordering::Acquire));
    assert!(requests.take().is_empty());
    drop((first, second));
    assert!(
        answers
            .iter()
            .all(|answer| matches!(answer.try_recv(), Err(mpsc::TryRecvError::Disconnected)))
    );
}

#[test]
fn urgent_debug_stop_precedes_queued_memory_work() {
    let (mut bridge, control, _) = development_bridge();
    let owner = runtime(1);
    bridge.initialize(owner.handle()).unwrap();
    control.attach.set(crate::vm::faults::AttachFault::Accept);
    let _debugger = bridge
        .attach_debugger(DebugSessionOptions::default())
        .unwrap();
    let answer = bridge.memory.request(true);
    bridge.coordinator.as_ref().unwrap().stop_signal()().unwrap();
    assert!(bridge.service_control().unwrap());
    assert!(owner.is_stop_requested());
    assert!(matches!(answer.try_recv(), Err(mpsc::TryRecvError::Empty)));
    drop(bridge);
    assert!(matches!(
        answer.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn ordinary_stop_precedes_queued_memory_work_without_a_debugger() {
    let mut bridge = bridge();
    let owner = runtime(1);
    bridge.initialize(owner.handle()).unwrap();
    let answer = bridge.memory.request(true);
    owner.handle().request_stop().unwrap();
    assert!(!bridge.service_control().unwrap());
    assert!(matches!(answer.try_recv(), Err(mpsc::TryRecvError::Empty)));
    drop(bridge);
    assert!(matches!(
        answer.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn memory_control_service_yields_with_requests_still_pending() {
    let mut bridge = bridge();
    let answers: Vec<_> = (0..20).map(|_| bridge.memory.request(false)).collect();
    assert!(bridge.service_control().unwrap());
    assert!(answers[..16].iter().all(|answer| answer.try_recv().is_ok()));
    assert!(
        answers[16..]
            .iter()
            .all(|answer| matches!(answer.try_recv(), Err(mpsc::TryRecvError::Empty)))
    );
    assert!(bridge.has_control_work());
    assert!(bridge.service_control().unwrap());
    assert!(answers[16..].iter().all(|answer| answer.try_recv().is_ok()));
    assert!(!bridge.has_control_work());
}

fn bridge() -> Bridge {
    EngineBridge::new(
        RustFixture::new("private-bridge-guards"),
        ContextLimits::default(),
        EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        None,
        8,
    )
    .unwrap()
}
fn runtime(jobs: usize) -> EventLoop<Bridge> {
    EventLoop::new(
        bridge(),
        SchedulerConfig {
            max_jobs: jobs,
            ..Default::default()
        },
        None,
    )
    .unwrap()
}
struct Return;
impl HostFunction for Return {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        call.import_data(JsData::Undefined)
    }
}
fn callback<E: JsEngine>(bridge: &EngineBridge<E>) -> CallbackJob {
    callback_with(bridge, Rc::new(Return))
}
fn callback_with<E: JsEngine>(
    bridge: &EngineBridge<E>,
    function: Rc<dyn HostFunction>,
) -> CallbackJob {
    let mut job = None;
    bridge
        .engine
        .with_scope(
            bridge.context,
            &support::NoLoop,
            &mut |scope: &dyn JsScope| {
                let function = scope
                    .host_function(
                        &JsString::from_utf8("real Rust callback"),
                        1,
                        function.clone(),
                    )
                    .map_err(|error| scope.detach(error))?;
                let receiver = scope
                    .import_data(JsData::Number(11.0))
                    .map_err(|error| scope.detach(error))?;
                let argument = scope
                    .import_data(JsData::Number(29.0))
                    .map_err(|error| scope.detach(error))?;
                job = Some(CallbackJob {
                    function: scope.retain(function)?,
                    receiver: Some(scope.retain(receiver)?),
                    arguments: vec![scope.retain(argument)?],
                });
                Ok(())
            },
        )
        .unwrap();
    job.unwrap()
}
fn assert_callback_preserved(bridge: &Bridge, job: &CallbackJob) {
    bridge
        .engine
        .with_scope(
            bridge.context,
            &support::NoLoop,
            &mut |scope: &dyn JsScope| {
                let function = scope
                    .local(&job.function)
                    .map_err(|error| scope.detach(error))?;
                assert_eq!(scope.kind(function)?, ValueKind::Function);
                let receiver = scope
                    .local(job.receiver.as_ref().unwrap())
                    .map_err(|error| scope.detach(error))?;
                assert_eq!(
                    scope
                        .export_data(receiver, ExportLimits::default())
                        .map_err(|error| scope.detach(error))?,
                    JsData::Number(11.0)
                );
                assert_eq!(job.arguments.len(), 1);
                let argument = scope
                    .local(&job.arguments[0])
                    .map_err(|error| scope.detach(error))?;
                assert_eq!(
                    scope
                        .export_data(argument, ExportLimits::default())
                        .map_err(|error| scope.detach(error))?,
                    JsData::Number(29.0)
                );
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn capability_discovery_facade_rejects_admission_and_preserves_owned_callbacks() {
    let bridge = bridge();
    let facade = BridgeHost {
        bridge: &bridge,
        local: None,
    };
    let rejected = facade.post(callback(&bridge)).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Closed);
    assert_callback_preserved(&bridge, &rejected.value);
    let rejected = facade
        .timer(Duration::ZERO, None, rejected.value)
        .unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Closed);
    assert_callback_preserved(&bridge, &rejected.value);
    assert_eq!(
        facade.request_stop().unwrap_err().code,
        FailureCode::ClosedContext
    );
    assert_eq!(
        facade.reserve_operation(1).unwrap_err().code,
        FailureCode::ClosedContext
    );
    assert_eq!(
        facade.cancel(TaskId {
            epoch: 1,
            slot: 0,
            generation: 1
        }),
        CancelOutcome::NotFound
    );
    assert_eq!(
        facade.cancel_operation(OperationId {
            epoch: 1,
            context: 1,
            slot: 0,
            generation: 1
        }),
        CancelOutcome::NotFound
    );

    let provider = runtime(1);
    let reservation = provider.host().reserve_operation(4096).unwrap();
    let job = callback(&bridge);
    assert_eq!(
        facade
            .bind_callback(reservation, job.function, None)
            .unwrap_err()
            .code,
        FailureCode::ClosedContext
    );
    assert_eq!(provider.handle().stats().operations, 0);
    let mut resolver = None;
    bridge
        .engine
        .with_scope(
            bridge.context,
            &support::NoLoop,
            &mut |scope: &dyn JsScope| {
                resolver = Some(
                    scope
                        .deferred()
                        .map_err(|error| scope.detach(error))?
                        .resolver,
                );
                Ok(())
            },
        )
        .unwrap();
    let reservation = provider.host().reserve_operation(4096).unwrap();
    assert_eq!(
        facade
            .bind_deferred(reservation, resolver.unwrap(), None)
            .unwrap_err()
            .code,
        FailureCode::ClosedContext
    );
    assert_eq!(provider.handle().stats().operations, 0);
}

#[test]
fn invalid_callable_receiver_and_argument_are_rejected_before_scheduler_admission() {
    for invalid in 0..3 {
        let runtime = runtime(1);
        let bridge = runtime.driver();
        let foreign = self::bridge();
        let local = runtime.host();
        let facade = BridgeHost {
            bridge,
            local: Some(&local),
        };
        let mut job = callback(bridge);
        if invalid == 0 {
            job.function = job.arguments.pop().unwrap();
        } else if invalid == 1 {
            job.receiver = Some(callback(&foreign).function);
        } else {
            job.arguments = vec![callback(&foreign).function];
        }
        let expected = if invalid == 0 {
            FailureCode::InvalidHandle
        } else {
            FailureCode::WrongContext
        };
        let rejected = facade.post(job).unwrap_err();
        assert!(
            matches!(rejected.reason, RejectionReason::InvalidData(ref error) if error.code == expected)
        );
        let rejected = facade
            .timer(Duration::ZERO, None, rejected.value)
            .unwrap_err();
        assert!(
            matches!(rejected.reason, RejectionReason::InvalidData(error) if error.code == expected)
        );
        assert_eq!(runtime.stats().live_jobs, 0);
    }
}

#[test]
fn scheduler_saturation_returns_whole_callback_roots_for_post_and_timer_retry() {
    let runtime = runtime(1);
    let bridge = runtime.driver();
    let local = runtime.host();
    let facade = BridgeHost {
        bridge,
        local: Some(&local),
    };
    let first = facade.post(callback(bridge)).unwrap();
    let rejected = facade.post(callback(bridge)).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Full);
    assert_callback_preserved(bridge, &rejected.value);
    let rejected = facade
        .timer(Duration::ZERO, None, rejected.value)
        .unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Full);
    assert_callback_preserved(bridge, &rejected.value);
    assert_eq!(facade.cancel(first), CancelOutcome::Canceled);
    assert_eq!(facade.cancel(first), CancelOutcome::NotFound);
    let timer = facade
        .timer(Duration::from_secs(3600), None, rejected.value)
        .unwrap();
    assert_eq!(runtime.stats().live_jobs, 1);
    assert_eq!(facade.cancel(timer), CancelOutcome::Canceled);
    assert_eq!(runtime.stats().live_jobs, 0);
}

#[test]
fn operation_cancel_forwarding_reaches_the_reserved_terminal_lane() {
    let mut runtime = runtime(1);
    let local = runtime.host();
    let ticket = {
        let bridge = runtime.driver();
        let facade = BridgeHost {
            bridge,
            local: Some(&local),
        };
        let reservation = facade.reserve_operation(4096).unwrap();
        let job = callback(bridge);
        let ticket = facade
            .bind_callback(reservation, job.function, None)
            .unwrap();
        assert_eq!(
            facade.cancel_operation(ticket.id()),
            CancelOutcome::Canceled
        );
        ticket
    };
    let id = ticket.id();
    drop(ticket);
    runtime.tick(false).unwrap();
    assert_eq!(runtime.handle().stats().operations, 0);
    assert_eq!(local.cancel_operation(id), CancelOutcome::NotFound);
}

#[test]
fn closed_bridge_rejects_entry_before_running_the_scope_body() {
    let runtime = runtime(1);
    let bridge = runtime.driver();
    let descriptor = bridge.descriptor().clone();
    bridge.close().unwrap();
    let entered = Cell::new(false);
    let error = bridge
        .enter(&runtime.host(), &mut |_: &dyn JsScope| {
            entered.set(true);
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, FailureCode::ClosedContext);
    assert!(!entered.get());
    assert_eq!(bridge.debug_state(), DebugExecutionState::Closed);
    assert_eq!(bridge.descriptor().adapter_id, descriptor.adapter_id);
    assert!(!bridge.accepts_messages());
}

#[test]
fn job_diagnostics_show_the_work_kind_and_source_origin_without_entering_the_engine() {
    let bridge = bridge();
    let job = callback(&bridge);
    let diagnostic = format!("{:?}", EngineJob::Callback(job));
    assert!(diagnostic.starts_with("Callback("));
    let work = EngineJob::Work(Box::new(|_: &dyn JsScope| -> Result<(), FailureReport> {
        panic!("diagnostic must not execute work")
    }));
    assert_eq!(format!("{work:?}"), "Work(owned scoped body)");
    let command = EngineCommand::new(
        crate::vm::command::Kind::Work(Box::new(|_: &dyn JsScope| -> Result<(), FailureReport> {
            panic!("diagnostic must not execute command")
        })),
        Rc::new(RefCell::new(crate::vm::command::Captured::Empty)),
    );
    assert_eq!(
        format!("{:?}", EngineJob::Command(command)),
        "Command(owner capture)"
    );
    let source = ScriptSource::new(
        JsString::from_utf8("exact named Rust program"),
        "rust-fixture://diagnostic".into(),
    );
    assert!(format!("{:?}", EngineJob::Submit(source)).starts_with("Submit("));
}

#[test]
fn unconfigured_message_and_unbound_completion_reject_before_entering_a_scope() {
    let (mut bridge, _, _) = fault_bridge(None);
    let provider = runtime(1);
    assert_eq!(
        bridge
            .deliver_message(JsData::Null, &provider.host())
            .unwrap_err()
            .code,
        FailureCode::Unsupported
    );
    let id = OperationId {
        epoch: 1,
        context: 1,
        slot: 0,
        generation: 1,
    };
    assert_eq!(
        bridge
            .deliver_completion(
                id,
                CompletionPayload::Success(JsData::Null),
                &provider.host()
            )
            .unwrap_err()
            .code,
        FailureCode::InvalidHandle
    );
    let facade = BridgeHost {
        bridge: &bridge,
        local: None,
    };
    bridge
        .engine
        .with_scope(bridge.context, &facade, &mut |scope: &dyn JsScope| {
            assert_eq!(
                scope.kind(scope.global().map_err(|error| scope.detach(error))?)?,
                ValueKind::Object
            );
            Ok(())
        })
        .unwrap();
}

// The wrapper substitutes only optional interface metadata. Value operations and
// context lifetimes still use the real Rust fixture/ValueStore implementation.
struct ModuleEngine {
    inner: RustFixture<Inline>,
    flags: ModuleFeatures,
}
impl JsEngine for ModuleEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        self.inner.descriptor()
    }
    fn create_context(&self, limits: ContextLimits) -> Result<ContextId, FailureReport> {
        self.inner.create_context(limits)
    }
    fn close_context(&self, context: ContextId) -> Result<(), FailureReport> {
        self.inner.close_context(context)
    }
    fn debug_backend(
        &self,
        context: ContextId,
    ) -> Result<Option<Rc<dyn DebugBackend>>, FailureReport> {
        self.inner.debug_backend(context)
    }
    fn with_scope(
        &self,
        context: ContextId,
        host: &dyn HostServices,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        self.inner
            .with_scope(context, host, &mut |scope: &dyn JsScope| {
                body.run(&ModuleScope {
                    inner: scope,
                    flags: self.flags,
                })
            })
    }
}
struct ModuleScope<'a> {
    inner: &'a dyn JsScope,
    flags: ModuleFeatures,
}
impl JsScope for ModuleScope<'_> {
    fn data_limits(&self) -> ExportLimits {
        self.inner.data_limits()
    }
    fn context(&self) -> ContextId {
        self.inner.context()
    }
    fn global<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.global()
    }
    fn kind(&self, v: ScopedValue<'_>) -> Result<ValueKind, FailureReport> {
        self.inner.kind(v)
    }
    fn local<'s>(&'s self, v: &RootedValue) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.local(v)
    }
    fn retain(&self, v: ScopedValue<'_>) -> Result<RootedValue, FailureReport> {
        self.inner.retain(v)
    }
    fn import_data<'s>(&'s self, v: JsData) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.import_data(v)
    }
    fn export_data<'s>(&'s self, v: ScopedValue<'s>, l: ExportLimits) -> JsResult<'s, JsData> {
        self.inner.export_data(v, l)
    }
    fn get<'s>(&'s self, v: ScopedValue<'s>, k: PropertyKey<'_>) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.get(v, k)
    }
    fn set<'s>(
        &'s self,
        o: ScopedValue<'s>,
        k: PropertyKey<'_>,
        v: ScopedValue<'s>,
    ) -> JsResult<'s, ()> {
        self.inner.set(o, k, v)
    }
    fn call<'s>(
        &'s self,
        f: ScopedValue<'s>,
        r: Option<ScopedValue<'s>>,
        a: &[ScopedValue<'s>],
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.call(f, r, a)
    }
    fn construct<'s>(
        &'s self,
        f: ScopedValue<'s>,
        a: &[ScopedValue<'s>],
        n: Option<ScopedValue<'s>>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.construct(f, a, n)
    }
    fn host_function<'s>(
        &'s self,
        n: &JsString,
        a: u16,
        f: Rc<dyn HostFunction>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.host_function(n, a, f)
    }
    fn deferred<'s>(&'s self) -> JsResult<'s, Deferred<'s>> {
        self.inner.deferred()
    }
    fn settle<'s>(&'s self, d: RootedDeferred, s: Settlement<'s>) -> JsResult<'s, ()> {
        self.inner.settle(d, s)
    }
    fn drain_microtasks(&self, h: NonZeroUsize) -> Result<CheckpointReport, FailureReport> {
        self.inner.drain_microtasks(h)
    }
    fn detach(&self, e: ScopedError<'_>) -> FailureReport {
        self.inner.detach(e)
    }
    fn source(&self) -> Option<&dyn SourceApi> {
        self.inner.source()
    }
    fn compiler(&self) -> Option<&dyn CompilerApi> {
        self.inner.compiler()
    }
    fn artifacts(&self) -> Option<&dyn ArtifactApi> {
        self.inner.artifacts()
    }
    fn modules(&self) -> Option<&dyn ModuleApi> {
        Some(self)
    }
}

impl ModuleApi for ModuleScope<'_> {
    fn features(&self) -> ModuleFeatures {
        self.flags
    }
    fn prepare(&self, _: &ScriptSource, _: &JsString) -> Result<ModuleId, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "metadata-only module interface",
        ))
    }
    fn imports(&self, _: &ModuleId) -> Result<Vec<JsString>, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "metadata-only module interface",
        ))
    }
    fn link(&self, _: &ModuleId, _: &[ModuleBinding]) -> Result<(), FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "metadata-only module interface",
        ))
    }
    fn bind(&self, m: &ModuleId, b: &[ModuleBinding]) -> Result<(), FailureReport> {
        self.link(m, b)
    }
    fn namespace<'s>(&'s self, m: &ModuleId) -> JsResult<'s, ScopedValue<'s>> {
        self.evaluate(m)
    }
    fn evaluation_state(&self, _: &ModuleId) -> Result<ModuleEvaluationState, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "metadata only",
        ))
    }
    fn define_builtin(&self, _: &JsString, _: &[BuiltinExport]) -> Result<ModuleId, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "metadata only",
        ))
    }
    fn images(&self) -> Option<&dyn ModuleImageApi> {
        None
    }
    fn evaluate<'s>(&'s self, _: &ModuleId) -> JsResult<'s, ScopedValue<'s>> {
        Err(FailureReport::fault(FailureCode::Unsupported, "metadata-only module interface").into())
    }
}
#[test]
fn bridge_discovers_each_optional_module_flag_from_the_scoped_interface() {
    for (top_level_await, dynamic_import) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let bridge = EngineBridge::new(
            ModuleEngine {
                inner: RustFixture::new("module-metadata"),
                flags: ModuleFeatures {
                    top_level_await,
                    dynamic_import,
                },
            },
            ContextLimits::default(),
            EngineRequirements {
                entry: EntryRequirement::Module,
                top_level_await,
                dynamic_import,
                debug: DebugRequirements::disabled(),
                ..Default::default()
            },
            None,
            1,
        )
        .unwrap();
        assert!(bridge.features().modules);
        assert_eq!(bridge.features().top_level_await, top_level_await);
        assert_eq!(bridge.features().dynamic_import, dynamic_import);
        assert!(!bridge.features().source);
        assert!(!bridge.features().compiler);
        assert!(!bridge.features().artifacts);
        bridge.close().unwrap();
        assert_eq!(bridge.features().top_level_await, top_level_await);
    }
}

use crate::vm::faults::{self, Control, FaultEngine, ScopeFault};
type FaultRuntime = EventLoop<EngineBridge<FaultEngine>>;
fn fault_runtime(
    handler: Option<Rc<dyn HostMessageHandler>>,
) -> (FaultRuntime, Control, support::FixtureProbe<Inline>) {
    let (bridge, control, probe) = fault_bridge(handler);
    (
        EventLoop::new(bridge, SchedulerConfig::default(), None).unwrap(),
        control,
        probe,
    )
}
fn fault_bridge(
    handler: Option<Rc<dyn HostMessageHandler>>,
) -> (
    EngineBridge<FaultEngine>,
    Control,
    support::FixtureProbe<Inline>,
) {
    let (engine, control, probe) = FaultEngine::new(true);
    let bridge = EngineBridge::new(
        engine,
        ContextLimits::default(),
        EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        handler,
        8,
    )
    .unwrap();
    (bridge, control, probe)
}
struct ObservedCallback {
    calls: Rc<Cell<usize>>,
    throws: bool,
}
impl HostFunction for ObservedCallback {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(
            call.export_data(call.receiver(), ExportLimits::default())?,
            JsData::Number(11.0)
        );
        assert_eq!(call.arguments().len(), 1);
        assert_eq!(
            call.export_data(call.arguments()[0], ExportLimits::default())?,
            JsData::Number(29.0)
        );
        let value = call.import_data(JsData::String(JsString::from_utf8("callback threw")))?;
        if self.throws {
            Err(ScopedError::Exception(value))
        } else {
            Ok(value)
        }
    }
}
struct CountCallback(Rc<Cell<usize>>);
impl HostFunction for CountCallback {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.set(self.0.get() + 1);
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn direct_callback_jobs_validate_each_root_before_entering_the_real_callable() {
    for invalid in 0..4 {
        let (mut runtime, _, probe) = fault_runtime(None);
        let (foreign, _, _) = fault_runtime(None);
        let calls = Rc::new(Cell::new(0));
        let mut job = callback_with(
            runtime.driver(),
            Rc::new(ObservedCallback {
                calls: calls.clone(),
                throws: false,
            }),
        );
        match invalid {
            1 => job.function = callback(foreign.driver()).function,
            2 => job.receiver = Some(callback(foreign.driver()).function),
            3 => job.arguments = vec![callback(foreign.driver()).function],
            _ => {}
        }
        runtime.host().post(EngineJob::Callback(job)).unwrap();
        if invalid == 0 {
            runtime.tick(false).unwrap();
            assert_eq!(calls.get(), 1);
            assert_eq!(probe.contexts_closed(), 0);
        } else {
            assert_eq!(
                runtime.tick(false).unwrap_err().code,
                FailureCode::WrongContext
            );
            assert_eq!(calls.get(), 0);
            assert_eq!(probe.contexts_closed(), 1);
            assert!(runtime.tick(false).unwrap().stopped);
        }
        assert_eq!(runtime.stats().live_jobs, 0);
    }
}

#[test]
fn foreign_function_and_adapter_kind_failures_reject_admission_without_losing_retryable_jobs() {
    let (mut runtime, control, _) = fault_runtime(None);
    let (foreign, _, _) = fault_runtime(None);
    let local = runtime.host();
    let facade = BridgeHost {
        bridge: runtime.driver(),
        local: Some(&local),
    };
    let foreign_job = callback(foreign.driver());
    assert!(
        matches!(facade.post(foreign_job).unwrap_err().reason, RejectionReason::InvalidData(error) if error.code == FailureCode::WrongContext)
    );
    for target in 0..3 {
        let mut job = callback(runtime.driver());
        if target == 0 {
            job.function = job.arguments.pop().unwrap();
        } else if target == 1 {
            job.receiver = Some(callback(foreign.driver()).function);
        } else {
            job.arguments = vec![callback(foreign.driver()).function];
        }
        let expected = if target == 0 {
            FailureCode::InvalidHandle
        } else {
            FailureCode::WrongContext
        };
        assert!(
            matches!(facade.post(job).unwrap_err().reason, RejectionReason::InvalidData(error) if error.code == expected)
        );
    }
    assert_eq!(runtime.stats().live_jobs, 0);
    let mut no_receiver = callback(runtime.driver());
    no_receiver.receiver = None;
    facade.validate_job(&no_receiver).unwrap();
    let calls = Rc::new(Cell::new(0));
    let job = callback_with(
        runtime.driver(),
        Rc::new(ObservedCallback {
            calls: calls.clone(),
            throws: false,
        }),
    );
    let binding = callback(runtime.driver());
    let local_binding = callback(runtime.driver());
    control.scope.set(ScopeFault::Kind);
    let rejected = facade.post(job).unwrap_err();
    assert!(
        matches!(rejected.reason, RejectionReason::InvalidData(ref error) if error.code == FailureCode::EngineFailure)
    );
    let rejected = facade
        .timer(Duration::ZERO, None, rejected.value)
        .unwrap_err();
    assert!(
        matches!(rejected.reason, RejectionReason::InvalidData(ref error) if error.code == FailureCode::EngineFailure)
    );
    let reservation = facade.reserve_operation(4096).unwrap();
    assert_eq!(
        facade
            .bind_callback(reservation, binding.function, None)
            .unwrap_err()
            .code,
        FailureCode::EngineFailure
    );
    assert_eq!(runtime.handle().stats().operations, 0);
    assert!(runtime.driver().bindings.borrow().is_empty());
    control.scope.set(ScopeFault::Local);
    let reservation = facade.reserve_operation(4096).unwrap();
    assert_eq!(
        facade
            .bind_callback(reservation, local_binding.function, None)
            .unwrap_err()
            .code,
        FailureCode::InvalidHandle
    );
    assert_eq!(runtime.handle().stats().operations, 0);
    assert!(runtime.driver().bindings.borrow().is_empty());
    control.scope.set(ScopeFault::None);
    facade.post(rejected.value).unwrap();
    runtime.tick(false).unwrap();
    assert_eq!(calls.get(), 1);
}

#[test]
fn source_submission_surfaces_missing_interface_and_original_adapter_exceptions() {
    for fault in [
        ScopeFault::SourceMissing,
        ScopeFault::Evaluate,
        ScopeFault::None,
    ] {
        let (mut runtime, control, probe) = fault_runtime(None);
        control.scope.set(fault);
        runtime
            .host()
            .post(EngineJob::Submit(faults::source()))
            .unwrap();
        if fault == ScopeFault::None {
            runtime.tick(false).unwrap();
            assert_eq!(control.program_calls.get(), 1);
        } else {
            let error = runtime.tick(false).unwrap_err();
            if fault == ScopeFault::SourceMissing {
                assert_eq!(error.code, FailureCode::Unsupported);
            } else {
                assert_eq!(
                    (error.class, error.code),
                    (FailureClass::Script, FailureCode::ScriptException)
                );
                assert_eq!(
                    error.thrown,
                    Some(JsData::String(JsString::from_utf8("source entry failed")))
                );
            }
            assert_eq!(control.program_calls.get(), 0);
            assert_eq!(probe.contexts_closed(), 1);
            assert!(runtime.tick(false).unwrap().stopped);
        }
    }
}

#[test]
fn callback_exceptions_keep_the_original_value_and_never_retry_after_owner_teardown() {
    let (mut runtime, _, probe) = fault_runtime(None);
    let calls = Rc::new(Cell::new(0));
    let job = callback_with(
        runtime.driver(),
        Rc::new(ObservedCallback {
            calls: calls.clone(),
            throws: true,
        }),
    );
    runtime.host().post(EngineJob::Callback(job)).unwrap();
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(
        (error.class, error.code),
        (FailureClass::Script, FailureCode::ScriptException)
    );
    assert_eq!(
        error.thrown,
        Some(JsData::String(JsString::from_utf8("callback threw")))
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(probe.contexts_closed(), 1);
    assert!(runtime.tick(false).unwrap().stopped);
    assert_eq!(calls.get(), 1);
}

struct ThrowingMessage(Rc<Cell<usize>>);
impl HostMessageHandler for ThrowingMessage {
    fn deliver<'s>(
        &self,
        scope: &'s dyn JsScope,
        _: &dyn HostServices,
        payload: JsData,
    ) -> JsResult<'s, ()> {
        self.0.set(self.0.get() + 1);
        assert_eq!(payload, JsData::Number(42.0));
        let value = scope.import_data(JsData::String(JsString::from_utf8("message threw")))?;
        Err(ScopedError::Exception(value))
    }
}
#[test]
fn routed_message_errors_release_the_permit_and_preserve_the_original_thrown_value() {
    let calls = Rc::new(Cell::new(0));
    let (mut runtime, _, probe) = fault_runtime(Some(Rc::new(ThrowingMessage(calls.clone()))));
    runtime.handle().try_send(JsData::Number(42.0)).unwrap();
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(
        (error.class, error.code),
        (FailureClass::Script, FailureCode::ScriptException)
    );
    assert_eq!(
        error.thrown,
        Some(JsData::String(JsString::from_utf8("message threw")))
    );
    assert_eq!(runtime.handle().stats().messages, 0);
    assert_eq!(probe.contexts_closed(), 1);
    assert!(runtime.tick(false).unwrap().stopped);
    assert_eq!(calls.get(), 1);
}

#[test]
fn completion_materialization_failure_releases_binding_and_never_invokes_the_callback() {
    for fault in [
        ScopeFault::Local,
        ScopeFault::Import,
        ScopeFault::ImportSecond,
        ScopeFault::None,
    ] {
        for success in [true, false] {
            let (mut runtime, control, probe) = fault_runtime(None);
            let calls = Rc::new(Cell::new(0));
            let local = runtime.host();
            let ticket = {
                let bridge = runtime.driver();
                let facade = BridgeHost {
                    bridge,
                    local: Some(&local),
                };
                let job = callback_with(bridge, Rc::new(CountCallback(calls.clone())));
                let reservation = facade.reserve_operation(4096).unwrap();
                facade
                    .bind_callback(reservation, job.function, None)
                    .unwrap()
            };
            let payload = if success {
                CompletionPayload::Success(JsData::Number(42.0))
            } else {
                CompletionPayload::Failure(OperationFailure {
                    code: "NativeError".into(),
                    message: JsString::from_utf8("native rejected"),
                    details: None,
                })
            };
            assert!(matches!(
                ticket.complete(payload),
                PublishOutcome::Published
            ));
            control.scope.set(fault);
            if fault == ScopeFault::None {
                runtime.tick(false).unwrap();
                assert_eq!(calls.get(), 1);
            } else {
                let error = runtime.tick(false).unwrap_err();
                assert_eq!(
                    (error.class, error.code),
                    (FailureClass::Engine, FailureCode::EngineFailure)
                );
                if matches!(fault, ScopeFault::Import | ScopeFault::ImportSecond) {
                    assert_eq!(
                        error.thrown,
                        Some(JsData::String(JsString::from_utf8("data import failed")))
                    );
                }
                assert_eq!(calls.get(), 0);
                assert_eq!(probe.contexts_closed(), 1);
                assert!(runtime.tick(false).unwrap().stopped);
            }
            assert_eq!(runtime.handle().stats().operations, 0);
            assert!(runtime.driver().bindings.borrow().is_empty());
        }
    }
}

#[test]
fn constructor_scope_failure_closes_the_real_new_context_before_returning_the_fault() {
    let (engine, control, probe) = FaultEngine::new(true);
    control.scope.set(ScopeFault::WithScope);
    let result = EngineBridge::new(
        engine,
        ContextLimits::default(),
        EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        None,
        8,
    );
    let error = result
        .err()
        .expect("adapter scope discovery must fail construction");
    assert_eq!(
        (error.class, error.code),
        (FailureClass::Engine, FailureCode::EngineFailure)
    );
    assert_eq!(probe.contexts_created(), 1);
    assert_eq!(probe.contexts_closed(), 1);
    assert!(!probe.is_context_open());
}

#[test]
fn native_close_panic_is_detached_after_real_context_cleanup_and_never_retried() {
    use crate::vm::faults::{FaultEngine, ScopeFault};
    let (engine, control, probe) = FaultEngine::new(false);
    let bridge = EngineBridge::new(
        engine,
        ContextLimits::default(),
        EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        None,
        8,
    )
    .unwrap();
    control.scope.set(ScopeFault::ClosePanic);
    assert_eq!(bridge.close().unwrap_err().code, FailureCode::HostPanic);
    assert_eq!(probe.contexts_closed(), 1);
    bridge.close().unwrap();
    assert_eq!(probe.contexts_closed(), 1);
}
#[test]
fn incomplete_internal_development_setup_rejects_missing_backend_before_entry() {
    let owner = runtime(8);
    let mut incomplete = bridge();
    incomplete.debug_enabled = true;
    incomplete.debug = None;
    assert_eq!(
        incomplete.initialize(owner.handle()).unwrap_err().code,
        FailureCode::Unsupported
    );
}

#[test]
fn backend_discovery_failure_rolls_back_the_real_context_before_scope_entry() {
    let (engine, control, probe) = FaultEngine::new(false);
    control.scope.set(ScopeFault::DebugBackend);
    let error = EngineBridge::new(
        engine,
        ContextLimits::default(),
        EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        None,
        8,
    )
    .err()
    .expect("backend discovery must reject construction");
    assert_eq!(error.code, FailureCode::EngineFailure);
    assert_eq!(probe.contexts_created(), 1);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(probe.checkpoint_calls(), 0);
}

#[test]
fn adapter_checkpoint_failure_closes_once_without_entering_queued_application_work() {
    let (mut runtime, control, probe) = fault_runtime(None);
    let calls = Rc::new(Cell::new(0));
    let entered = calls.clone();
    runtime
        .host()
        .post(EngineJob::Work(Box::new(move |_: &dyn JsScope| {
            entered.set(entered.get() + 1);
            Ok(())
        })))
        .unwrap();
    control.scope.set(ScopeFault::Checkpoint);
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(error.code, FailureCode::EngineFailure);
    assert_eq!(
        calls.get(),
        0,
        "startup checkpoint precedes application entry"
    );
    assert_eq!(probe.contexts_closed(), 1);
    assert!(runtime.tick(false).unwrap().stopped);
    assert_eq!(calls.get(), 0);
}

fn development_bridge() -> (
    EngineBridge<FaultEngine>,
    Control,
    support::FixtureProbe<Inline>,
) {
    let (engine, control, probe) = FaultEngine::new(false);
    let bridge = EngineBridge::new(
        engine,
        ContextLimits::default(),
        EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: DebugRequirements::default(),
            ..Default::default()
        },
        None,
        8,
    )
    .unwrap();
    (bridge, control, probe)
}

#[test]
fn urgent_debug_stop_reaches_the_existing_application_transport_before_scope_entry() {
    let (mut bridge, control, probe) = development_bridge();
    let runtime = runtime(1);
    bridge.initialize(runtime.handle()).unwrap();
    control.attach.set(crate::vm::faults::AttachFault::Accept);
    let _debugger = bridge
        .attach_debugger(DebugSessionOptions::default())
        .unwrap();
    assert_eq!(bridge.debug_state(), DebugExecutionState::Running);
    assert!(!bridge.service_control().unwrap());
    assert!(
        !runtime.is_stop_requested(),
        "idle debug coordination leaves application admission open"
    );
    let coordinator = bridge.coordinator.as_ref().unwrap().clone();
    coordinator.stop_signal()().unwrap();
    assert!(
        bridge.service_control().unwrap(),
        "the attached session has urgent stop work"
    );
    assert!(runtime.is_stop_requested());
    let called = Cell::new(false);
    assert_eq!(
        bridge
            .enter(&runtime.host(), &mut |_: &dyn JsScope| {
                called.set(true);
                Ok(())
            })
            .unwrap_err()
            .code,
        FailureCode::EngineFailure
    );
    assert!(!called.get());
    bridge.close().unwrap();
    assert_eq!(probe.contexts_closed(), 1);
}

struct FailingWake;
impl crate::transport::Wake for FailingWake {
    fn wake(&self) -> std::io::Result<()> {
        Err(std::io::Error::other("real transport wake seam failed"))
    }
}

#[test]
fn native_stop_hook_reports_an_accepted_debug_wake_failure_without_reopening_admission() {
    let (mut bridge, _, probe) = development_bridge();
    let transport = crate::transport::Transport::<JsData>::new(
        1,
        &SchedulerConfig::default(),
        Arc::new(FailingWake),
    )
    .unwrap();
    let handle = transport.handle();
    bridge.initialize(handle.clone()).unwrap();
    assert!(transport.arm());
    assert_eq!(
        handle.request_stop().unwrap_err().code,
        FailureCode::EngineFailure
    );
    assert_eq!(
        handle.stats().wake_calls,
        2,
        "ordinary and debug wake each tried once"
    );
    assert!(transport.stopped());
    assert!(bridge.coordinator.as_ref().unwrap().is_stopping());
    assert_eq!(
        bridge.service_control().unwrap_err().code,
        FailureCode::EngineFailure,
        "debug stop forwarding also preserves the real application wake failure"
    );
    bridge.close().unwrap();
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn pending_debug_service_failure_is_detached_before_application_entry_and_context_close() {
    let (bridge, control, probe) = development_bridge();
    let mut runtime = EventLoop::new(bridge, SchedulerConfig::default(), None).unwrap();
    control.attach.set(crate::vm::faults::AttachFault::Accept);
    let handle = runtime
        .driver()
        .attach_debugger(DebugSessionOptions::default())
        .unwrap();
    assert_eq!(runtime.driver().debug_state(), DebugExecutionState::Running);
    control.scope.set(ScopeFault::DebugService);
    handle.request_pause().unwrap();
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(error.code, FailureCode::EngineFailure);
    assert_eq!(probe.checkpoint_calls(), 0);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(runtime.driver().debug_state(), DebugExecutionState::Closed);
    assert!(runtime.tick(false).unwrap().stopped);
}

struct ProjectionDrop(Rc<Cell<usize>>);
impl ScopeCall for ProjectionDrop {
    fn run(&mut self, _: &dyn JsScope) -> Result<(), FailureReport> {
        panic!("projecting ownership never executes the retained body")
    }
}
impl Drop for ProjectionDrop {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn typed_job_projections_preserve_matching_ownership_and_return_other_variants_intact() {
    let bridge = bridge();
    let job = EngineJob::Callback(callback(&bridge));
    let callback = job.into_submit().unwrap_err().into_callback().unwrap();
    assert_callback_preserved(&bridge, &callback);

    let source = faults::source();
    let pointer = source.text.as_units().as_ptr();
    let source = EngineJob::Submit(source)
        .into_callback()
        .unwrap_err()
        .into_submit()
        .unwrap();
    assert_eq!(source.text.as_units().as_ptr(), pointer);
    assert_eq!(source.origin, faults::source().origin);

    for command in [false, true] {
        let drops = Rc::new(Cell::new(0));
        let body = Box::new(ProjectionDrop(drops.clone()));
        let job = if command {
            EngineJob::Command(EngineCommand::new(
                crate::vm::command::Kind::Work(body),
                Rc::new(RefCell::new(crate::vm::command::Captured::Empty)),
            ))
        } else {
            EngineJob::Work(body)
        };
        let job = job.into_callback().unwrap_err().into_submit().unwrap_err();
        assert_eq!(drops.get(), 0, "rejected projections retain the owned body");
        assert_eq!(matches!(job, EngineJob::Command(_)), command);
        drop(job);
        assert_eq!(drops.get(), 1);
    }
}

#[test]
fn debug_stop_receipts_keep_accepted_wake_failures_fatal_and_closed_sessions_harmless() {
    for outcome in [
        Ok(DebugControlReceipt::Accepted),
        Ok(DebugControlReceipt::AlreadyRequested),
        Err(DebugError::Closed),
        Err(DebugError::NotAttached),
    ] {
        stop_outcome(outcome).unwrap();
    }
    for outcome in [
        Err(DebugError::Capacity),
        Err(DebugError::Unsupported),
        Ok(DebugControlReceipt::AcceptedWakeFailed {
            error: DebugError::Closed,
        }),
    ] {
        let error = stop_outcome(outcome).unwrap_err();
        assert_eq!(error.class, FailureClass::Host);
        assert_eq!(error.code, FailureCode::EngineFailure);
    }
    for outcome in [
        Err(DebugError::EngineFailure(FailureReport::fault(
            FailureCode::InvalidHandle,
            "native stop error",
        ))),
        Ok(DebugControlReceipt::AcceptedWakeFailed {
            error: DebugError::WakeFailed(FailureReport::fault(
                FailureCode::InvalidHandle,
                "owner stop wake error",
            )),
        }),
    ] {
        assert_eq!(
            stop_outcome(outcome).unwrap_err().code,
            FailureCode::InvalidHandle
        );
    }
}

#[test]
fn failed_constructor_stages_report_without_creating_or_leaking_a_native_context() {
    for scenario in 0..4 {
        let (engine, control, probe) = FaultEngine::new(false);
        let limits = ContextLimits {
            max_persistent_roots: if scenario == 0 { 0 } else { 1 },
            ..Default::default()
        };
        let operations = match scenario {
            1 => 0,
            2 => usize::MAX,
            _ => 8,
        };
        if scenario == 3 {
            control.scope.set(ScopeFault::CreateContext);
        }
        let error = EngineBridge::new(
            engine,
            limits,
            EngineRequirements {
                entry: EntryRequirement::ExistingCallables,
                debug: DebugRequirements::disabled(),
                ..Default::default()
            },
            None,
            operations,
        )
        .err()
        .expect("the selected constructor stage must reject before native creation");
        assert_eq!(
            error.code,
            if scenario == 3 {
                FailureCode::EngineFailure
            } else {
                FailureCode::Capacity
            }
        );
        assert_eq!(probe.contexts_created(), 0);
        assert_eq!(probe.contexts_closed(), 0);
    }
}

#[test]
fn unsupported_profile_closes_the_discovered_context_before_returning_constructor_failure() {
    let (engine, _, probe) = FaultEngine::new(false);
    let result = EngineBridge::new(
        engine,
        ContextLimits::default(),
        EngineRequirements {
            entry: EntryRequirement::Source,
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        None,
        8,
    );
    assert_eq!(result.err().unwrap().code, FailureCode::Unsupported);
    assert_eq!(probe.contexts_created(), 1);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn debugger_detach_failure_still_closes_the_native_context_and_is_not_retried() {
    let (bridge, control, probe) = development_bridge();
    let mut runtime = EventLoop::new(bridge, SchedulerConfig::default(), None).unwrap();
    control.attach.set(crate::vm::faults::AttachFault::Accept);
    let _handle = runtime
        .driver()
        .attach_debugger(DebugSessionOptions::default())
        .unwrap();
    control.scope.set(ScopeFault::DebugDetach);
    let error = runtime.shutdown().unwrap_err();
    assert_eq!(
        (error.class, error.code),
        (FailureClass::Host, FailureCode::EngineFailure)
    );
    assert_eq!(probe.contexts_closed(), 1);
    runtime.shutdown().unwrap();
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn failed_debug_setup_and_closed_entry_never_run_the_real_scope_body() {
    let owner = runtime(1);
    let (mut bridge, _, probe) = development_bridge();
    assert_eq!(bridge.debug_state(), DebugExecutionState::Detached);
    bridge.debug = None;
    assert_eq!(
        bridge.initialize(owner.handle()).unwrap_err().code,
        FailureCode::Unsupported
    );
    bridge.close().unwrap();
    let called = Cell::new(false);
    assert_eq!(
        bridge
            .enter(&owner.host(), &mut |_: &dyn JsScope| {
                called.set(true);
                Ok(())
            })
            .unwrap_err()
            .code,
        FailureCode::ClosedContext
    );
    assert!(!called.get());
    assert_eq!(bridge.debug_state(), DebugExecutionState::Closed);
    assert_eq!(probe.contexts_closed(), 1);
}

struct CleanupHook {
    calls: Rc<Cell<usize>>,
    drops: Rc<Cell<usize>>,
    fail: bool,
}
impl CancelNative for CleanupHook {
    fn cancel(&self) -> Result<(), OperationFailure> {
        self.calls.set(self.calls.get() + 1);
        if self.fail {
            Err(OperationFailure {
                code: "NativeCancel".into(),
                message: JsString::from_utf8("native cancel failed"),
                details: None,
            })
        } else {
            Ok(())
        }
    }
}
impl Drop for CleanupHook {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
        assert!(!self.fail, "native cancellation owner destructor failed");
    }
}

#[test]
fn teardown_keeps_the_first_hook_error_and_closes_after_all_binding_destructors() {
    let (mut runtime, _, probe) = fault_runtime(None);
    let calls = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let local = runtime.host();
    let facade = BridgeHost {
        bridge: runtime.driver(),
        local: Some(&local),
    };
    let mut tickets = Vec::new();
    for fail in [true, false] {
        let hook = Rc::new(CleanupHook {
            calls: calls.clone(),
            drops: drops.clone(),
            fail,
        });
        facade
            .bridge
            .engine
            .with_scope(
                facade.bridge.context,
                &facade,
                &mut |scope: &dyn JsScope| {
                    let resolver = scope
                        .deferred()
                        .map_err(|error| scope.detach(error))?
                        .resolver;
                    tickets.push(facade.bind_deferred(
                        facade.reserve_operation(4096)?,
                        resolver,
                        Some(hook.clone()),
                    )?);
                    Ok(())
                },
            )
            .unwrap();
    }
    assert_eq!(
        runtime.shutdown().unwrap_err().code,
        FailureCode::EngineFailure
    );
    assert_eq!(calls.get(), 2);
    assert_eq!(drops.get(), 2);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(runtime.handle().stats().operations, 0);
    drop(tickets);
    runtime.shutdown().unwrap();
}

#[test]
fn deferred_delivery_preserves_both_settlements_and_detaches_native_settlement_failure() {
    for fail in [false, true] {
        for success in [false, true] {
            let (mut runtime, control, probe) = fault_runtime(None);
            let local = runtime.host();
            let facade = BridgeHost {
                bridge: runtime.driver(),
                local: Some(&local),
            };
            let mut output = None;
            facade
                .bridge
                .engine
                .with_scope(
                    facade.bridge.context,
                    &facade,
                    &mut |scope: &dyn JsScope| {
                        let deferred = scope.deferred().map_err(|error| scope.detach(error))?;
                        let promise = scope.retain(deferred.promise)?;
                        let ticket = facade.bind_deferred(
                            facade.reserve_operation(4096)?,
                            deferred.resolver,
                            None,
                        )?;
                        output = Some((promise, ticket));
                        Ok(())
                    },
                )
                .unwrap();
            let (promise, ticket) = output.unwrap();
            if fail {
                control.scope.set(ScopeFault::Settle);
            }
            let payload = if success {
                CompletionPayload::Success(JsData::Number(42.0))
            } else {
                CompletionPayload::Failure(OperationFailure {
                    code: "NativeResult".into(),
                    message: JsString::from_utf8("native operation failed"),
                    details: None,
                })
            };
            assert!(matches!(
                ticket.complete(payload),
                PublishOutcome::Published
            ));
            if fail {
                let error = runtime.tick(false).unwrap_err();
                assert_eq!(
                    (error.class, error.code),
                    (FailureClass::Engine, FailureCode::EngineFailure)
                );
                assert_eq!(
                    error.thrown,
                    Some(JsData::String(JsString::from_utf8(
                        "native settlement failed"
                    )))
                );
                assert_eq!(probe.contexts_closed(), 1);
            } else {
                runtime.tick(false).unwrap();
                let (rejected, value) = probe.promise(&promise).unwrap().unwrap();
                assert_eq!(rejected, !success);
                if success {
                    assert_eq!(value, JsData::Number(42.0));
                } else {
                    assert_eq!(
                        value,
                        failure_data(OperationFailure {
                            code: "NativeResult".into(),
                            message: JsString::from_utf8("native operation failed"),
                            details: None,
                        })
                    );
                }
                assert_eq!(probe.contexts_closed(), 0);
            }
            assert!(runtime.driver().bindings.borrow().is_empty());
            assert_eq!(runtime.handle().stats().operations, 0);
        }
    }
}

#[test]
fn canceled_deferred_delivery_uses_the_terminal_lane_and_invokes_native_cancel_once() {
    let (mut runtime, _, probe) = fault_runtime(None);
    let calls = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let local = runtime.host();
    let facade = BridgeHost {
        bridge: runtime.driver(),
        local: Some(&local),
    };
    let mut output = None;
    facade
        .bridge
        .engine
        .with_scope(
            facade.bridge.context,
            &facade,
            &mut |scope: &dyn JsScope| {
                let deferred = scope.deferred().map_err(|error| scope.detach(error))?;
                let promise = scope.retain(deferred.promise)?;
                let ticket = facade.bind_deferred(
                    facade.reserve_operation(4096)?,
                    deferred.resolver,
                    Some(Rc::new(CleanupHook {
                        calls: calls.clone(),
                        drops: drops.clone(),
                        fail: false,
                    })),
                )?;
                output = Some((promise, ticket));
                Ok(())
            },
        )
        .unwrap();
    let (promise, ticket) = output.unwrap();
    assert_eq!(local.cancel_operation(ticket.id()), CancelOutcome::Canceled);
    drop(ticket);
    runtime.tick(false).unwrap();
    let (rejected, value) = probe.promise(&promise).unwrap().unwrap();
    assert!(rejected);
    assert_eq!(
        value,
        failure_data(OperationFailure {
            code: "Canceled".into(),
            message: JsString::from_utf8("Canceled"),
            details: None,
        })
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(drops.get(), 1);
    assert_eq!(probe.contexts_closed(), 0);
    assert_eq!(runtime.handle().stats().operations, 0);
    assert!(runtime.driver().bindings.borrow().is_empty());
    runtime.shutdown().unwrap();
    assert_eq!(calls.get(), 1);
}
#[test]
fn optional_dynamic_module_defaults_reject_tokens_without_native_support() {
    let engine = RustFixture::<Inline>::new("metadata");
    let context = engine.create_context(ContextLimits::default()).unwrap();
    engine
        .with_scope(context, &support::NoLoop, &mut |scope: &dyn JsScope| {
            let module = ModuleScope {
                inner: scope,
                flags: ModuleFeatures::default(),
            };
            assert!(module.take_dynamic_imports()?.requests.is_empty());
            assert_eq!(
                module
                    .finish_dynamic_import(
                        ModuleImportId {
                            context,
                            sequence: 1
                        },
                        Err(FailureReport::fault(FailureCode::Unsupported, "test"))
                    )
                    .unwrap_err()
                    .code,
                FailureCode::Unsupported
            );
            Ok(())
        })
        .unwrap();
    engine.close_context(context).unwrap();
}
