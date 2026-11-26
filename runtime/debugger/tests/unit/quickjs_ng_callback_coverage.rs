//! Real native callback delegation and private boundary guards.
use super::super::{EntryGuard, QuickJsNgEngine};
use super::*;
use std::{cell::Cell, ptr, time::Duration};

#[derive(Default)]
struct Services(Cell<usize>);
impl HostServices for Services {
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
        self.0.set(self.0.get() + 1);
        Ok(())
    }
    fn reserve_operation(&self, _: usize) -> Result<OperationReservation, FailureReport> {
        Err(host_panic())
    }
    fn bind_deferred(
        &self,
        _: OperationReservation,
        _: RootedDeferred,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        Err(host_panic())
    }
    fn bind_callback(
        &self,
        _: OperationReservation,
        _: RootedValue,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        Err(host_panic())
    }
    fn cancel_operation(&self, _: OperationId) -> CancelOutcome {
        CancelOutcome::NotFound
    }
}
struct Receiver;
impl HostFunction for Receiver {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        Ok(call.receiver())
    }
}
fn make_registration(state: &Rc<ContextState>) -> Registration {
    Registration {
        function: Rc::new(Receiver),
        state: Rc::downgrade(state),
        raw: Rc::downgrade(&state.raw),
    }
}
fn source(program: &str) -> ScriptSource {
    ScriptSource::new(
        JsString::from_utf8(program),
        "file:///callback-delegation.js".into(),
    )
}

struct RegistrationDrop(Rc<Cell<usize>>);
impl HostFunction for RegistrationDrop {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        Ok(call.receiver())
    }
}
impl Drop for RegistrationDrop {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn native_name_allocation_failure_releases_unregistered_host_function_once() {
    let engine = QuickJsNgEngine::with_config(super::super::QuickJsNgConfig {
        max_memory_bytes: Some(512 * 1024),
        ..Default::default()
    })
    .unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let drops = Rc::new(Cell::new(0));
    // Even one-byte native string compression cannot fit this complete name in
    // the runtime's real bounded heap. Rust owns the source UTF-16 independently.
    let name = JsString::from_units(vec![0x1234; 1024 * 1024]);
    engine
        .with_scope(context, &Services::default(), &mut |scope: &dyn JsScope| {
            let function = Rc::new(RegistrationDrop(drops.clone()));
            let observer = Rc::downgrade(&function);
            let error = scope.host_function(&name, 0, function).unwrap_err();
            let report = scope.detach(error);
            assert_eq!(report.class, FailureClass::Engine);
            assert_eq!(report.code, FailureCode::EngineFailure);
            assert!(observer.upgrade().is_none());
            assert_eq!(drops.get(), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(engine.context(context).unwrap().values.live_roots(), 0);
    engine.close_context(context).unwrap();
    drop(engine);
    assert_eq!(drops.get(), 1);
}

// This helper only supplies real registrations, live native contexts and the
// context-free undefined primitive. Rejection occurs before any argv access.
fn guarded(
    registration: Option<&Registration>,
    context: *mut ffi::Context,
    argc: usize,
) -> Result<(ffi::Value, c_int), FailureReport> {
    let opaque = registration.map_or(ptr::null_mut(), |r| ptr::from_ref(r).cast_mut().cast());
    // SAFETY: All supplied registration/context pointers remain live. A null
    // argv with positive argc is the explicit guard case; it is never read.
    unsafe { invoke_inner(opaque, context, ffi::jbq_undefined(), ptr::null(), argc) }
}

#[test]
fn callback_guards_reject_missing_expired_closed_and_foreign_inputs() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let other = engine.create_context(ContextLimits::default()).unwrap();
    let state = engine.context(context).unwrap();
    let raw = state.raw.ptr().unwrap();
    let mut registration = make_registration(&state);
    assert_eq!(
        guarded(None, raw, 0).unwrap_err().code,
        FailureCode::InvalidHandle
    );
    assert_eq!(
        guarded(
            Some(&registration),
            engine.context(other).unwrap().raw.ptr().unwrap(),
            0
        )
        .unwrap_err()
        .code,
        FailureCode::WrongContext
    );
    assert_eq!(
        guarded(Some(&registration), raw, 0).unwrap_err().code,
        FailureCode::InvalidHandle
    );
    let services = Services::default();
    let entry = NativeEntry { host: &services };
    {
        let _entry = EntryGuard::new(&state.raw, &entry).unwrap();
        assert_eq!(
            guarded(Some(&registration), raw, 1).unwrap_err().code,
            FailureCode::InvalidHandle
        );
        registration.state = Weak::new();
        assert_eq!(
            guarded(Some(&registration), raw, 0).unwrap_err().code,
            FailureCode::ClosedContext
        );
        assert_eq!(
            argument_storage(usize::MAX).unwrap_err().code,
            FailureCode::Capacity
        );
        assert!(argument_storage(0).unwrap().is_empty());
    }
    registration.raw = Weak::new();
    assert_eq!(
        guarded(Some(&registration), raw, 0).unwrap_err().code,
        FailureCode::ClosedContext
    );
    registration.raw = Rc::downgrade(&state.raw);
    engine.close_context(context).unwrap();
    // The pointer argument is no longer inspected: RawContext rejects its
    // closed state before comparing or dereferencing the supplied context.
    assert_eq!(
        guarded(Some(&registration), ptr::null_mut(), 0)
            .unwrap_err()
            .code,
        FailureCode::ClosedContext
    );
    engine.close_context(other).unwrap();
}

#[test]
fn foreign_boundary_accepts_null_outputs_and_records_only_first_fault() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let state = engine.context(context).unwrap();
    let raw = state.raw.ptr().unwrap();
    let mut status = -1;
    // SAFETY: Missing opaque is rejected before any pointer/argv access;
    // status is a valid stack output and undefined owns no native resource.
    unsafe {
        invoke(
            ptr::null_mut(),
            raw,
            ffi::jbq_undefined(),
            ptr::null(),
            0,
            &mut status,
        );
    }
    assert_eq!(status, 2);
    let mut registration = make_registration(&state);
    let opaque = ptr::from_mut(&mut registration).cast();
    // SAFETY: Registration and context are live. No entry is installed, so
    // the guard returns before arguments are read; null status is permitted.
    unsafe {
        invoke(
            opaque,
            raw,
            ffi::jbq_undefined(),
            ptr::null(),
            0,
            ptr::null_mut(),
        );
    }
    let first = state.raw.take_fault().unwrap();
    assert_eq!(first.code, FailureCode::InvalidHandle);
    record(&state.raw, first);
    record(&state.raw, host_panic());
    assert_eq!(
        state.raw.take_fault().unwrap().code,
        FailureCode::InvalidHandle
    );
    {
        let _busy = state.raw.pending.borrow_mut();
        record(&state.raw, host_panic());
    }
    assert!(
        state.raw.take_fault().is_none(),
        "recording failures never unwind across C"
    );
    registration.raw = Weak::new();
    // SAFETY: The live registration has an expired raw Weak, so rejection
    // occurs before the live context or any native argument is examined.
    unsafe {
        invoke(
            opaque,
            raw,
            ffi::jbq_undefined(),
            ptr::null(),
            0,
            &mut status,
        );
    }
    assert_eq!(status, 2);
    assert!(state.raw.take_fault().is_none());
    engine.close_context(context).unwrap();
}

struct DropPanic;
impl HostFunction for DropPanic {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        Ok(call.receiver())
    }
}
impl Drop for DropPanic {
    fn drop(&mut self) {
        panic!("registration destructor");
    }
}

#[test]
fn release_restores_nested_finalizer_depth_and_contains_destructors() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let state = engine.context(context).unwrap();
    state.raw.finalizing.set(7);
    let mut registration = make_registration(&state);
    registration.function = Rc::new(DropPanic);
    // SAFETY: Ownership of the unique Box transfers to release once. The
    // null path owns nothing; both execute on the real context owner thread.
    unsafe {
        release(ptr::null_mut());
        release(Box::into_raw(Box::new(registration)).cast());
    }
    assert_eq!(state.raw.finalizing.get(), 7);
    assert_eq!(state.raw.take_fault().unwrap().code, FailureCode::HostPanic);
    state.raw.finalizing.set(0);
    let mut expired = make_registration(&state);
    expired.raw = Weak::new();
    expired.state = Weak::new();
    // SAFETY: This independently owned Box is transferred exactly once;
    // expired weak references require no native context during destruction.
    unsafe {
        release(Box::into_raw(Box::new(expired)).cast());
    }
    engine.close_context(context).unwrap();
}

