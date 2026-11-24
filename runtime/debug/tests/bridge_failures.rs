//! Bridge ownership and failure contracts over actual Rust ValueStore fixtures.
//! Promise observations read fixture state; no JavaScript syntax or native VM is simulated.
mod support;
use coldmind_runtime::{debug::DebugRequirements, engine::*, scheduler::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::{Inline, RustFixture};

fn requirements() -> EngineRequirements {
    EngineRequirements {
        entry: EntryRequirement::ExistingCallables,
        debug: DebugRequirements::disabled(),
        ..Default::default()
    }
}
fn make_bridge<E: JsEngine>(engine: E, limits: ContextLimits) -> EngineBridge<E> {
    EngineBridge::new(engine, limits, requirements(), None, 8).unwrap()
}
fn work<E: JsEngine>(
    runtime: &mut EventLoop<EngineBridge<E>>,
    body: impl FnMut(&dyn JsScope) -> Result<(), FailureReport> + 'static,
) {
    let id = runtime
        .host()
        .post(EngineJob::Work(Box::new(body)))
        .unwrap();
    assert_eq!(runtime.run_until(id).unwrap(), TurnOutcome::Completed);
}
fn invoke<E: JsEngine>(runtime: &mut EventLoop<EngineBridge<E>>, function: Rc<dyn HostFunction>) {
    work(runtime, move |scope| {
        let function = scope
            .host_function(&JsString::from_utf8("contract"), 0, function.clone())
            .map_err(|error| scope.detach(error))?;
        scope
            .call(function, None, &[])
            .map_err(|error| scope.detach(error))?;
        Ok(())
    });
}
fn callable<E: JsEngine>(
    runtime: &mut EventLoop<EngineBridge<E>>,
    function: Rc<dyn HostFunction>,
) -> RootedValue {
    let result = Rc::new(RefCell::new(None));
    let output = result.clone();
    work(runtime, move |scope| {
        let local = scope
            .host_function(&JsString::from_utf8("target"), 2, function.clone())
            .map_err(|error| scope.detach(error))?;
        *output.borrow_mut() = Some(scope.retain(local)?);
        Ok(())
    });
    result.borrow_mut().take().unwrap()
}
fn retained<E: JsEngine>(runtime: &mut EventLoop<EngineBridge<E>>, data: JsData) -> RootedValue {
    let result = Rc::new(RefCell::new(None));
    let output = result.clone();
    let mut data = Some(data);
    work(runtime, move |scope| {
        let local = scope
            .import_data(data.take().unwrap())
            .map_err(|error| scope.detach(error))?;
        *output.borrow_mut() = Some(scope.retain(local)?);
        Ok(())
    });
    result.borrow_mut().take().unwrap()
}
#[derive(Default)]
struct Calls {
    count: Cell<usize>,
    arguments: RefCell<Vec<JsData>>,
    throw: bool,
}
impl HostFunction for Calls {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.count.set(self.count.get() + 1);
        for &argument in call.arguments() {
            self.arguments
                .borrow_mut()
                .push(call.export_data(argument, ExportLimits::default())?);
        }
        let value = call.import_data(JsData::Number(73.0))?;
        if self.throw {
            Err(ScopedError::Exception(value))
        } else {
            Ok(value)
        }
    }
}
struct BeginCallback {
    callback: RefCell<Option<RootedValue>>,
    ticket: Rc<RefCell<Option<CompletionTicket>>>,
    cancel: Option<Rc<dyn CancelNative>>,
}
impl HostFunction for BeginCallback {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let reservation = call.host().reserve_operation(16_384)?;
        *self.ticket.borrow_mut() = Some(call.host().bind_callback(
            reservation,
            self.callback.borrow_mut().take().unwrap(),
            self.cancel.clone(),
        )?);
        call.import_data(JsData::Undefined)
    }
}
fn begin_callback<E: JsEngine>(
    runtime: &mut EventLoop<EngineBridge<E>>,
    callback: RootedValue,
    cancel: Option<Rc<dyn CancelNative>>,
) -> CompletionTicket {
    let output = Rc::new(RefCell::new(None));
    invoke(
        runtime,
        Rc::new(BeginCallback {
            callback: RefCell::new(Some(callback)),
            ticket: output.clone(),
            cancel,
        }),
    );
    output.borrow_mut().take().unwrap()
}
struct BeginDeferred {
    promise: Rc<RefCell<Option<RootedValue>>>,
    ticket: Rc<RefCell<Option<CompletionTicket>>>,
    cancel: Option<Rc<dyn CancelNative>>,
}
impl HostFunction for BeginDeferred {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let deferred = call.deferred()?;
        *self.promise.borrow_mut() = Some(call.retain(deferred.promise)?);
        let reservation = call.host().reserve_operation(16_384)?;
        *self.ticket.borrow_mut() = Some(call.host().bind_deferred(
            reservation,
            deferred.resolver,
            self.cancel.clone(),
        )?);
        Ok(deferred.promise)
    }
}
fn begin_deferred<E: JsEngine>(
    runtime: &mut EventLoop<EngineBridge<E>>,
    cancel: Option<Rc<dyn CancelNative>>,
) -> (RootedValue, CompletionTicket) {
    let promise = Rc::new(RefCell::new(None));
    let ticket = Rc::new(RefCell::new(None));
    invoke(
        runtime,
        Rc::new(BeginDeferred {
            promise: promise.clone(),
            ticket: ticket.clone(),
            cancel,
        }),
    );
    let promise = promise.borrow_mut().take().unwrap();
    let ticket = ticket.borrow_mut().take().unwrap();
    (promise, ticket)
}
#[derive(Clone, Copy)]
enum HookMode {
    Ok,
    Error,
    Panic,
}
struct Hook {
    calls: Rc<Cell<usize>>,
    mode: HookMode,
}
impl CancelNative for Hook {
    fn cancel(&self) -> Result<(), OperationFailure> {
        self.calls.set(self.calls.get() + 1);
        match self.mode {
            HookMode::Ok => Ok(()),
            HookMode::Error => Err(OperationFailure {
                code: "NativeCancel".into(),
                message: JsString::from_utf8("cancel failed"),
                details: Some(JsData::Number(91.0)),
            }),
            HookMode::Panic => panic!("native cancellation panic fixture"),
        }
    }
}
fn hook(mode: HookMode) -> (Rc<Cell<usize>>, Rc<dyn CancelNative>) {
    let calls = Rc::new(Cell::new(0));
    (calls.clone(), Rc::new(Hook { calls, mode }))
}
fn failure() -> OperationFailure {
    OperationFailure {
        code: "RejectedByNative".into(),
        message: JsString::from_units(vec![0xd800, 0]),
        details: Some(JsData::Array(vec![JsData::Number(9.0)])),
    }
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

#[test]
fn deferred_success_and_rejection_settle_the_actual_promise_once_without_canceling_native() {
    for reject in [false, true] {
        let fixture = RustFixture::<Inline>::new("deferred-contract");
        let probe = fixture.probe();
        let mut runtime = EventLoop::new(
            make_bridge(fixture, ContextLimits::default()),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();
        let (cancels, cancel) = hook(HookMode::Ok);
        let (promise, ticket) = begin_deferred(&mut runtime, Some(cancel));
        assert_eq!(probe.promise(&promise).unwrap(), None);
        let id = ticket.id();
        let expected = if reject {
            failure_data(failure())
        } else {
            JsData::Record(vec![(JsString::from_utf8("result"), JsData::Number(42.0))])
        };
        let payload = if reject {
            CompletionPayload::Failure(failure())
        } else {
            CompletionPayload::Success(expected.clone())
        };
        assert!(matches!(
            ticket.complete(payload),
            PublishOutcome::Published
        ));
        runtime.tick(false).unwrap();
        assert_eq!(probe.promise(&promise).unwrap(), Some((reject, expected)));
        assert_eq!(runtime.host().cancel_operation(id), CancelOutcome::NotFound);
        runtime.tick(false).unwrap();
        assert_eq!(cancels.get(), 0);
        assert_eq!(runtime.handle().stats().operations, 0);
        drop(runtime);
        assert_eq!(cancels.get(), 0);
        assert_eq!(probe.contexts_closed(), 1);
        assert_eq!(
            probe.promise(&promise).unwrap_err().code,
            FailureCode::ClosedContext
        );
    }
}

#[test]
fn rejected_callback_receives_error_then_undefined_with_lossless_details() {
    let mut runtime = EventLoop::new(
        make_bridge(
            RustFixture::<Inline>::new("callback-reject"),
            ContextLimits::default(),
        ),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let calls = Rc::new(Calls::default());
    let callback = callable(&mut runtime, calls.clone());
    let (cancels, cancel) = hook(HookMode::Ok);
    let ticket = begin_callback(&mut runtime, callback, Some(cancel));
    assert!(matches!(
        ticket.complete(CompletionPayload::Failure(failure())),
        PublishOutcome::Published
    ));
    runtime.tick(false).unwrap();
    assert_eq!(calls.count.get(), 1);
    assert_eq!(
        *calls.arguments.borrow(),
        [failure_data(failure()), JsData::Undefined]
    );
    drop(runtime);
    assert_eq!(cancels.get(), 0);
}

#[test]
fn callback_exception_is_not_retried_and_successful_native_work_is_not_canceled() {
    let fixture = RustFixture::<Inline>::new("throwing-completion");
    let probe = fixture.probe();
    let mut runtime = EventLoop::new(
        make_bridge(fixture, ContextLimits::default()),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let calls = Rc::new(Calls {
        throw: true,
        ..Default::default()
    });
    let callback = callable(&mut runtime, calls.clone());
    let (cancels, cancel) = hook(HookMode::Ok);
    let ticket = begin_callback(&mut runtime, callback, Some(cancel));
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Published
    ));
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(
        (error.class, error.code),
        (FailureClass::Script, FailureCode::ScriptException)
    );
    assert_eq!(error.thrown, Some(JsData::Number(73.0)));
    assert_eq!(calls.count.get(), 1);
    assert_eq!(cancels.get(), 0);
    assert!(runtime.tick(false).unwrap().stopped);
    assert_eq!(calls.count.get(), 1);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn cancellation_and_abandonment_reject_real_promises_and_call_native_hook_once() {
    for abandon in [false, true] {
        let fixture = RustFixture::<Inline>::new("cancel-deferred");
        let probe = fixture.probe();
        let mut runtime = EventLoop::new(
            make_bridge(fixture, ContextLimits::default()),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();
        let (cancels, cancel) = hook(HookMode::Ok);
        let (promise, ticket) = begin_deferred(&mut runtime, Some(cancel));
        let id = ticket.id();
        if !abandon {
            assert_eq!(runtime.host().cancel_operation(id), CancelOutcome::Canceled);
        }
        drop(ticket);
        runtime.tick(false).unwrap();
        let Some((rejected, JsData::Record(ref fields))) = probe.promise(&promise).unwrap() else {
            panic!("native terminal must settle the real fixture Promise");
        };
        assert!(rejected);
        let expected = if abandon { "Abandoned" } else { "Canceled" };
        assert!(
            fields
                .iter()
                .any(|(key, value)| key.to_utf8().unwrap() == "code"
                    && value == &JsData::String(JsString::from_utf8(expected)))
        );
        assert_eq!(cancels.get(), 1);
        assert_eq!(runtime.host().cancel_operation(id), CancelOutcome::NotFound);
        drop(runtime);
        assert_eq!(cancels.get(), 1);
    }
}

#[test]
fn cancellation_hook_error_or_panic_does_not_retry_terminal_callback_or_skip_cleanup() {
    for mode in [HookMode::Error, HookMode::Panic] {
        let fixture = RustFixture::<Inline>::new("hook-failure");
        let probe = fixture.probe();
        let mut runtime = EventLoop::new(
            make_bridge(fixture, ContextLimits::default()),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();
        let calls = Rc::new(Calls::default());
        let callback = callable(&mut runtime, calls.clone());
        let (cancels, cancel) = hook(mode);
        let ticket = begin_callback(&mut runtime, callback, Some(cancel));
        assert_eq!(
            runtime.host().cancel_operation(ticket.id()),
            CancelOutcome::Canceled
        );
        drop(ticket);
        let error = runtime.tick(false).unwrap_err();
        assert_eq!(error.class, FailureClass::Host);
        assert_eq!(
            error.code,
            if matches!(mode, HookMode::Panic) {
                FailureCode::HostPanic
            } else {
                FailureCode::EngineFailure
            }
        );
        assert_eq!(calls.count.get(), 1);
        assert_eq!(cancels.get(), 1);
        assert_eq!(probe.contexts_closed(), 1);
        assert_eq!(runtime.handle().stats().operations, 0);
        runtime.tick(false).unwrap();
        drop(runtime);
        assert_eq!(calls.count.get(), 1);
        assert_eq!(cancels.get(), 1);
    }
}

#[test]
fn stop_closes_all_active_bindings_even_when_native_hooks_fail() {
    let fixture = RustFixture::<Inline>::new("stop-active-bindings");
    let probe = fixture.probe();
    let mut runtime = EventLoop::new(
        make_bridge(fixture, ContextLimits::default()),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let mut tickets = Vec::new();
    let mut hooks = Vec::new();
    for mode in [HookMode::Error, HookMode::Panic, HookMode::Ok] {
        let (calls, hook) = hook(mode);
        let (_promise, ticket) = begin_deferred(&mut runtime, Some(hook));
        tickets.push(ticket);
        hooks.push(calls);
    }
    runtime.host().request_stop();
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(error.class, FailureClass::Host);
    assert!(matches!(
        error.code,
        FailureCode::HostPanic | FailureCode::EngineFailure
    ));
    assert!(hooks.iter().all(|calls| calls.get() == 1));
    assert_eq!(probe.contexts_closed(), 1);
    assert!(!probe.is_context_open());
    assert_eq!(runtime.handle().stats().operations, 0);
    for ticket in tickets {
        assert!(matches!(
            ticket.complete(CompletionPayload::Success(JsData::Null)),
            PublishOutcome::Rejected {
                reason: PublishRejection::Closed | PublishRejection::Stale,
                ..
            }
        ));
    }
    runtime.tick(false).unwrap();
    drop(runtime);
    assert!(hooks.iter().all(|calls| calls.get() == 1));
}

enum AttemptTarget {
    Callback(RootedValue),
    Deferred(RootedDeferred),
}
struct Attempt {
    target: RefCell<Option<AttemptTarget>>,
    reservation: RefCell<Option<OperationReservation>>,
    error: Rc<RefCell<Option<FailureReport>>>,
    cancel: Rc<dyn CancelNative>,
    stop_before_bind: bool,
}
impl HostFunction for Attempt {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let reservation = match self.reservation.borrow_mut().take() {
            Some(reservation) => reservation,
            None => call.host().reserve_operation(4096)?,
        };
        if self.stop_before_bind {
            call.host().request_stop()?;
        }
        let result = match self.target.borrow_mut().take().unwrap() {
            AttemptTarget::Callback(callback) => {
                call.host()
                    .bind_callback(reservation, callback, Some(self.cancel.clone()))
            }
            AttemptTarget::Deferred(resolver) => {
                call.host()
                    .bind_deferred(reservation, resolver, Some(self.cancel.clone()))
            }
        };
        *self.error.borrow_mut() =
            Some(result.expect_err("invalid operation bind must be rejected"));
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn rejected_noncallable_and_foreign_targets_roll_back_reservations_and_roots() {
    for invalid in 0..3 {
        let fixture = RustFixture::<Inline>::new("binding-rollback");
        let probe = fixture.probe();
        let mut runtime = EventLoop::new(
            make_bridge(fixture, ContextLimits::default()),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();
        let foreign = RustFixture::<Inline>::new("foreign-target");
        let foreign_context = foreign.create_context(ContextLimits::default()).unwrap();
        let target = if invalid == 0 {
            AttemptTarget::Callback(retained(&mut runtime, JsData::Null))
        } else {
            let mut target = None;
            foreign
                .with_scope(
                    foreign_context,
                    &support::NoLoop,
                    &mut |scope: &dyn JsScope| {
                        target = Some(if invalid == 1 {
                            let local = scope
                                .host_function(
                                    &JsString::from_utf8("foreign"),
                                    0,
                                    Rc::new(Calls::default()),
                                )
                                .map_err(|error| scope.detach(error))?;
                            AttemptTarget::Callback(scope.retain(local)?)
                        } else {
                            AttemptTarget::Deferred(
                                scope
                                    .deferred()
                                    .map_err(|error| scope.detach(error))?
                                    .resolver,
                            )
                        });
                        Ok(())
                    },
                )
                .unwrap();
            target.unwrap()
        };
        let error = Rc::new(RefCell::new(None));
        let (cancels, cancel) = hook(HookMode::Ok);
        invoke(
            &mut runtime,
            Rc::new(Attempt {
                target: RefCell::new(Some(target)),
                reservation: RefCell::new(None),
                error: error.clone(),
                cancel,
                stop_before_bind: false,
            }),
        );
        assert_eq!(
            error.borrow().as_ref().unwrap().code,
            if invalid == 0 {
                FailureCode::InvalidHandle
            } else {
                FailureCode::WrongContext
            }
        );
        assert_eq!(runtime.handle().stats().operations, 0);
        assert_eq!(runtime.handle().stats().reserved_payload_bytes, 0);
        assert_eq!(probe.live_roots(), Some(0));
        assert_eq!(
            cancels.get(),
            0,
            "native work never started for rejected bindings"
        );
        work(&mut runtime, |_| Ok(()));
        assert!(runtime.last_failure().is_none());
        foreign.close_context(foreign_context).unwrap();
    }
}

#[test]
fn foreign_reservation_binding_failure_releases_both_transports_without_native_cancel() {
    let mut runtime = EventLoop::new(
        make_bridge(
            RustFixture::<Inline>::new("foreign-reservation"),
            ContextLimits::default(),
        ),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let other = EventLoop::new(
        make_bridge(
            RustFixture::<Inline>::new("reservation-owner"),
            ContextLimits::default(),
        ),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let reservation = other.host().reserve_operation(4096).unwrap();
    let callback = callable(&mut runtime, Rc::new(Calls::default()));
    let error = Rc::new(RefCell::new(None));
    let (cancels, cancel) = hook(HookMode::Ok);
    invoke(
        &mut runtime,
        Rc::new(Attempt {
            target: RefCell::new(Some(AttemptTarget::Callback(callback))),
            reservation: RefCell::new(Some(reservation)),
            error: error.clone(),
            cancel,
            stop_before_bind: false,
        }),
    );
    assert_eq!(
        error.borrow().as_ref().unwrap().code,
        FailureCode::WrongOwner
    );
    assert_eq!(runtime.handle().stats().operations, 0);
    assert_eq!(other.handle().stats().operations, 0);
    assert_eq!(cancels.get(), 0);
}

#[test]
fn stopping_between_reserve_and_bind_rolls_back_without_starting_native_work() {
    let fixture = RustFixture::<Inline>::new("stopped-bind");
    let probe = fixture.probe();
    let mut runtime = EventLoop::new(
        make_bridge(fixture, ContextLimits::default()),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let callback = callable(&mut runtime, Rc::new(Calls::default()));
    let error = Rc::new(RefCell::new(None));
    let (cancels, cancel) = hook(HookMode::Ok);
    let attempt: Rc<dyn HostFunction> = Rc::new(Attempt {
        target: RefCell::new(Some(AttemptTarget::Callback(callback))),
        reservation: RefCell::new(None),
        error: error.clone(),
        cancel,
        stop_before_bind: true,
    });
    let task = runtime
        .host()
        .post(EngineJob::Work(Box::new(move |scope: &dyn JsScope| {
            let function = scope
                .host_function(&JsString::from_utf8("stop-bind"), 0, attempt.clone())
                .map_err(|error| scope.detach(error))?;
            scope
                .call(function, None, &[])
                .map_err(|error| scope.detach(error))?;
            Ok(())
        })))
        .unwrap();
    assert_eq!(runtime.run_until(task).unwrap(), TurnOutcome::Stopped);
    assert_eq!(
        error.borrow().as_ref().unwrap().code,
        FailureCode::ClosedContext
    );
    assert_eq!(runtime.handle().stats().operations, 0);
    assert_eq!(cancels.get(), 0);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn constructor_rejection_closes_only_contexts_that_were_actually_created() {
    for scenario in 0..6 {
        let mut fixture = RustFixture::<Inline>::new("constructor-rollback");
        let probe = fixture.probe();
        let mut limits = ContextLimits::default();
        let mut required = requirements();
        let mut operations = 8;
        let (expected, contexts) = match scenario {
            0 => {
                required.entry = EntryRequirement::Source;
                (FailureCode::Unsupported, 1)
            }
            1 => {
                required.debug = DebugRequirements::default();
                (FailureCode::Unsupported, 1)
            }
            2 => {
                limits.max_persistent_roots = 0;
                (FailureCode::Capacity, 0)
            }
            3 => {
                operations = 0;
                (FailureCode::Capacity, 0)
            }
            4 => {
                let owner = std::thread::spawn(|| std::thread::current().id())
                    .join()
                    .unwrap();
                fixture = fixture.with_reported_owner(owner);
                (FailureCode::WrongOwner, 0)
            }
            _ => {
                operations = usize::MAX;
                (FailureCode::Capacity, 0)
            }
        };
        let error = EngineBridge::new(fixture, limits, required, None, operations)
            .err()
            .unwrap();
        assert_eq!(error.code, expected);
        assert_eq!(probe.contexts_created(), contexts);
        assert_eq!(probe.contexts_closed(), contexts);
        assert!(!probe.is_context_open());
    }
}

fn check_engine_selection<E: JsEngine, R: support::Representation>(
    engine: E,
    probe: support::FixtureProbe<R>,
) {
    let mut runtime = EventLoop::new(
        make_bridge(engine, ContextLimits::default()),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let (promise, ticket) = begin_deferred(&mut runtime, None);
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Published
    ));
    runtime.tick(false).unwrap();
    assert_eq!(
        probe.promise(&promise).unwrap(),
        Some((false, JsData::Number(42.0)))
    );
    runtime.host().request_stop();
    runtime.tick(false).unwrap();
    assert_eq!(probe.contexts_created(), 1);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn generic_inline_boxed_and_erased_engine_selection_share_the_same_real_binding_path() {
    let inline = RustFixture::<Inline>::new("generic-inline");
    let probe = inline.probe();
    check_engine_selection(inline, probe);
    let boxed = RustFixture::<support::Boxed>::new("generic-boxed");
    let probe = boxed.probe();
    check_engine_selection(boxed, probe);
    let erased = RustFixture::<Inline>::new("erased-inline");
    let probe = erased.probe();
    let erased: Box<dyn JsEngine> = Box::new(erased);
    check_engine_selection(erased, probe);
}

struct Messages(Rc<RefCell<Vec<JsData>>>);
impl HostMessageHandler for Messages {
    fn deliver<'s>(
        &self,
        _: &'s dyn JsScope,
        _: &dyn HostServices,
        payload: JsData,
    ) -> JsResult<'s, ()> {
        self.0.borrow_mut().push(payload);
        Ok(())
    }
}

#[test]
fn invalid_data_and_closed_admission_preserve_owned_payload_without_entering_handler() {
    let observed = Rc::new(RefCell::new(Vec::new()));
    let limits = ContextLimits {
        export: ExportLimits {
            max_bytes: 256,
            ..Default::default()
        },
        ..Default::default()
    };
    let bridge = EngineBridge::new(
        RustFixture::<Inline>::new("bounded-message"),
        limits,
        requirements(),
        Some(Rc::new(Messages(observed.clone()))),
        8,
    )
    .unwrap();
    let mut runtime = EventLoop::new(bridge, SchedulerConfig::default(), None).unwrap();
    let mut bytes = Vec::with_capacity(4096);
    bytes.extend([1, 2, 3]);
    let pointer = bytes.as_ptr();
    let rejected = runtime.handle().try_send(JsData::Bytes(bytes)).unwrap_err();
    assert!(matches!(
        rejected.reason,
        RejectionReason::InvalidData(FailureReport {
            code: FailureCode::Capacity,
            ..
        })
    ));
    assert!(
        matches!(&rejected.value, JsData::Bytes(bytes) if bytes.as_ptr() == pointer && bytes == &[1, 2, 3])
    );
    assert_eq!(runtime.handle().stats().messages, 0);
    assert!(observed.borrow().is_empty());
    runtime.handle().try_send(JsData::Number(7.0)).unwrap();
    runtime.tick(false).unwrap();
    assert_eq!(*observed.borrow(), [JsData::Number(7.0)]);
    runtime.host().request_stop();
    let rejected = runtime.handle().try_send(JsData::Number(9.0)).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Closed);
    assert_eq!(rejected.value, JsData::Number(9.0));
}

#[test]
fn invalid_real_fixture_resolver_is_fatal_without_invoking_native_cancel() {
    // Explicit adapter-fault fixture: the retained resolver wraps Null, so the
    // real fixture settle() rejects its type rather than simulating JS semantics.
    let fixture = RustFixture::<Inline>::new("faulty-resolver").with_invalid_resolver();
    let probe = fixture.probe();
    let mut runtime = EventLoop::new(
        make_bridge(fixture, ContextLimits::default()),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let (cancels, cancel) = hook(HookMode::Ok);
    let (promise, ticket) = begin_deferred(&mut runtime, Some(cancel));
    assert_eq!(probe.promise(&promise).unwrap(), None);
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Published
    ));
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(
        (error.class, error.code),
        (FailureClass::Engine, FailureCode::EngineFailure)
    );
    assert_eq!(cancels.get(), 0);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(runtime.handle().stats().operations, 0);
}

#[test]
fn continue_reports_callback_exception_once_and_dispatches_newer_work() {
    let reports = Rc::new(RefCell::new(Vec::new()));
    let observed = reports.clone();
    let sink: ReportSink = Rc::new(move |report| observed.borrow_mut().push(report.clone()));
    let fixture = RustFixture::<Inline>::new("continue-callback");
    let probe = fixture.probe();
    let mut runtime = EventLoop::with_policy(
        make_bridge(fixture, ContextLimits::default()),
        SchedulerConfig::default(),
        Some(sink),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    let calls = Rc::new(Calls {
        throw: true,
        ..Default::default()
    });
    let callback = callable(&mut runtime, calls.clone());
    let (cancels, cancel) = hook(HookMode::Ok);
    let ticket = begin_callback(&mut runtime, callback, Some(cancel));
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Published
    ));
    runtime.tick(false).unwrap();
    let newer = Rc::new(Cell::new(0));
    let entered = newer.clone();
    work(&mut runtime, move |_| {
        entered.set(entered.get() + 1);
        Ok(())
    });
    assert_eq!(newer.get(), 1);
    assert_eq!(calls.count.get(), 1);
    assert_eq!(reports.borrow().len(), 1);
    assert_eq!(reports.borrow()[0].class, FailureClass::Script);
    assert_eq!(cancels.get(), 0);
    assert!(probe.is_context_open());
    drop(runtime);
    assert_eq!(cancels.get(), 0);
}

#[test]
fn completion_import_failure_is_fatal_even_when_script_continue_is_enabled() {
    let reports = Rc::new(RefCell::new(Vec::new()));
    let observed = reports.clone();
    let sink: ReportSink = Rc::new(move |report| observed.borrow_mut().push(report.clone()));
    let fixture = RustFixture::<Inline>::new("import-limit");
    let probe = fixture.probe();
    let limits = ContextLimits {
        export: ExportLimits {
            max_bytes: 128,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut runtime = EventLoop::with_policy(
        make_bridge(fixture, limits),
        SchedulerConfig::default(),
        Some(sink),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    let calls = Rc::new(Calls::default());
    let callback = callable(&mut runtime, calls.clone());
    let ticket = begin_callback(&mut runtime, callback, None);
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Bytes(vec![5; 4096]))),
        PublishOutcome::Published
    ));
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(
        (error.class, error.code),
        (FailureClass::Engine, FailureCode::EngineFailure)
    );
    assert_eq!(calls.count.get(), 0);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(reports.borrow().len(), 1);
    assert_eq!(runtime.handle().stats().operations, 0);
    assert!(runtime.tick(false).unwrap().stopped);
}

struct PostingHook {
    host: Host<EngineJob>,
    calls: Rc<Cell<usize>>,
    posted: Rc<Cell<usize>>,
}
impl CancelNative for PostingHook {
    fn cancel(&self) -> Result<(), OperationFailure> {
        self.calls.set(self.calls.get() + 1);
        let observed = self.posted.clone();
        self.host
            .post(EngineJob::Work(Box::new(move |_: &dyn JsScope| {
                observed.set(observed.get() + 1);
                Ok(())
            })))
            .map_err(|_| OperationFailure {
                code: "PostRejected".into(),
                message: JsString::from_utf8("hook post was rejected"),
                details: None,
            })?;
        Ok(())
    }
}

#[test]
fn cancellation_hook_can_post_owner_work_without_a_scheduler_registry_borrow() {
    let mut runtime = EventLoop::new(
        make_bridge(
            RustFixture::<Inline>::new("hook-reentry"),
            ContextLimits::default(),
        ),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let calls = Rc::new(Calls::default());
    let callback = callable(&mut runtime, calls.clone());
    let canceled = Rc::new(Cell::new(0));
    let posted = Rc::new(Cell::new(0));
    let hook = Rc::new(PostingHook {
        host: runtime.host(),
        calls: canceled.clone(),
        posted: posted.clone(),
    });
    let ticket = begin_callback(&mut runtime, callback, Some(hook));
    assert_eq!(
        runtime.host().cancel_operation(ticket.id()),
        CancelOutcome::Canceled
    );
    drop(ticket);
    runtime.tick(false).unwrap();
    assert_eq!(canceled.get(), 1);
    assert_eq!(calls.count.get(), 1);
    assert_eq!(posted.get(), 1);
    assert!(runtime.last_failure().is_none());
}

type Admission = Rc<RefCell<Option<Result<TaskId, Rejected<CallbackJob>>>>>;
struct AdmitInvalid {
    job: RefCell<Option<CallbackJob>>,
    timer: bool,
    result: Admission,
}
impl HostFunction for AdmitInvalid {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let job = self.job.borrow_mut().take().unwrap();
        *self.result.borrow_mut() = Some(if self.timer {
            call.host().timer(std::time::Duration::ZERO, None, job)
        } else {
            call.host().post(job)
        });
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn callback_and_timer_admission_reject_noncallable_roots_before_queuing_a_turn() {
    for timer in [false, true] {
        let mut runtime = EventLoop::new(
            make_bridge(
                RustFixture::<Inline>::new("invalid-callback-admission"),
                ContextLimits::default(),
            ),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();
        let callback = retained(&mut runtime, JsData::Null);
        let result = Rc::new(RefCell::new(None));
        invoke(
            &mut runtime,
            Rc::new(AdmitInvalid {
                job: RefCell::new(Some(CallbackJob {
                    function: callback,
                    receiver: None,
                    arguments: Vec::new(),
                })),
                timer,
                result: result.clone(),
            }),
        );
        let rejected = result
            .borrow_mut()
            .take()
            .unwrap()
            .expect_err("noncallable root must be rejected before admission");
        assert_eq!(runtime.stats().live_jobs, 0);
        assert!(runtime.last_failure().is_none());
        let mut callback = Some(rejected.value.function);
        work(&mut runtime, move |scope| {
            let root = callback.take().unwrap();
            let local = scope.local(&root).map_err(|error| scope.detach(error))?;
            assert_eq!(
                scope
                    .export_data(local, ExportLimits::default())
                    .map_err(|error| scope.detach(error))?,
                JsData::Null
            );
            Ok(())
        });
    }
}

#[test]
fn callback_admission_rejects_foreign_receiver_and_arguments_while_preserving_roots() {
    for receiver in [false, true] {
        let mut runtime = EventLoop::new(
            make_bridge(
                RustFixture::<Inline>::new("foreign-job-admission"),
                ContextLimits::default(),
            ),
            SchedulerConfig::default(),
            None,
        )
        .unwrap();
        let calls = Rc::new(Calls::default());
        let function = callable(&mut runtime, calls.clone());
        let foreign = RustFixture::<Inline>::new("foreign-job-root");
        let context = foreign.create_context(ContextLimits::default()).unwrap();
        let mut foreign_root = None;
        foreign
            .with_scope(context, &support::NoLoop, &mut |scope: &dyn JsScope| {
                let value = scope
                    .import_data(JsData::Number(9.0))
                    .map_err(|error| scope.detach(error))?;
                foreign_root = Some(scope.retain(value)?);
                Ok(())
            })
            .unwrap();
        let mut job = CallbackJob {
            function,
            receiver: None,
            arguments: Vec::new(),
        };
        if receiver {
            job.receiver = foreign_root;
        } else {
            job.arguments.push(foreign_root.unwrap());
        }
        let result = Rc::new(RefCell::new(None));
        invoke(
            &mut runtime,
            Rc::new(AdmitInvalid {
                job: RefCell::new(Some(job)),
                timer: false,
                result: result.clone(),
            }),
        );
        let rejected = result
            .borrow_mut()
            .take()
            .unwrap()
            .expect_err("foreign receiver/arguments must fail before job admission");
        assert_eq!(calls.count.get(), 0);
        assert_eq!(runtime.stats().live_jobs, 0);
        let mut job = rejected.value;
        let foreign_root = if receiver {
            job.receiver.take().unwrap()
        } else {
            job.arguments.pop().unwrap()
        };
        foreign
            .with_scope(context, &support::NoLoop, &mut |scope: &dyn JsScope| {
                let value = scope
                    .local(&foreign_root)
                    .map_err(|error| scope.detach(error))?;
                assert_eq!(
                    scope
                        .export_data(value, ExportLimits::default())
                        .map_err(|error| scope.detach(error))?,
                    JsData::Number(9.0)
                );
                Ok(())
            })
            .unwrap();
        let valid = runtime.host().post(EngineJob::Callback(job)).unwrap();
        runtime.run_until(valid).unwrap();
        assert_eq!(calls.count.get(), 1);
    }
}

#[test]
fn native_hook_fault_cannot_be_hidden_by_callback_exception_in_continue_mode() {
    for mode in [HookMode::Error, HookMode::Panic] {
        let reports = Rc::new(RefCell::new(Vec::new()));
        let observed = reports.clone();
        let sink: ReportSink = Rc::new(move |report| observed.borrow_mut().push(report.clone()));
        let fixture = RustFixture::<Inline>::new("hook-fault-priority");
        let probe = fixture.probe();
        let mut runtime = EventLoop::with_policy(
            make_bridge(fixture, ContextLimits::default()),
            SchedulerConfig::default(),
            Some(sink),
            ScriptErrorPolicy::Continue,
        )
        .unwrap();
        let calls = Rc::new(Calls {
            throw: true,
            ..Default::default()
        });
        let callback = callable(&mut runtime, calls.clone());
        let (cancels, cancel) = hook(mode);
        let ticket = begin_callback(&mut runtime, callback, Some(cancel));
        assert_eq!(
            runtime.host().cancel_operation(ticket.id()),
            CancelOutcome::Canceled
        );
        drop(ticket);
        let error = runtime.tick(false).unwrap_err();
        assert_eq!(error.class, FailureClass::Host);
        assert_eq!(
            error.code,
            if matches!(mode, HookMode::Panic) {
                FailureCode::HostPanic
            } else {
                FailureCode::EngineFailure
            }
        );
        assert_eq!(calls.count.get(), 1);
        assert_eq!(cancels.get(), 1);
        assert_eq!(probe.contexts_closed(), 1);
        assert!(
            reports
                .borrow()
                .iter()
                .any(|report| report.class == FailureClass::Host)
        );
    }
}

struct DropPanicsAfterCancel {
    calls: Rc<Cell<usize>>,
    drops: Rc<Cell<usize>>,
}
impl CancelNative for DropPanicsAfterCancel {
    fn cancel(&self) -> Result<(), OperationFailure> {
        self.calls.set(self.calls.get() + 1);
        Ok(())
    }
}
impl Drop for DropPanicsAfterCancel {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
        panic!("native cancellation owner destructor panic");
    }
}
#[test]
fn cancellation_owner_destructor_panic_does_not_skip_other_bindings_or_context_close() {
    let engine = RustFixture::<Inline>::new("destructor-teardown-regression");
    let probe = engine.probe();
    let mut runtime = EventLoop::new(
        make_bridge(engine, ContextLimits::default()),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let bad_calls = Rc::new(Cell::new(0));
    let bad_drops = Rc::new(Cell::new(0));
    let bad: Rc<dyn CancelNative> = Rc::new(DropPanicsAfterCancel {
        calls: bad_calls.clone(),
        drops: bad_drops.clone(),
    });
    let (promise, ticket) = begin_deferred(&mut runtime, Some(bad));
    let (other_calls, other) = hook(HookMode::Ok);
    let (other_promise, other_ticket) = begin_deferred(&mut runtime, Some(other));
    assert_eq!(runtime.shutdown().unwrap_err().code, FailureCode::HostPanic);
    assert_eq!(bad_calls.get(), 1);
    assert_eq!(bad_drops.get(), 1);
    assert_eq!(other_calls.get(), 1);
    assert_eq!(
        probe.contexts_closed(),
        1,
        "context close must run after every binding cleanup attempt"
    );
    assert_eq!(
        probe.promise(&promise).unwrap_err().code,
        FailureCode::ClosedContext
    );
    assert_eq!(
        probe.promise(&other_promise).unwrap_err().code,
        FailureCode::ClosedContext
    );
    runtime.shutdown().unwrap();
    drop((ticket, other_ticket));
}
