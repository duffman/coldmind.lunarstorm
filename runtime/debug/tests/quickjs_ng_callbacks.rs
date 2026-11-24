//! Native callback recursion, registration ownership, and foreign panic boundaries.
#![cfg(feature = "quickjs-ng")]

use coldmind_runtime::{
    debug::DebugRequirements, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*,
};
use std::{
    cell::{Cell, RefCell},
    panic::panic_any,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn source(program: &str) -> ScriptSource {
    ScriptSource::new(
        JsString::from_utf8(program),
        "file:///native-callbacks.js".into(),
    )
}

struct Payload(Arc<AtomicUsize>);
impl Drop for Payload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct PanicPayload(Arc<AtomicUsize>);
impl HostFunction for PanicPayload {
    fn invoke<'s>(&self, _: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        panic_any(Payload(self.0.clone()));
    }
}
#[test]
fn javascript_try_catch_cannot_hide_native_host_fault_or_leak_payload() {
    let mut vm = VirtualMachine::new(
        QuickJsNgEngine::new().unwrap(),
        VmConfig {
            requirements: EngineRequirements {
                debug: DebugRequirements::disabled(),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let _panic = vm
        .bind_function(
            JsString::from_utf8("hostPanic"),
            0,
            Rc::new(PanicPayload(drops.clone())),
        )
        .unwrap();
    // This QuickJS-NG pin marks InternalError uncatchable. A try/catch containing
    // a successful fallback must still produce the adapter's typed fatal fault.
    let error = vm
        .evaluate(source("try { hostPanic(); } catch (error) { 42; } 42"))
        .unwrap_err();
    let VmError::Execution(report) = error else {
        panic!("expected fatal typed host fault: {error:?}")
    };
    assert_eq!(report.class, FailureClass::Host);
    assert_eq!(report.code, FailureCode::HostPanic);
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "normal panic payload must be destroyed exactly once"
    );
    assert_eq!(vm.state(), VmState::Faulted);
    vm.shutdown().unwrap();
}

// These direct-engine tests do not dispatch scheduler work. Rejection preserves
// owned inputs and keeps every native operation on the real public engine API.
struct NoServices;
impl HostServices for NoServices {
    #[allow(clippy::result_large_err)]
    fn post(&self, job: CallbackJob) -> Result<TaskId, Rejected<CallbackJob>> {
        Err(Rejected {
            value: job,
            reason: RejectionReason::Unsupported,
        })
    }
    #[allow(clippy::result_large_err)]
    fn timer(
        &self,
        _: Duration,
        _: Option<Duration>,
        job: CallbackJob,
    ) -> Result<TaskId, Rejected<CallbackJob>> {
        self.post(job)
    }
    fn cancel(&self, _: TaskId) -> CancelOutcome {
        CancelOutcome::NotFound
    }
    fn request_stop(&self) -> Result<(), FailureReport> {
        Err(unsupported())
    }
    fn reserve_operation(&self, _: usize) -> Result<OperationReservation, FailureReport> {
        Err(unsupported())
    }
    fn bind_deferred(
        &self,
        _: OperationReservation,
        _: RootedDeferred,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        Err(unsupported())
    }
    fn bind_callback(
        &self,
        _: OperationReservation,
        _: RootedValue,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        Err(unsupported())
    }
    fn cancel_operation(&self, _: OperationId) -> CancelOutcome {
        CancelOutcome::NotFound
    }
}
fn unsupported() -> FailureReport {
    FailureReport::fault(
        FailureCode::Unsupported,
        "no scheduler services in native callback test",
    )
}
fn run(
    engine: &QuickJsNgEngine,
    context: ContextId,
    mut body: impl FnMut(&dyn JsScope) -> Result<(), FailureReport>,
) -> Result<(), FailureReport> {
    engine.with_scope(context, &NoServices, &mut body)
}
fn execute(engine: &QuickJsNgEngine, context: ContextId, program: &str) {
    run(engine, context, |scope| {
        scope
            .source()
            .unwrap()
            .evaluate(&source(program))
            .map_err(|error| scope.detach(error))?;
        Ok(())
    })
    .unwrap();
}
fn install(
    engine: &QuickJsNgEngine,
    context: ContextId,
    name: &str,
    function: Rc<dyn HostFunction>,
) {
    run(engine, context, |scope| {
        let function = scope
            .host_function(&JsString::from_utf8(name), 2, function.clone())
            .map_err(|error| scope.detach(error))?;
        let global = scope.global().map_err(|error| scope.detach(error))?;
        scope
            .set(
                global,
                PropertyKey::String(&JsString::from_utf8(name)),
                function,
            )
            .map_err(|error| scope.detach(error))?;
        Ok(())
    })
    .unwrap();
}

struct RecurseWithGc {
    engine: Weak<QuickJsNgEngine>,
    visited: Rc<RefCell<Vec<f64>>>,
    drops: Rc<Cell<usize>>,
}
impl Drop for RecurseWithGc {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}
impl HostFunction for RecurseWithGc {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let original = call.arguments()[1];
        let key = JsString::from_utf8("n");
        let n = call.get(original, PropertyKey::String(&key))?;
        let JsData::Number(n) = call.export_data(n, ExportLimits::default())? else {
            panic!("numeric native recursion argument")
        };
        self.visited.borrow_mut().push(n);
        self.engine
            .upgrade()
            .unwrap()
            .collect_garbage(call.context())?;
        let next = call.import_data(JsData::Number(n - 1.0))?;
        let inner = call.call(call.arguments()[0], None, &[next])?;
        let JsData::Number(inner) = call.export_data(inner, ExportLimits::default())? else {
            panic!("numeric native recursion result")
        };
        self.engine
            .upgrade()
            .unwrap()
            .collect_garbage(call.context())?;
        let preserved = call.get(original, PropertyKey::String(&key))?;
        assert_eq!(
            call.export_data(preserved, ExportLimits::default())?,
            JsData::Number(n)
        );
        assert_eq!(call.arguments().len(), 2);
        call.import_data(JsData::Number(n + inner))
    }
}

#[test]
fn same_registration_survives_depth_three_reentry_and_native_gc_with_outer_arguments() {
    let engine = Rc::new(QuickJsNgEngine::new().unwrap());
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let visited = Rc::new(RefCell::new(Vec::new()));
    let drops = Rc::new(Cell::new(0));
    install(
        &engine,
        context,
        "recurse",
        Rc::new(RecurseWithGc {
            engine: Rc::downgrade(&engine),
            visited: visited.clone(),
            drops: drops.clone(),
        }),
    );
    run(&engine, context, |scope| {
        let result = scope.source().unwrap().evaluate(&source("(function again(n) { return n === 0 ? 0 : recurse(again, {n, keep: [n, n + 1]}); })(3)")).map_err(|error| scope.detach(error))?;
        assert_eq!(scope.export_data(result, ExportLimits::default()).map_err(|error| scope.detach(error))?, JsData::Number(6.0));
        Ok(())
    }).unwrap();
    assert_eq!(*visited.borrow(), [3.0, 2.0, 1.0]);
    assert_eq!(drops.get(), 0);
    engine.close_context(context).unwrap();
    assert_eq!(drops.get(), 1);
}

struct DropProbe {
    engine: Weak<QuickJsNgEngine>,
    context: ContextId,
    drops: Rc<Cell<usize>>,
    attempts: Rc<RefCell<Vec<FailureCode>>>,
    body_entered: Rc<Cell<bool>>,
    panic: bool,
    // Persistent roots use non-owning release metadata, so this capture must not
    // form registration -> root table -> context -> registration ownership.
    _captured: Option<RootedValue>,
}
impl HostFunction for DropProbe {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        Ok(call.receiver())
    }
}
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
        if let Some(engine) = self.engine.upgrade() {
            let result = run(&engine, self.context, |scope| {
                self.body_entered.set(true);
                scope
                    .source()
                    .unwrap()
                    .evaluate(&source("globalThis.finalizerExecutedJavaScript = true"))
                    .map_err(|error| scope.detach(error))?;
                Ok(())
            });
            self.attempts.borrow_mut().push(result.unwrap_err().code);
        }
        assert!(!self.panic, "registration destructor panic");
    }
}