struct Delegate {
    context: ContextId,
}
impl HostFunction for Delegate {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        assert_eq!(call.context(), self.context);
        assert_eq!(call.arguments().len(), 1);
        assert_eq!(call.kind(call.receiver())?, ValueKind::Object);
        call.host().request_stop()?;
        let value = call.arguments()[0];
        let retained = call.retain(value)?;
        let local = call.local(&retained)?;
        assert_eq!(
            call.export_data(local, ExportLimits::default())?,
            JsData::Number(21.0)
        );
        let global = call.global()?;
        let key = JsString::from_utf8("delegatedValue");
        call.set(global, PropertyKey::String(&key), local)?;
        assert_eq!(
            call.export_data(
                call.get(global, PropertyKey::String(&key))?,
                ExportLimits::default()
            )?,
            JsData::Number(21.0)
        );
        let nested = call.host_function(&JsString::from_utf8("nested"), 0, Rc::new(Receiver))?;
        assert_eq!(
            call.kind(call.call(nested, Some(global), &[])?)?,
            ValueKind::Object
        );
        let constructor = call
            .source()
            .unwrap()
            .evaluate(&source("(function Result(value) { this.value = value; })"))?;
        let object = call.construct(constructor, &[local], Some(constructor))?;
        let object_key = JsString::from_utf8("value");
        assert_eq!(
            call.export_data(
                call.get(object, PropertyKey::String(&object_key))?,
                ExportLimits::default()
            )?,
            JsData::Number(21.0)
        );
        let artifact = call.compiler().unwrap().compile(
            &source("20 + 22"),
            QuickJsNgEngine::script_compile_options(),
        )?;
        let answer = call.artifacts().unwrap().execute(&artifact)?;
        assert_eq!(
            call.export_data(answer, ExportLimits::default())?,
            JsData::Number(42.0)
        );
        assert!(call.modules().is_some());
        let deferred = call.deferred()?;
        let promise_key = JsString::from_utf8("delegatedPromise");
        call.set(global, PropertyKey::String(&promise_key), deferred.promise)?;
        call.source().unwrap().evaluate(&source(
            "delegatedPromise.then(value => { globalThis.delegatedSettled = value; })",
        ))?;
        call.settle(deferred.resolver, Settlement::Resolve(answer))?;
        let checkpoint = call.drain_microtasks(NonZeroUsize::new(10).unwrap())?;
        assert_eq!(checkpoint, CheckpointReport::empty());
        let result = call
            .source()
            .unwrap()
            .evaluate(&source("delegatedSettled"))?;
        assert_eq!(
            call.export_data(result, ExportLimits::default())?,
            JsData::Number(42.0)
        );
        let error = call
            .source()
            .unwrap()
            .evaluate(&source("throw 17"))
            .unwrap_err();
        assert_eq!(call.detach(error).thrown, Some(JsData::Number(17.0)));
        call.import_data(JsData::Number(42.0))
    }
}

