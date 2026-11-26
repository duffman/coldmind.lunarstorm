//! Promise and reserved-completion acceptance against actual public QuickJS-NG VM entry.
#![cfg(feature = "quickjs-ng")]

use coldmind_runtime::{
    debug::DebugRequirements, engine::*, engines::quickjs_ng::QuickJsNgEngine,
    scheduler::ScriptErrorPolicy, vm::*,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Barrier},
    time::Duration,
};

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
fn vm(config: VmConfig) -> NativeVm {
    VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap()
}
fn source(program: &str) -> ScriptSource {
    ScriptSource::new(
        JsString::from_utf8(program),
        "file:///native-async.js".into(),
    )
}
fn text(value: &str) -> JsData {
    JsData::String(JsString::from_utf8(value))
}
fn eval<'s>(scope: &'s dyn JsScope, program: &str) -> JsResult<'s, ScopedValue<'s>> {
    scope.source().unwrap().evaluate(&source(program))
}
fn copied(scope: &dyn JsScope, program: &str) -> Result<JsData, FailureReport> {
    let value = eval(scope, program).map_err(|error| scope.detach(error))?;
    scope
        .export_data(value, ExportLimits::default())
        .map_err(|error| scope.detach(error))
}
fn bind(vm: &mut NativeVm, name: &str, function: impl HostFunction + 'static) {
    drop(
        vm.bind_function(JsString::from_utf8(name), 1, Rc::new(function))
            .unwrap(),
    );
}
fn script(vm: &mut NativeVm, program: &str) {
    drop(vm.evaluate(source(program)).unwrap());
}
fn pump(vm: &mut NativeVm) {
    for _ in 0..3 {
        vm.tick(false).unwrap();
    }
}

struct Record(Events);
impl HostFunction for Record {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let data = call.export_data(call.arguments()[0], ExportLimits::default())?;
        self.0.borrow_mut().push(data);
        Ok(call.receiver())
    }
}
struct CancelCount(Rc<Cell<usize>>);
impl CancelNative for CancelCount {
    fn cancel(&self) -> Result<(), OperationFailure> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
}
struct Begin {
    tickets: Tickets,
    allowance: usize,
    cancel: Option<Rc<dyn CancelNative>>,
    cancel_immediately: bool,
}
impl HostFunction for Begin {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let reservation = call.host().reserve_operation(self.allowance)?;
        let deferred = call.deferred()?;
        let ticket =
            call.host()
                .bind_deferred(reservation, deferred.resolver, self.cancel.clone())?;
        if self.cancel_immediately {
            assert_eq!(
                call.host().cancel_operation(ticket.id()),
                CancelOutcome::Canceled
            );
        }
        self.tickets.borrow_mut().push(ticket);
        Ok(deferred.promise)
    }
}