#[test]
fn native_cycle_gc_drops_userdata_once_and_blocks_finalizer_native_entry() {
    let engine = Rc::new(QuickJsNgEngine::new().unwrap());
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let drops = Rc::new(Cell::new(0));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let body_entered = Rc::new(Cell::new(false));
    install(
        &engine,
        context,
        "probe",
        Rc::new(DropProbe {
            engine: Rc::downgrade(&engine),
            context,
            drops: drops.clone(),
            attempts: attempts.clone(),
            body_entered: body_entered.clone(),
            panic: false,
            _captured: None,
        }),
    );
    execute(&engine, context, "probe.cycle = probe");
    engine.collect_garbage(context).unwrap();
    assert_eq!(
        drops.get(),
        0,
        "reachable native function retains its userdata"
    );
    execute(&engine, context, "delete globalThis.probe");
    assert_eq!(
        drops.get(),
        0,
        "the native reference cycle awaits collection"
    );
    engine.collect_garbage(context).unwrap();
    assert_eq!(drops.get(), 1);
    assert_eq!(*attempts.borrow(), [FailureCode::ActiveContext]);
    assert!(!body_entered.get());
    engine.collect_garbage(context).unwrap();
    engine.close_context(context).unwrap();
    assert_eq!(drops.get(), 1);
}