#[test]
fn real_native_callback_delegates_every_scope_operation_and_reenters() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let services = Services::default();
    engine
        .with_scope(context, &services, &mut |scope: &dyn JsScope| {
            let callback = scope
                .host_function(
                    &JsString::from_utf8("delegate"),
                    1,
                    Rc::new(Delegate { context }),
                )
                .map_err(|e| scope.detach(e))?;
            let receiver = scope
                .import_data(JsData::Record(vec![]))
                .map_err(|e| scope.detach(e))?;
            let argument = scope
                .import_data(JsData::Number(21.0))
                .map_err(|e| scope.detach(e))?;
            let result = scope
                .call(callback, Some(receiver), &[argument])
                .map_err(|e| scope.detach(e))?;
            assert_eq!(
                scope
                    .export_data(result, ExportLimits::default())
                    .map_err(|e| scope.detach(e))?,
                JsData::Number(42.0)
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(services.0.get(), 1);
    // A second owner entry proves the callback's recursive native entry,
    // temporary frame, and retained resolver/root cleanup were restored.
    engine
        .with_scope(context, &services, &mut |scope: &dyn JsScope| {
            assert_eq!(scope.context(), context);
            Ok(())
        })
        .unwrap();
    assert_eq!(engine.context(context).unwrap().values.live_roots(), 0);
    engine.close_context(context).unwrap();
}

struct Reject(bool);
impl HostFunction for Reject {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        if self.0 {
            return Err(host_panic().into());
        }
        Err(ScopedError::Exception(
            call.import_data(JsData::Number(91.0))?,
        ))
    }
}

struct Panics;
impl HostFunction for Panics {
    fn invoke<'s>(&self, _: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        panic!("native callback panic")
    }
}

#[test]
fn native_callback_contains_panics_and_restores_the_owner_entry() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let services = Services::default();
    engine
        .with_scope(context, &services, &mut |scope: &dyn JsScope| {
            let callback = scope
                .host_function(&JsString::from_utf8("panic"), 0, Rc::new(Panics))
                .map_err(|error| scope.detach(error))?;
            let error = scope.call(callback, None, &[]).unwrap_err();
            assert_eq!(scope.detach(error).code, FailureCode::HostPanic);
            Ok(())
        })
        .unwrap();
    engine
        .with_scope(context, &services, &mut |scope: &dyn JsScope| {
            let result = scope
                .source()
                .unwrap()
                .evaluate(&source("6 * 7"))
                .map_err(|error| scope.detach(error))?;
            assert_eq!(
                scope
                    .export_data(result, ExportLimits::default())
                    .map_err(|error| scope.detach(error))?,
                JsData::Number(42.0)
            );
            Ok(())
        })
        .unwrap();
    engine.close_context(context).unwrap();
}

#[test]
fn native_callback_keeps_script_exception_distinct_from_host_fault() {
    for fault in [false, true] {
        let engine = QuickJsNgEngine::new().unwrap();
        let context = engine.create_context(ContextLimits::default()).unwrap();
        engine
            .with_scope(context, &Services::default(), &mut |scope: &dyn JsScope| {
                let callback = scope
                    .host_function(&JsString::from_utf8("reject"), 0, Rc::new(Reject(fault)))
                    .map_err(|e| scope.detach(e))?;
                let error = scope.call(callback, None, &[]).unwrap_err();
                let report = scope.detach(error);
                assert_eq!(
                    report.code,
                    if fault {
                        FailureCode::HostPanic
                    } else {
                        FailureCode::ScriptException
                    }
                );
                if !fault {
                    assert_eq!(report.thrown, Some(JsData::Number(91.0)));
                }
                Ok(())
            })
            .unwrap();
        engine.close_context(context).unwrap();
    }
}
