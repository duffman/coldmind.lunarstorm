//! Public VM acceptance against actual QuickJS-NG JavaScript and native jobs.
#![cfg(feature = "quickjs-ng")]

use coldmind_runtime::{
    debug::DebugRequirements, engine::*, engines::quickjs_ng::QuickJsNgEngine,
    scheduler::ScriptErrorPolicy, vm::*,
};
use std::{cell::RefCell, num::NonZeroUsize, rc::Rc, time::Duration};

type NativeVm = VirtualMachine<QuickJsNgEngine>;
type Events = Rc<RefCell<Vec<JsData>>>;
type Tickets = Rc<RefCell<Vec<CompletionTicket>>>;

fn config() -> VmConfig {
    VmConfig {
        requirements: EngineRequirements {
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        ..Default::default()
    }
}
fn vm() -> NativeVm {
    VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config()).unwrap()
}
fn source(text: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(text), "file:///quickjs-ng-vm.js".into())
}
fn text(value: &str) -> JsData {
    JsData::String(JsString::from_utf8(value))
}
fn exported(vm: &mut NativeVm, root: RootedValue) -> JsData {
    let output = Rc::new(RefCell::new(None));
    let captured = output.clone();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let value = scope.local(&root).map_err(|error| scope.detach(error))?;
        *captured.borrow_mut() = Some(
            scope
                .export_data(value, ExportLimits::default())
                .map_err(|error| scope.detach(error))?,
        );
        Ok(())
    }))
    .unwrap();
    output.borrow_mut().take().unwrap()
}
fn evaluated(vm: &mut NativeVm, program: &str) -> JsData {
    let root = vm.evaluate(source(program)).unwrap();
    exported(vm, root)
}
fn bind(vm: &mut NativeVm, name: &str, callback: impl HostFunction + 'static) -> RootedValue {
    vm.bind_function(JsString::from_utf8(name), 1, Rc::new(callback))
        .unwrap()
}

struct Record(Events);
impl HostFunction for Record {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let data = call.export_data(call.arguments()[0], ExportLimits::default())?;
        self.0.borrow_mut().push(data);
        call.import_data(JsData::Undefined)
    }
}

struct Schedule {
    timer: bool,
    cancel: bool,
}
impl HostFunction for Schedule {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let job = CallbackJob {
            function: call.retain(call.arguments()[0])?,
            receiver: None,
            arguments: Vec::new(),
        };
        let admitted = if self.timer {
            call.host().timer(Duration::ZERO, None, job)
        } else {
            call.host().post(job)
        };
        let task = admitted.map_err(|_| {
            FailureReport::fault(FailureCode::Capacity, "callback admission failed")
        })?;
        if self.cancel {
            assert_eq!(call.host().cancel(task), CancelOutcome::Canceled);
        }
        call.import_data(JsData::Undefined)
    }
}

struct StartOperation {
    tickets: Tickets,
    promise: bool,
}
impl HostFunction for StartOperation {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let reservation = call.host().reserve_operation(4096)?;
        let (ticket, value) = if self.promise {
            let deferred = call.deferred()?;
            (
                call.host()
                    .bind_deferred(reservation, deferred.resolver, None)?,
                deferred.promise,
            )
        } else {
            let callback = call.retain(call.arguments()[0])?;
            (
                call.host().bind_callback(reservation, callback, None)?,
                call.import_data(JsData::Undefined)?,
            )
        };
        self.tickets.borrow_mut().push(ticket);
        Ok(value)
    }
}

#[test]
fn default_development_requires_a_qualified_native_debugger() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    assert!(
        vm.debug_descriptor()
            .unwrap()
            .capabilities
            .contains_all(coldmind_runtime::debug::DebugCapabilities::baseline())
    );
    vm.shutdown().unwrap();
}