#[test]
fn intrinsic_deferred_assimilates_thenables_once_without_an_implicit_checkpoint() {
    let mut vm = vm(config());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        eval(scope, "globalThis.trace=[]; globalThis.Promise=function(){throw 'replaced Promise constructor';}").map_err(|error| scope.detach(error))?;
        let deferred = scope.deferred().map_err(|error| scope.detach(error))?;
        let observe = eval(scope, "promise => promise.then(value => trace.push(value), error => trace.push('unexpected-rejection'))").map_err(|error| scope.detach(error))?;
        scope.call(observe, None, &[deferred.promise]).map_err(|error| scope.detach(error))?;
        let thenable = eval(scope, "({get then(){trace.push('get-then');return function(resolve,reject){trace.push('then');resolve(42);reject('late rejection');resolve(99);};}})").map_err(|error| scope.detach(error))?;
        scope.settle(deferred.resolver, Settlement::Resolve(thenable)).map_err(|error| scope.detach(error))?;
        assert_eq!(copied(scope, "trace")?, JsData::Array(vec![text("get-then")]), "then getter is synchronous, thenable body and reaction are native jobs");
        Ok(())
    })).unwrap();
    vm.execute(Box::new(|scope: &dyn JsScope| {
        assert_eq!(
            copied(scope, "trace")?,
            JsData::Array(vec![text("get-then"), text("then"), JsData::Number(42.0)])
        );
        Ok(())
    }))
    .unwrap();
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn throwing_then_getter_rejects_with_original_object_at_the_explicit_checkpoint() {
    let mut vm = vm(config());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        eval(scope, "globalThis.trace=[];globalThis.sentinel={tag:'original'}").map_err(|error| scope.detach(error))?;
        let deferred = scope.deferred().map_err(|error| scope.detach(error))?;
        let observe = eval(scope, "promise => promise.then(() => trace.push('unexpected-success'), error => trace.push(error === sentinel))").map_err(|error| scope.detach(error))?;
        scope.call(observe, None, &[deferred.promise]).map_err(|error| scope.detach(error))?;
        let thenable = eval(scope, "({get then(){trace.push('get-then');throw sentinel;}})").map_err(|error| scope.detach(error))?;
        scope.settle(deferred.resolver, Settlement::Resolve(thenable)).map_err(|error| scope.detach(error))?;
        assert_eq!(copied(scope, "trace")?, JsData::Array(vec![text("get-then")]));
        Ok(())
    })).unwrap();
    vm.execute(Box::new(|scope: &dyn JsScope| {
        assert_eq!(
            copied(scope, "trace")?,
            JsData::Array(vec![text("get-then"), JsData::Bool(true)])
        );
        Ok(())
    }))
    .unwrap();
    assert_eq!(
        vm.state(),
        VmState::Ready,
        "native Promise rejection is ordinary Promise state"
    );
}

#[test]
fn canceled_abandoned_and_oversized_results_each_reject_a_native_promise_once() {
    for mode in ["Canceled", "Abandoned", "PayloadTooLarge"] {
        let mut cfg = config();
        cfg.scheduler.max_operations = 1;
        let mut vm = vm(cfg);
        let events = Rc::new(RefCell::new(Vec::new()));
        let tickets = Rc::new(RefCell::new(Vec::new()));
        let cancels = Rc::new(Cell::new(0));
        bind(&mut vm, "record", Record(events.clone()));
        bind(
            &mut vm,
            "begin",
            Begin {
                tickets: tickets.clone(),
                allowance: 512,
                cancel: Some(Rc::new(CancelCount(cancels.clone()))),
                cancel_immediately: mode == "Canceled",
            },
        );
        script(
            &mut vm,
            "begin().then(value => record('unexpected-success'), error => record(error.code))",
        );
        assert!(events.borrow().is_empty());
        let ticket = tickets.borrow_mut().pop().unwrap();
        match mode {
            "Canceled" => {
                let bytes = vec![3; 16];
                let pointer = bytes.as_ptr();
                let outcome = std::thread::spawn(move || {
                    ticket.complete(CompletionPayload::Success(JsData::Bytes(bytes)))
                })
                .join()
                .unwrap();
                assert!(
                    matches!(outcome, PublishOutcome::Rejected { reason: PublishRejection::Canceled, payload: CompletionPayload::Success(JsData::Bytes(ref bytes)) } if bytes.as_ptr() == pointer && bytes == &[3;16])
                );
            }
            "Abandoned" => std::thread::spawn(move || drop(ticket)).join().unwrap(),
            "PayloadTooLarge" => {
                let bytes = vec![5; 4096];
                let pointer = bytes.as_ptr();
                let outcome = std::thread::spawn(move || {
                    ticket.complete(CompletionPayload::Success(JsData::Bytes(bytes)))
                })
                .join()
                .unwrap();
                assert!(
                    matches!(outcome, PublishOutcome::Rejected { reason: PublishRejection::PayloadTooLarge, payload: CompletionPayload::Success(JsData::Bytes(ref bytes)) } if bytes.as_ptr() == pointer && bytes.len() == 4096)
                );
            }
            _ => unreachable!(),
        }
        assert!(
            events.borrow().is_empty(),
            "producer terminal publication cannot enter JavaScript"
        );
        pump(&mut vm);
        assert_eq!(*events.borrow(), [text(mode)]);
        assert_eq!(cancels.get(), usize::from(mode != "PayloadTooLarge"));
        // The sole reservation slot must be reusable after terminal delivery.
        script(
            &mut vm,
            "begin().then(value => record(value), error => record(error.code))",
        );
        let ticket = tickets.borrow_mut().pop().unwrap();
        if mode == "Canceled" {
            drop(ticket);
        } else {
            assert!(matches!(
                ticket.complete(CompletionPayload::Success(JsData::Number(42.0))),
                PublishOutcome::Published
            ));
        }
        pump(&mut vm);
        assert_eq!(events.borrow().len(), 2);
        assert_eq!(
            events.borrow()[1],
            if mode == "Canceled" {
                text("Canceled")
            } else {
                JsData::Number(42.0)
            }
        );
        vm.shutdown().unwrap();
        assert_eq!(
            cancels.get(),
            if mode == "Canceled" {
                2
            } else {
                usize::from(mode == "Abandoned")
            }
        );
    }
}

