//! Owner-local host registrations and unwind-safe native callback trampolines.
use super::{ContextState, NativeEntry, NativeValue, RawContext, Scope, ffi};
use crate::engine::*;
use std::{
    ffi::{c_int, c_void},
    mem,
    num::NonZeroUsize,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::{Rc, Weak},
};

// Native callable -> registration -> context is deliberately non-owning. In
// particular, a registration capturing persistent roots cannot retain its engine.
struct Registration {
    function: Rc<dyn HostFunction>,
    state: Weak<ContextState>,
    raw: Weak<RawContext>,
}

fn host_panic() -> FailureReport {
    FailureReport::new(
        FailureClass::Host,
        FailureCode::HostPanic,
        "host callback or registration destructor panicked",
    )
}

// Panic payloads can themselves have panicking destructors. Nothing received
// from application code is dropped outside a catch at either foreign boundary.
pub(super) fn contain<T>(body: impl FnOnce() -> T) -> Result<T, ()> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            // Release ordinary payload userdata exactly once. A destructor can
            // itself panic, so only that secondary panic payload is abandoned:
            // dropping it could otherwise recurse or unwind across the ABI.
            if let Err(secondary) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
                mem::forget(secondary);
            }
            Err(())
        }
    }
}

fn record(raw: &RawContext, report: FailureReport) {
    // record_fault retains the first bounded report. No value-store or registry
    // borrow is live here, including when this runs during native destruction.
    let _ = contain(|| raw.record_fault(report));
}

pub(super) fn register<'s>(
    scope: &'s Scope<'_, '_>,
    name: &JsString,
    arity: u16,
    function: Rc<dyn HostFunction>,
) -> JsResult<'s, ScopedValue<'s>> {
    let context = scope.state.raw.ptr()?;
    // SAFETY: ptr checked the live owner context; name is borrowed for this call.
    let name = scope.put(unsafe {
        ffi::jbq_string(context, name.as_units().as_ptr(), name.as_units().len())
    })?;
    let name = scope.read(name)?;
    let registration = Box::new(Registration {
        function,
        state: Rc::downgrade(scope.state),
        raw: Rc::downgrade(&scope.state.raw),
    });
    // SAFETY: C consumes this Box on every path, invoking release exactly once.
    // The name wrapper remains rooted across allocations and possible GC.
    let function = unsafe {
        ffi::jbq_host_function(
            context,
            name.value,
            arity,
            invoke,
            Box::into_raw(registration).cast(),
            release,
        )
    };
    scope.put(function)
}

unsafe extern "C" fn invoke(
    opaque: *mut c_void,
    context: *mut ffi::Context,
    receiver: ffi::Value,
    argv: *const c_void,
    argc: usize,
    thrown: *mut c_int,
) -> ffi::Value {
    // SAFETY: native host_call owns the registration and its callback arguments
    // for this entire synchronous invocation. No borrowed argv escapes it.
    let result = contain(|| unsafe { invoke_inner(opaque, context, receiver, argv, argc) })
        .unwrap_or_else(|()| Err(host_panic()));
    let (value, status) = match result {
        Ok(result) => result,
        Err(report) => {
            if !opaque.is_null() {
                // SAFETY: opaque is supplied by our registration constructor and
                // C keeps the callable's holder alive throughout its active call.
                let registration = unsafe { &*opaque.cast::<Registration>() };
                if let Some(raw) = registration.raw.upgrade() {
                    record(&raw, report);
                }
            }
            // SAFETY: undefined has no ownership or context prerequisites.
            (unsafe { ffi::jbq_undefined() }, 2)
        }
    };
    if !thrown.is_null() {
        // SAFETY: native host_call supplies its valid stack-local status output.
        unsafe { *thrown = status };
    }
    value
}