#[test]
fn panicking_registration_drop_preserves_fault_and_continues_context_cleanup() {
    let engine = Rc::new(QuickJsNgEngine::new().unwrap());
    let weak = Rc::downgrade(&engine);
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let body_entered = Rc::new(Cell::new(false));
    let first = Rc::new(Cell::new(0));
    let second = Rc::new(Cell::new(0));
    let mut external = None;
    run(&engine, context, |scope| {
        let object = scope
            .import_data(JsData::Record(vec![]))
            .map_err(|error| scope.detach(error))?;
        external = Some(scope.retain(object)?);
        Ok(())
    })
    .unwrap();
    for (name, drops, panic) in [
        ("panickingProbe", first.clone(), true),
        ("remainingProbe", second.clone(), false),
    ] {
        let mut captured = None;
        run(&engine, context, |scope| {
            let object = scope
                .local(external.as_ref().unwrap())
                .map_err(|error| scope.detach(error))?;
            captured = Some(scope.retain(object)?);
            Ok(())
        })
        .unwrap();
        install(
            &engine,
            context,
            name,
            Rc::new(DropProbe {
                engine: weak.clone(),
                context,
                drops,
                attempts: attempts.clone(),
                body_entered: body_entered.clone(),
                panic,
                _captured: captured,
            }),
        );
    }
    execute(
        &engine,
        context,
        "panickingProbe.cycle = panickingProbe; remainingProbe.cycle = remainingProbe",
    );
    let error = engine.close_context(context).unwrap_err();
    assert_eq!(error.class, FailureClass::Host);
    assert_eq!(error.code, FailureCode::HostPanic);
    assert_eq!(
        (first.get(), second.get()),
        (1, 1),
        "cleanup continues after one host destructor panics"
    );
    assert!(!body_entered.get());
    assert_eq!(attempts.borrow().len(), 2);
    assert!(attempts.borrow().iter().all(|code| matches!(
        code,
        FailureCode::ActiveContext | FailureCode::ClosedContext
    )));
    engine.close_context(context).unwrap();
    assert_eq!(
        run(&engine, context, |_| Ok(())).unwrap_err().code,
        FailureCode::ClosedContext
    );
    drop(engine);
    assert!(
        weak.upgrade().is_none(),
        "native registrations cannot keep the engine alive"
    );
    drop(external);
    assert_eq!((first.get(), second.get()), (1, 1));
}

struct PanickingPayload(Arc<AtomicUsize>);
impl Drop for PanickingPayload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("panic payload destructor also panicked");
    }
}
struct PanicTwice(Arc<AtomicUsize>);
impl HostFunction for PanicTwice {
    fn invoke<'s>(&self, _: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        panic_any(PanickingPayload(self.0.clone()));
    }
}

#[test]
fn a_panicking_payload_destructor_also_stays_inside_the_native_boundary() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    install(
        &engine,
        context,
        "panicTwice",
        Rc::new(PanicTwice(drops.clone())),
    );
    let error = run(&engine, context, |scope| {
        scope
            .source()
            .unwrap()
            .evaluate(&source("panicTwice()"))
            .map_err(|error| scope.detach(error))?;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.class, FailureClass::Host);
    assert_eq!(error.code, FailureCode::HostPanic);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    engine.close_context(context).unwrap();
}

#[test]
fn owner_entry_contains_a_scope_panic_whose_payload_destructor_also_panics() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&engine, context, |_| {
            panic_any(PanickingPayload(drops.clone()));
        })
    }));
    assert!(
        outcome.is_ok(),
        "owner entry leaked a panic from the original payload destructor"
    );
    let error = outcome.unwrap().unwrap_err();
    assert_eq!(error.class, FailureClass::Host);
    assert_eq!(error.code, FailureCode::HostPanic);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    engine.close_context(context).unwrap();
    engine.close_context(context).unwrap();
}