struct Rollback {
    rejected: Rc<Cell<usize>>,
    cancel: Rc<dyn CancelNative>,
}
impl HostFunction for Rollback {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        drop(call.host().reserve_operation(1024)?);
        let reservation = call.host().reserve_operation(1024)?;
        let noncallable = call.import_data(JsData::Null)?;
        let root = call.retain(noncallable)?;
        let error = call
            .host()
            .bind_callback(reservation, root, Some(self.cancel.clone()))
            .unwrap_err();
        assert_eq!(error.code, FailureCode::InvalidHandle);
        self.rejected.set(self.rejected.get() + 1);
        Ok(call.receiver())
    }
}

#[test]
fn dropped_reservations_and_invalid_native_callback_bindings_roll_back_without_cancel() {
    let mut cfg = config();
    cfg.scheduler.max_operations = 1;
    cfg.context.max_persistent_roots = 8;
    let mut vm = vm(cfg);
    let tickets = Rc::new(RefCell::new(Vec::new()));
    let events = Rc::new(RefCell::new(Vec::new()));
    let rejected = Rc::new(Cell::new(0));
    let cancels = Rc::new(Cell::new(0));
    bind(
        &mut vm,
        "rollback",
        Rollback {
            rejected: rejected.clone(),
            cancel: Rc::new(CancelCount(cancels.clone())),
        },
    );
    bind(
        &mut vm,
        "begin",
        Begin {
            tickets: tickets.clone(),
            allowance: 1024,
            cancel: None,
            cancel_immediately: false,
        },
    );
    bind(&mut vm, "record", Record(events.clone()));
    // Separate owner turns also prove rejected roots flush, rather than merely
    // reusing the transport permit while exhausting the native root quota.
    for _ in 0..24 {
        script(&mut vm, "rollback()");
    }
    assert_eq!(rejected.get(), 24);
    assert_eq!(cancels.get(), 0);
    script(&mut vm, "begin().then(record)");
    assert!(matches!(
        tickets
            .borrow_mut()
            .pop()
            .unwrap()
            .complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Published
    ));
    pump(&mut vm);
    assert_eq!(*events.borrow(), [JsData::Number(42.0)]);
    assert_eq!(cancels.get(), 0);
}

type RaceOutcomes = Rc<RefCell<Vec<(CancelOutcome, PublishOutcome)>>>;
struct Race {
    outcomes: RaceOutcomes,
    cancel: Rc<dyn CancelNative>,
}
impl HostFunction for Race {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let reservation = call.host().reserve_operation(1024)?;
        let deferred = call.deferred()?;
        let ticket =
            call.host()
                .bind_deferred(reservation, deferred.resolver, Some(self.cancel.clone()))?;
        let id = ticket.id();
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            ticket.complete(CompletionPayload::Success(JsData::Number(42.0)))
        });
        barrier.wait();
        let canceled = call.host().cancel_operation(id);
        let published = worker.join().unwrap();
        self.outcomes.borrow_mut().push((canceled, published));
        Ok(deferred.promise)
    }
}