#[test]
fn javascript_calls_keep_receiver_identity_and_scoped_construction_uses_new_target() {
    let mut vm = vm();
    let function = vm
        .evaluate(source(
            "(function (x) { 'use strict'; this.count += x; return this; })",
        ))
        .unwrap();
    let receiver = vm.evaluate(source("({count: 35})")).unwrap();
    let seven = vm.evaluate(source("7")).unwrap();
    let alias = vm.call(&function, Some(&receiver), &[seven]).unwrap();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let object = scope.local(&alias).map_err(|error| scope.detach(error))?;
        let value = scope
            .import_data(JsData::Number(43.0))
            .map_err(|error| scope.detach(error))?;
        scope
            .set(
                object,
                PropertyKey::String(&JsString::from_utf8("count")),
                value,
            )
            .map_err(|error| scope.detach(error))?;
        Ok(())
    }))
    .unwrap();
    assert_eq!(
        exported(&mut vm, receiver),
        JsData::Record(vec![(JsString::from_utf8("count"), JsData::Number(43.0))])
    );
    let strict = vm
        .evaluate(source(
            "(function () { 'use strict'; return this === undefined; })",
        ))
        .unwrap();
    let result = vm.call(&strict, None, &[]).unwrap();
    assert_eq!(exported(&mut vm, result), JsData::Bool(true));
    vm.execute(Box::new(|scope: &dyn JsScope| {
        let js = scope.source().unwrap();
        let constructor = js
            .evaluate(&source("(function Box(n) { this.n = n; })"))
            .map_err(|error| scope.detach(error))?;
        let new_target = js
            .evaluate(&source("(function Target() {})"))
            .map_err(|error| scope.detach(error))?;
        let prototype = js
            .evaluate(&source("({tag: 'custom-prototype'})"))
            .map_err(|error| scope.detach(error))?;
        scope
            .set(
                new_target,
                PropertyKey::String(&JsString::from_utf8("prototype")),
                prototype,
            )
            .map_err(|error| scope.detach(error))?;
        let number = scope
            .import_data(JsData::Number(42.0))
            .map_err(|error| scope.detach(error))?;
        let object = scope
            .construct(constructor, &[number], Some(new_target))
            .map_err(|error| scope.detach(error))?;
        for (key, expected) in [
            ("n", JsData::Number(42.0)),
            ("tag", text("custom-prototype")),
        ] {
            let value = scope
                .get(object, PropertyKey::String(&JsString::from_utf8(key)))
                .map_err(|error| scope.detach(error))?;
            assert_eq!(
                scope
                    .export_data(value, ExportLimits::default())
                    .map_err(|error| scope.detach(error))?,
                expected
            );
        }
        Ok(())
    }))
    .unwrap();
    vm.shutdown().unwrap();
}

#[test]
fn compile_and_artifact_execution_preserve_real_javascript_state_and_fifo() {
    let mut vm = vm();
    drop(vm.evaluate(source("globalThis.count = 0")).unwrap());
    vm.submit(source(
        "count += 1; Promise.resolve().then(() => { count += 10; });",
    ))
    .unwrap();
    let artifact = vm
        .compile(
            source("count += 100; count"),
            QuickJsNgEngine::script_compile_options(),
        )
        .unwrap();
    assert_eq!(
        evaluated(&mut vm, "count"),
        JsData::Number(11.0),
        "compile executes no application code and follows the older checkpoint"
    );
    let result = vm.execute_artifact(artifact).unwrap();
    assert_eq!(exported(&mut vm, result), JsData::Number(111.0));
}

#[test]
fn submitted_javascript_and_native_microtasks_obey_the_same_fifo_capture_boundary() {
    let mut cfg = config();
    cfg.scheduler.microtask_hint = NonZeroUsize::new(1).unwrap();
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), cfg).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let _record = bind(&mut vm, "record", Record(events.clone()));
    let _post = bind(
        &mut vm,
        "post",
        Schedule {
            timer: false,
            cancel: false,
        },
    );
    vm.submit(source("record('older'); Promise.resolve().then(() => record('older-microtask')).then(() => record('older-chain')); ")).unwrap();
    let result = vm.evaluate(source("record('target'); post(() => record('newer')); Promise.resolve().then(() => record('target-microtask')).then(() => record('target-chain')); 42")).unwrap();
    assert_eq!(
        *events.borrow(),
        [
            text("older"),
            text("older-microtask"),
            text("older-chain"),
            text("target"),
            text("target-microtask"),
            text("target-chain")
        ]
    );
    vm.tick(false).unwrap();
    assert_eq!(events.borrow().last(), Some(&text("newer")));
    assert_eq!(exported(&mut vm, result), JsData::Number(42.0));
}