unsafe fn invoke_inner(
    opaque: *mut c_void,
    context: *mut ffi::Context,
    receiver: ffi::Value,
    argv: *const c_void,
    argc: usize,
) -> Result<(ffi::Value, c_int), FailureReport> {
    if opaque.is_null() {
        return Err(FailureReport::fault(
            FailureCode::InvalidHandle,
            "missing host registration",
        ));
    }
    // SAFETY: the native callable owns this registration throughout the call.
    let registration = unsafe { &*opaque.cast::<Registration>() };
    let raw = registration.raw.upgrade().ok_or_else(|| {
        FailureReport::fault(FailureCode::ClosedContext, "host context is closed")
    })?;
    if raw.ptr()? != context {
        return Err(FailureReport::fault(
            FailureCode::WrongContext,
            "host callback belongs to another native context",
        ));
    }
    // Validate the native context before reading its entry field or arguments.
    // SAFETY: raw.ptr established this is our currently live native context.
    let entry = unsafe { ffi::jbq_get_entry(context) };
    if entry.is_null() || (argc != 0 && argv.is_null()) {
        return Err(FailureReport::fault(
            FailureCode::InvalidHandle,
            "host callback has no active owner entry or argument storage",
        ));
    }
    let state = registration.state.upgrade().ok_or_else(|| {
        FailureReport::fault(FailureCode::ClosedContext, "host context is closed")
    })?;
    let function = Rc::clone(&registration.function);
    // SAFETY: the parent's synchronous entry guard installs NativeEntry only
    // while its borrowed HostServices is alive, and restores the previous entry
    // after native execution. This callback cannot outlive that enclosing entry.
    let entry = unsafe { &*entry.cast::<NativeEntry<'_>>() };
    state.values.with_scope(|values| {
        // Each recursive call gets a new frame and its own stable Vec. All store
        // borrows end before invoking arbitrary host code or reentering native JS.
        let receiver = values.insert(NativeValue {
            // SAFETY: receiver is borrowed from this context for this callback.
            value: unsafe { duplicate(context, receiver) },
            raw: Rc::clone(&raw),
            resolver: false,
            settled: std::cell::Cell::new(false),
        })?;
        let mut arguments = argument_storage(argc)?;
        for index in 0..argc {
            // SAFETY: index is within native argc, and argv is only used during
            // this callback. Duplicate the borrowed element before adopting it.
            let value = unsafe { duplicate(context, ffi::jbq_argv_get(argv, index)) };
            arguments.push(values.insert(NativeValue {
                value,
                raw: Rc::clone(&raw),
                resolver: false,
                settled: std::cell::Cell::new(false),
            })?);
        }
        let scope = Scope {
            state: &state,
            values,
            host: entry.host,
        };
        let call = Call {
            scope: &scope,
            receiver,
            arguments,
        };
        let (value, thrown) = match function.invoke(&call) {
            Ok(value) => (value, 0),
            Err(ScopedError::Exception(value)) => (value, 1),
            // Fix: a host function handed data past a copy limit returned a `Capacity`
            // fault, which ended the program (`Capacity: copied data limits exceeded`)
            // where JavaScript expects an error it can catch.
            // Decision: a `Capacity` fault is thrown as a `RangeError` here, once for every
            // host function, rather than mapped at each call site. Every other fault code
            // is a broken contract or a closed context and stays fatal. If the error
            // itself cannot be built, the original fault stands.
            Err(ScopedError::Fault(report)) if report.code == FailureCode::Capacity => {
                match capacity_error(&call, &report) {
                    Ok(error) => (error, 1),
                    Err(_) => return Err(report),
                }
            }
            Err(ScopedError::Fault(report)) => return Err(report),
        };
        let value = scope.read(value)?;
        // SAFETY: duplicate the validated original result/exception before its
        // local callback frame unwinds; C consumes this separate owned reference.
        Ok((unsafe { duplicate(context, value.value) }, thrown))
    })
}

/// Another owned reference to `value`.
///
/// Fix: the receiver, every argument and the result of each host call were duplicated with
/// a call into the shim, also numbers, booleans, null and undefined, which have no reference
/// count. Decision: only reference-counted values make the call.
///
/// # Arguments
/// * `context` - The live native context `value` belongs to.
/// * `value` - A live value borrowed from that context.
///
/// # Returns
/// The owned value.
///
/// # Safety
/// `context` must be the live native context and `value` a live value of it.
unsafe fn duplicate(context: *mut ffi::Context, value: ffi::Value) -> ffi::Value {
    if value.has_ref() {
        // SAFETY: the caller's contract.
        unsafe { ffi::jbq_dup(context, value) }
    } else {
        value
    }
}

/// The `RangeError` a host function's `Capacity` fault is thrown as.
///
/// # Arguments
/// * `call` - The host call the fault came from.
/// * `report` - The fault; its message becomes the error's `message`.
///
/// # Returns
/// A new `RangeError` from the context's global constructor.
///
/// # Errors
/// The constructor cannot be read or the error cannot be built.
fn capacity_error<'s>(
    call: &'s dyn HostCall,
    report: &FailureReport,
) -> JsResult<'s, ScopedValue<'s>> {
    let global = call.global()?;
    let constructor = call.get(
        global,
        PropertyKey::String(&JsString::from_utf8("RangeError")),
    )?;
    let message = report
        .message
        .clone()
        .unwrap_or_else(|| JsString::from_utf8("data exceeds a host limit"));
    let message = call.import_data(JsData::String(message))?;
    call.construct(constructor, &[message], None)
}

fn argument_storage<'a>(argc: usize) -> Result<Vec<ScopedValue<'a>>, FailureReport> {
    let mut arguments = Vec::new();
    arguments.try_reserve_exact(argc).map_err(|_| {
        FailureReport::fault(
            FailureCode::Capacity,
            "host arguments exceed available capacity",
        )
    })?;
    Ok(arguments)
}

struct Finalizing<'a> {
    raw: &'a RawContext,
    previous: usize,
}
impl Drop for Finalizing<'_> {
    fn drop(&mut self) {
        self.raw.finalizing.set(self.previous);
    }
}

unsafe extern "C" fn release(opaque: *mut c_void) {
    if opaque.is_null() {
        return;
    }
    // SAFETY: C transfers its single registration allocation back exactly once.
    // Box's drop glue releases the allocation even if HostFunction::drop panics.
    let registration = unsafe { Box::from_raw(opaque.cast::<Registration>()) };
    let raw = registration.raw.upgrade();
    let _finalizing = raw.as_deref().map(|raw| {
        let previous = raw.finalizing.get();
        raw.finalizing.set(previous.saturating_add(1));
        Finalizing { raw, previous }
    });
    if contain(|| drop(registration)).is_err()
        && let Some(raw) = &raw
    {
        record(raw, host_panic());
    }
}