#[test]
fn racing_owner_cancellation_and_worker_completion_have_one_native_terminal_winner() {
    let mut vm = vm(config());
    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(Cell::new(0));
    let events = Rc::new(RefCell::new(Vec::new()));
    bind(
        &mut vm,
        "race",
        Race {
            outcomes: outcomes.clone(),
            cancel: Rc::new(CancelCount(cancels.clone())),
        },
    );
    bind(&mut vm, "record", Record(events.clone()));
    script(
        &mut vm,
        "for(let index=0;index<16;index++) race().then(value => record([index,value]), error => record([index,error.code]));",
    );
    assert!(events.borrow().is_empty());
    let mut canceled = 0;
    let mut expected = Vec::new();
    for (cancel, publish) in outcomes.borrow().iter() {
        match (cancel, publish) {
            (
                CancelOutcome::Canceled,
                PublishOutcome::Rejected {
                    reason: PublishRejection::Canceled,
                    ..
                },
            ) => {
                canceled += 1;
                expected.push(text("Canceled"));
            }
            (CancelOutcome::TooLate, PublishOutcome::Published) => {
                expected.push(JsData::Number(42.0));
            }
            other => panic!("inconsistent native terminal winner: {other:?}"),
        }
    }
    pump(&mut vm);
    assert_eq!(events.borrow().len(), 16);
    let mut seen = [false; 16];
    for event in events.borrow().iter() {
        let JsData::Array(fields) = event else {
            panic!("native Promise reaction payload")
        };
        let JsData::Number(index) = fields[0] else {
            panic!("reaction index")
        };
        let index = index as usize;
        assert!(!seen[index], "one native Promise cannot react twice");
        seen[index] = true;
        assert_eq!(
            fields[1], expected[index],
            "native settlement must match the winning transport terminal"
        );
    }
    assert!(seen.into_iter().all(|seen| seen));
    assert_eq!(cancels.get(), canceled);
    vm.shutdown().unwrap();
    assert_eq!(
        cancels.get(),
        canceled,
        "successful work is never canceled during cleanup"
    );
}

struct FillTimer(Rc<Cell<usize>>);
impl HostFunction for FillTimer {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let job = || -> Result<CallbackJob, FailureReport> {
            Ok(CallbackJob {
                function: call.retain(call.arguments()[0])?,
                receiver: None,
                arguments: Vec::new(),
            })
        };
        call.host()
            .timer(Duration::from_secs(3600), None, job()?)
            .unwrap();
        let rejected = call
            .host()
            .timer(Duration::from_secs(3600), None, job()?)
            .unwrap_err();
        assert_eq!(rejected.reason, RejectionReason::Full);
        self.0.set(self.0.get() + 1);
        Ok(call.receiver())
    }
}
struct Messages(Events);
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
fn reserved_terminal_delivery_survives_full_job_timer_message_and_operation_quotas() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let tickets = Rc::new(RefCell::new(Vec::new()));
    let timer_full = Rc::new(Cell::new(0));
    let mut cfg = config();
    cfg.scheduler.max_jobs = 2;
    cfg.scheduler.max_messages = 1;
    cfg.scheduler.max_operations = 1;
    cfg.message_handler = Some(Rc::new(Messages(events.clone())));
    let mut vm = vm(cfg);
    bind(
        &mut vm,
        "begin",
        Begin {
            tickets: tickets.clone(),
            allowance: 1024,
            cancel: None,
            cancel_immediately: false,
        },
    );
    bind(&mut vm, "record", Record(events.clone()));
    bind(&mut vm, "fillTimer", FillTimer(timer_full.clone()));
    script(
        &mut vm,
        "begin().then(value => record(['first',value]));fillTimer(() => record('unexpected timer'))",
    );
    assert_eq!(timer_full.get(), 1);
    vm.submit(source("record('ordinary')")).unwrap();
    assert_eq!(
        vm.submit(source("record('must-not-run')"))
            .unwrap_err()
            .reason,
        RejectionReason::Full
    );
    let handle = vm.handle();
    handle.try_send(text("message")).unwrap();
    assert_eq!(
        handle.try_send(text("must-not-run")).unwrap_err().reason,
        RejectionReason::Full
    );
    let ticket = tickets.borrow_mut().pop().unwrap();
    assert!(matches!(
        std::thread::spawn(
            move || ticket.complete(CompletionPayload::Success(JsData::Number(42.0)))
        )
        .join()
        .unwrap(),
        PublishOutcome::Published
    ));
    pump(&mut vm);
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|event| **event == JsData::Array(vec![text("first"), JsData::Number(42.0)]))
            .count(),
        1
    );
    assert!(events.borrow().contains(&text("ordinary")));
    assert!(events.borrow().contains(&text("message")));
    assert_eq!(events.borrow().len(), 3);
    // A new reservation proves the delivered completion released its only slot;
    // the sleeping native callback timer continues to occupy one local job slot.
    script(&mut vm, "begin().then(value => record(['second',value]))");
    assert!(matches!(
        tickets
            .borrow_mut()
            .pop()
            .unwrap()
            .complete(CompletionPayload::Success(JsData::Number(43.0))),
        PublishOutcome::Published
    ));
    pump(&mut vm);
    assert_eq!(
        events.borrow().last(),
        Some(&JsData::Array(vec![text("second"), JsData::Number(43.0)]))
    );
    assert_eq!(events.borrow().len(), 4);
    vm.shutdown().unwrap();
}