#[test]
fn thrown_primitives_return_once_after_checkpoint_and_continue_preserves_the_vm() {
    let reports = Rc::new(RefCell::new(Vec::new()));
    let sink = reports.clone();
    let mut cfg = config();
    cfg.script_error_policy = ScriptErrorPolicy::Continue;
    cfg.report_sink = Some(Rc::new(move |report| {
        sink.borrow_mut().push(report.clone())
    }));
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), cfg).unwrap();
    let checkpoint_events = Rc::new(RefCell::new(Vec::new()));
    let _record = bind(&mut vm, "record", Record(checkpoint_events.clone()));
    for (expression, expected) in [
        ("undefined", JsData::Undefined),
        ("null", JsData::Null),
        ("true", JsData::Bool(true)),
        ("42", JsData::Number(42.0)),
        ("'native throw'", text("native throw")),
    ] {
        let error = vm.evaluate(source(&format!("globalThis.checked = false; Promise.resolve().then(() => {{ checked = true; record('checked'); }}); throw {expression};"))).unwrap_err();
        let VmError::Execution(report) = error else {
            panic!("expected native script exception: {error:?}")
        };
        assert_eq!(report.class, FailureClass::Script);
        assert_eq!(report.code, FailureCode::ScriptException);
        assert_eq!(report.thrown, Some(expected));
        assert_eq!(
            *checkpoint_events.borrow(),
            [text("checked")],
            "the throwing turn's checkpoint must finish before its capture returns"
        );
        checkpoint_events.borrow_mut().clear();
        assert_eq!(vm.state(), VmState::Ready);
        assert_eq!(evaluated(&mut vm, "checked"), JsData::Bool(true));
        assert!(
            reports.borrow().is_empty(),
            "captured failures are not also sent to the queued sink"
        );
    }
    vm.submit(source("throw 'queued native throw'")).unwrap();
    vm.tick(false).unwrap();
    assert_eq!(reports.borrow().len(), 1);
    assert_eq!(
        reports.borrow()[0].thrown,
        Some(text("queued native throw"))
    );
    assert_eq!(evaluated(&mut vm, "6 * 7"), JsData::Number(42.0));
}

#[test]
fn stop_policy_tears_down_after_a_native_throw_and_keeps_its_detached_snapshot() {
    let mut vm = vm();
    let root = vm.evaluate(source("({kept: true})")).unwrap();
    let error = vm.evaluate(source("throw 17")).unwrap_err();
    let VmError::Execution(report) = error else {
        panic!("expected script failure")
    };
    assert_eq!(report.class, FailureClass::Script);
    assert_eq!(report.thrown, Some(JsData::Number(17.0)));
    assert_eq!(vm.state(), VmState::Faulted);
    assert_eq!(vm.evaluate(source("42")).unwrap_err(), VmError::Faulted);
    vm.shutdown().unwrap();
    drop(root);
    assert_eq!(report.thrown, Some(JsData::Number(17.0)));
}