// The call borrows Scope, rather than moving it after creating scoped tokens.
// Forwarding keeps all operations in the same callback-local value frame.
struct Call<'call, 'state, 'store> {
    scope: &'call Scope<'state, 'store>,
    receiver: ScopedValue<'call>,
    arguments: Vec<ScopedValue<'call>>,
}

impl HostCall for Call<'_, '_, '_> {
    fn receiver<'s>(&'s self) -> ScopedValue<'s> {
        self.receiver
    }
    fn arguments<'s>(&'s self) -> &'s [ScopedValue<'s>] {
        &self.arguments
    }
    fn host(&self) -> &dyn HostServices {
        self.scope.host
    }
}

impl JsScope for Call<'_, '_, '_> {
    fn data_limits(&self) -> ExportLimits {
        self.scope.data_limits()
    }
    fn context(&self) -> ContextId {
        self.scope.context()
    }
    fn global<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.global()
    }
    fn kind(&self, value: ScopedValue<'_>) -> Result<ValueKind, FailureReport> {
        self.scope.kind(value)
    }
    fn local<'s>(&'s self, value: &RootedValue) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.local(value)
    }
    fn retain(&self, value: ScopedValue<'_>) -> Result<RootedValue, FailureReport> {
        self.scope.retain(value)
    }
    fn import_data<'s>(&'s self, data: JsData) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.import_data(data)
    }
    fn export_data<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, JsData> {
        self.scope.export_data(value, limits)
    }
    fn export_utf8<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, Option<String>> {
        self.scope.export_utf8(value, limits)
    }
    fn import_utf8<'s>(&'s self, text: &str) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.import_utf8(text)
    }
    /// Forwards to the scope's fast Number read.
    ///
    /// # Arguments
    /// * `value` - The value.
    ///
    /// # Returns
    /// The number, or `None` for any other kind.
    ///
    /// # Errors
    /// As [`JsScope::number`].
    fn number<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, Option<f64>> {
        JsScope::number(self.scope, value)
    }
    fn get<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.get(object, key)
    }
    fn set<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
        value: ScopedValue<'s>,
    ) -> JsResult<'s, ()> {
        self.scope.set(object, key, value)
    }
    fn call<'s>(
        &'s self,
        function: ScopedValue<'s>,
        receiver: Option<ScopedValue<'s>>,
        arguments: &[ScopedValue<'s>],
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.call(function, receiver, arguments)
    }
    fn construct<'s>(
        &'s self,
        constructor: ScopedValue<'s>,
        arguments: &[ScopedValue<'s>],
        new_target: Option<ScopedValue<'s>>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.construct(constructor, arguments, new_target)
    }
    fn host_function<'s>(
        &'s self,
        name: &JsString,
        arity: u16,
        function: Rc<dyn HostFunction>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.host_function(name, arity, function)
    }
    fn materialize<'s>(
        &'s self,
        definition: &Rc<HostValueDefinition>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.materialize(definition)
    }
    fn deferred<'s>(&'s self) -> JsResult<'s, Deferred<'s>> {
        self.scope.deferred()
    }
    fn settle<'s>(
        &'s self,
        deferred: RootedDeferred,
        settlement: Settlement<'s>,
    ) -> JsResult<'s, ()> {
        self.scope.settle(deferred, settlement)
    }
    fn drain_microtasks(
        &self,
        budget_hint: NonZeroUsize,
    ) -> Result<CheckpointReport, FailureReport> {
        self.scope.drain_microtasks(budget_hint)
    }
    fn enqueue_microtask<'s>(&'s self, function: ScopedValue<'s>) -> JsResult<'s, ()> {
        self.scope.enqueue_microtask(function)
    }
    fn async_context<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        self.scope.async_context()
    }
    fn set_async_context<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, ()> {
        self.scope.set_async_context(value)
    }
    fn take_rejection<'s>(&'s self) -> JsResult<'s, Option<Rejection<'s>>> {
        self.scope.take_rejection()
    }
    fn detach_buffer<'s>(&'s self, buffer: ScopedValue<'s>) -> JsResult<'s, ()> {
        self.scope.detach_buffer(buffer)
    }
    fn internals<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, Internals<'s>> {
        self.scope.internals(value)
    }
    fn detach(&self, error: ScopedError<'_>) -> FailureReport {
        self.scope.detach(error)
    }
    fn source(&self) -> Option<&dyn SourceApi> {
        self.scope.source()
    }
    fn compiler(&self) -> Option<&dyn CompilerApi> {
        self.scope.compiler()
    }
    fn artifacts(&self) -> Option<&dyn ArtifactApi> {
        self.scope.artifacts()
    }
    fn modules(&self) -> Option<&dyn ModuleApi> {
        self.scope.modules()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/quickjs_ng_callback_coverage.rs"]
mod tests;