#[test]
fn completion_materialization_failure_is_fatal_and_closes_other_native_resolvers() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let tickets = Rc::new(RefCell::new(Vec::new()));
    let cancels = Rc::new(Cell::new(0));
    let reports = Rc::new(RefCell::new(Vec::new()));
    let observed = reports.clone();
    let mut cfg = config();
    cfg.context.export.max_bytes = 1024;
    cfg.script_error_policy = ScriptErrorPolicy::Continue;
    cfg.report_sink = Some(Rc::new(move |report| {
        observed.borrow_mut().push(report.clone())
    }));
    let mut vm = vm(cfg);
    bind(&mut vm, "record", Record(events.clone()));
    bind(
        &mut vm,
        "begin",
        Begin {
            tickets: tickets.clone(),
            allowance: 16_384,
            cancel: Some(Rc::new(CancelCount(cancels.clone()))),
            cancel_immediately: false,
        },
    );
    script(
        &mut vm,
        "begin().then(() => record('first resolved'), () => record('first rejected'));begin().then(() => record('second resolved'), () => record('second rejected'))",
    );
    let mut owned = std::mem::take(&mut *tickets.borrow_mut());
    let second = owned.pop().unwrap();
    let first = owned.pop().unwrap();
    assert!(matches!(
        first.complete(CompletionPayload::Success(JsData::Bytes(vec![9; 4096]))),
        PublishOutcome::Published
    ));
    let error = vm.tick(false).unwrap_err();
    let VmError::Execution(report) = error else {
        panic!("expected fatal completion materialization failure: {error:?}")
    };
    assert_eq!(
        (report.class, report.code),
        (FailureClass::Engine, FailureCode::EngineFailure)
    );
    assert_eq!(vm.state(), VmState::Faulted);
    assert!(
        events.borrow().is_empty(),
        "no terminal callback may run after pre-settlement materialization fails"
    );
    assert_eq!(
        cancels.get(),
        1,
        "the other still-pending native operation is canceled during fatal teardown"
    );
    assert!(
        reports
            .borrow()
            .iter()
            .any(|report| report.class == FailureClass::Engine)
    );
    assert!(matches!(
        second.complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Rejected {
            reason: PublishRejection::Closed | PublishRejection::Stale,
            ..
        }
    ));
    vm.shutdown().unwrap();
    assert_eq!(cancels.get(), 1);
}