struct Recurse;
impl HostFunction for Recurse {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let original = call.arguments()[1];
        let JsData::Number(n) = call.export_data(original, ExportLimits::default())? else {
            panic!("numeric recursion argument")
        };
        let next = call.import_data(JsData::Number(n - 1.0))?;
        let value = call.call(call.arguments()[0], None, &[next])?;
        let JsData::Number(result) = call.export_data(value, ExportLimits::default())? else {
            panic!("numeric recursion result")
        };
        assert_eq!(
            call.export_data(original, ExportLimits::default())?,
            JsData::Number(n),
            "nested callbacks must not overwrite their caller's argument frame"
        );
        call.import_data(JsData::Number(n + result))
    }
}
#[test]
fn javascript_can_reenter_the_same_host_registration_with_stable_argument_frames() {
    let mut vm = vm();
    let _recurse = bind(&mut vm, "hostRecurse", Recurse);
    assert_eq!(
        evaluated(
            &mut vm,
            "(function recur(n) { return n === 0 ? 0 : hostRecurse(recur, n); })(8)"
        ),
        JsData::Number(36.0)
    );
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn retained_javascript_closures_run_as_one_shot_timers_and_canceled_timers_stay_silent() {
    let mut vm = vm();
    let events = Rc::new(RefCell::new(Vec::new()));
    let _record = bind(&mut vm, "record", Record(events.clone()));
    let _timer = bind(
        &mut vm,
        "timer",
        Schedule {
            timer: true,
            cancel: false,
        },
    );
    let _canceled = bind(
        &mut vm,
        "canceledTimer",
        Schedule {
            timer: true,
            cancel: true,
        },
    );
    drop(vm.evaluate(source("(() => { const closedOver = 42; timer(() => { record(closedOver); Promise.resolve().then(() => record('timer-checkpoint')); }); canceledTimer(() => record('must-not-run')); })()")).unwrap());
    assert!(events.borrow().is_empty());
    vm.tick(false).unwrap();
    assert_eq!(
        *events.borrow(),
        [JsData::Number(42.0), text("timer-checkpoint")]
    );
    vm.tick(false).unwrap();
    assert_eq!(events.borrow().len(), 2, "one-shot callback is not rearmed");
}

#[test]
fn reserved_worker_callback_and_promise_settle_only_when_the_owner_pumps_again() {
    let mut vm = vm();
    let events = Rc::new(RefCell::new(Vec::new()));
    let tickets = Rc::new(RefCell::new(Vec::new()));
    let _record = bind(&mut vm, "record", Record(events.clone()));
    let _callback = bind(
        &mut vm,
        "startCallback",
        StartOperation {
            tickets: tickets.clone(),
            promise: false,
        },
    );
    let _promise = bind(
        &mut vm,
        "startPromise",
        StartOperation {
            tickets: tickets.clone(),
            promise: true,
        },
    );
    let promise = vm.evaluate(source("startCallback((error, value) => { if (error !== null) throw 'unexpected callback error'; record(value); Promise.resolve().then(() => record('callback-checkpoint')); }); globalThis.pending = true; startPromise().then(value => { pending = false; record(value); });")).unwrap();
    assert!(events.borrow().is_empty());
    assert_eq!(evaluated(&mut vm, "pending"), JsData::Bool(true));
    let owned = std::mem::take(&mut *tickets.borrow_mut());
    std::thread::spawn(move || {
        for (index, ticket) in owned.into_iter().enumerate() {
            assert!(matches!(
                ticket.complete(CompletionPayload::Success(JsData::Number(
                    41.0 + index as f64
                ))),
                PublishOutcome::Published
            ));
        }
    })
    .join()
    .unwrap();
    assert!(
        events.borrow().is_empty(),
        "producer publication must not enter JavaScript"
    );
    vm.tick(false).unwrap();
    assert_eq!(
        *events.borrow(),
        [
            JsData::Number(41.0),
            text("callback-checkpoint"),
            JsData::Number(42.0)
        ]
    );
    assert_eq!(evaluated(&mut vm, "pending"), JsData::Bool(false));
    drop(promise);
}

#[test]
fn failed_worker_result_is_native_promise_rejection_data_not_a_vm_fault() {
    let mut vm = vm();
    let events = Rc::new(RefCell::new(Vec::new()));
    let tickets = Rc::new(RefCell::new(Vec::new()));
    let _record = bind(&mut vm, "record", Record(events.clone()));
    let _promise = bind(
        &mut vm,
        "startPromise",
        StartOperation {
            tickets: tickets.clone(),
            promise: true,
        },
    );
    drop(vm.evaluate(source("startPromise().then(() => record('unexpected success'), error => record([error.code, error.message, error.details]));")).unwrap());
    let ticket = tickets.borrow_mut().pop().unwrap();
    assert!(matches!(
        std::thread::spawn(
            move || ticket.complete(CompletionPayload::Failure(OperationFailure {
                code: "E_WORK".into(),
                message: JsString::from_utf8("worker failed"),
                details: Some(JsData::Number(7.0))
            }))
        )
        .join()
        .unwrap(),
        PublishOutcome::Published
    ));
    vm.tick(false).unwrap();
    assert_eq!(
        *events.borrow(),
        [JsData::Array(vec![
            text("E_WORK"),
            text("worker failed"),
            JsData::Number(7.0)
        ])]
    );
    assert_eq!(vm.state(), VmState::Ready);
}

struct MessageHandler(Rc<RefCell<Option<RootedValue>>>);
impl HostMessageHandler for MessageHandler {
    fn deliver<'s>(
        &self,
        scope: &'s dyn JsScope,
        _: &dyn HostServices,
        payload: JsData,
    ) -> JsResult<'s, ()> {
        let callback = self.0.borrow();
        let function = scope.local(
            callback
                .as_ref()
                .expect("route installed before publication"),
        )?;
        let value = scope.import_data(payload)?;
        scope.call(function, None, &[value])?;
        Ok(())
    }
}
#[test]
fn ordinary_messages_call_javascript_on_the_owner_and_finish_its_checkpoint() {
    let callback = Rc::new(RefCell::new(None));
    let mut cfg = config();
    cfg.message_handler = Some(Rc::new(MessageHandler(callback.clone())));
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), cfg).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let _record = bind(&mut vm, "record", Record(events.clone()));
    *callback.borrow_mut() = Some(vm.evaluate(source("(value => { record(value); Promise.resolve().then(() => record('message-checkpoint')); })")).unwrap());
    let handle = vm.handle();
    std::thread::spawn(move || handle.try_send(JsData::Number(42.0)).unwrap())
        .join()
        .unwrap();
    assert!(events.borrow().is_empty());
    vm.tick(false).unwrap();
    assert_eq!(
        *events.borrow(),
        [JsData::Number(42.0), text("message-checkpoint")]
    );
    vm.shutdown().unwrap();
    let rejected = vm.handle().try_send(text("late message")).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Closed);
    assert_eq!(rejected.value, text("late message"));
    drop(callback.borrow_mut().take());
}

#[test]
fn shutdown_invalidates_external_roots_and_rejects_late_worker_publication() {
    let mut first = vm();
    let mut replacement = vm();
    let tickets = Rc::new(RefCell::new(Vec::new()));
    let _start = bind(
        &mut first,
        "startPromise",
        StartOperation {
            tickets: tickets.clone(),
            promise: true,
        },
    );
    let external = first.evaluate(source("(() => 99)")).unwrap();
    let promise = first.evaluate(source("startPromise()")).unwrap();
    assert_eq!(
        replacement.call(&external, None, &[]).unwrap_err(),
        VmError::InvalidHandle
    );
    let handle = first.handle();
    first.shutdown().unwrap();
    first.shutdown().unwrap();
    assert_eq!(first.state(), VmState::Stopped);
    assert_eq!(
        first.call(&external, None, &[]).unwrap_err(),
        VmError::Stopped
    );
    assert_eq!(
        replacement.call(&external, None, &[]).unwrap_err(),
        VmError::InvalidHandle
    );
    let ticket = tickets.borrow_mut().pop().unwrap();
    let outcome = std::thread::spawn(move || {
        ticket.complete(CompletionPayload::Success(JsData::Number(42.0)))
    })
    .join()
    .unwrap();
    assert!(matches!(
        outcome,
        PublishOutcome::Rejected {
            reason: PublishRejection::Closed | PublishRejection::Stale,
            payload: CompletionPayload::Success(JsData::Number(42.0))
        }
    ));
    assert_eq!(
        handle.try_send(JsData::Null).unwrap_err().reason,
        RejectionReason::Closed
    );
    assert_eq!(evaluated(&mut replacement, "6 * 7"), JsData::Number(42.0));
    drop((external, promise));
}

struct Stop;
impl HostFunction for Stop {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        call.host().request_stop()?;
        call.import_data(JsData::Number(42.0))
    }
}
#[test]
fn stop_from_a_native_promise_reaction_discards_the_pending_successful_capture() {
    let mut vm = vm();
    let _stop = bind(&mut vm, "stop", Stop);
    assert_eq!(
        vm.evaluate(source("Promise.resolve().then(() => stop()); 42"))
            .unwrap_err(),
        VmError::Stopped
    );
    assert_eq!(vm.state(), VmState::Stopped);
}
