//! Owner-thread QuickJS-NG adapter using the pinned native C engine directly.
//!
//! Source evaluation, script compilation/artifacts, values and Promise checkpoints
//! implement the shared contract. Stock QuickJS-NG has no qualified interactive
//! backend: select an explicit Disabled debugging profile. Default Development
//! requirements still fail with Unsupported before application execution.
mod attach;
mod compiler;
mod callbacks;
mod data;
mod debug;
mod ffi;
mod modules;
mod scope;

/// Links the forked `rquickjs-sys`, the one crate that compiles QuickJS-NG and the `jbq_*`
/// bridge declared in `ffi`; without a reference its archive is dropped from the link.
use rquickjs_sys as _;

pub use attach::Debugger;

use crate::{debug::DebugBackend, engine::*};
use scope::Scope;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

/// Exact upstream revision compiled into this adapter.
pub const NATIVE_REVISION: &str = "7b5f588e0f9b643e2996f291bfc861abeaceddc2";

/// Native heap and C-stack policy, independent of copied-data/root quotas.
#[derive(Clone, Debug)]
pub struct QuickJsNgConfig {
    /// Optional positive native heap allocation limit; None uses native unlimited policy.
    pub max_memory_bytes: Option<usize>,
    /// Positive native stack budget used by QuickJS's stack-overflow checks.
    pub max_stack_bytes: usize,
    /// Positive limit on queued promise and microtask jobs. A job past it is lost, so the
    /// context fails with a `Capacity` fault and runs no more JavaScript.
    pub max_pending_jobs: usize,
}
impl Default for QuickJsNgConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: None,
            max_stack_bytes: 512 * 1024,
            max_pending_jobs: 1 << 20,
        }
    }
}
/// Cross-thread cooperative abort metadata; never contains a native pointer.
///
/// Aborting raises a native interruption exception. It does not pause execution
/// for inspection and does not establish an interactive debugger capability.
#[derive(Clone, Debug)]
pub struct QuickJsNgInterrupt {
    requested: Arc<AtomicBool>,
}
impl QuickJsNgInterrupt {
    /// Ask native JavaScript to abort at its next interrupt check.
    pub fn request_abort(&self) {
        self.requested.store(true, Ordering::Release);
    }
    /// Clear the request before deliberately starting another execution.
    pub fn clear(&self) {
        self.requested.store(false, Ordering::Release);
    }
}

/// Actual QuickJS-NG implementation of the engine-neutral ownership contract.
///
/// Each host context owns an independent native runtime/context on this thread.
/// Native tokens are private; cross-thread work uses copied data or interrupt metadata.
pub struct QuickJsNgEngine {
    identity: EngineIdentity,
    descriptor: EngineDescriptor,
    config: QuickJsNgConfig,
    contexts: RefCell<HashMap<ContextId, Rc<ContextState>>>,
    /// The context looked up last, cleared whenever `contexts` changes.
    ///
    /// Fix: every scope entry hashed the context id into `contexts`, twice per turn (the
    /// turn and its checkpoint), although a VM enters the same context each time.
    /// Decision: a one-entry cache in front of the map.
    recent: RefCell<Option<(ContextId, Rc<ContextState>)>>,
    closed: RefCell<HashSet<ContextId>>,
}
impl QuickJsNgEngine {
    /// Construct an owner-local adapter without creating or executing a context.
    pub fn new() -> Result<Self, FailureReport> {
        Self::with_config(QuickJsNgConfig::default())
    }
    /// Validate native resource limits and capture the creating thread identity.
    pub fn with_config(config: QuickJsNgConfig) -> Result<Self, FailureReport> {
        if config.max_memory_bytes == Some(0)
            || config.max_stack_bytes == 0
            || config.max_pending_jobs == 0
        {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "native limits must be positive",
            ));
        }
        let identity = EngineIdentity::new()?;
        let descriptor = EngineDescriptor {
            adapter_id: "quickjs-ng".into(),
            adapter_version: env!("CARGO_PKG_VERSION").into(),
            native_version: NATIVE_REVISION.into(),
            contract_version: 1,
            language_features: "Native QuickJS-NG; source, trusted script artifacts, native modules, top-level await and dynamic import; interactive debugger unavailable".into(),
            target_fingerprint: env!("JETBUNNY_QJS_TARGET").into(),
            build_fingerprint: env!("JETBUNNY_QJS_BUILD_ID").into(),
            native_build_requirements: vec![
                "vendored pinned C11 QuickJS-NG + jetbunny-sticky-oom-v1 + jetbunny-modules-v3 + jetbunny-debugger-v2 + jetbunny-eval-control-v1 + jetbunny-binding-edit-v1 + jetbunny-safe-lookup-v1 + jetbunny-exception-info-v1 + jetbunny-async-context-v1 + jetbunny-rust-compiler-v1 + jetbunny-test262-vm-v1 + jetbunny-host-jobs-v1; private bridge ABI 14; explicit Disabled debugging".into(),
            ],
            owner: identity.owner(),
        };
        Ok(Self {
            identity,
            descriptor,
            config,
            contexts: RefCell::new(HashMap::new()),
            recent: RefCell::new(None),
            closed: RefCell::new(HashSet::new()),
        })
    }
    /// Return the supported script compiler policy; output is tied to this build.
    pub fn script_compile_options() -> CompileOptions {
        CompileOptions {
            mode: CodeMode::Script,
            debug_info: DebugInfo::Strip,
            fingerprint: "default".into(),
        }
    }
    fn owner(&self) -> Result<(), FailureReport> {
        validate_owner(self.identity.owner())
    }
    fn context(&self, id: ContextId) -> Result<Rc<ContextState>, FailureReport> {
        self.owner()?;
        if id.engine_epoch() != self.identity.epoch() {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign QuickJS context",
            ));
        }
        if let Some((recent, state)) = &*self.recent.borrow()
            && *recent == id
        {
            return Ok(state.clone());
        }
        let state = self.contexts.borrow().get(&id).cloned().ok_or_else(|| {
            FailureReport::fault(FailureCode::ClosedContext, "QuickJS context is closed")
        })?;
        *self.recent.borrow_mut() = Some((id, state.clone()));
        Ok(state)
    }
    /// Run native cycle collection while retaining all currently scoped/rooted values.
    pub fn collect_garbage(&self, context: ContextId) -> Result<(), FailureReport> {
        let state = self.context(context)?;
        let ptr = state.raw.ptr()?;
        // SAFETY: The checked owner context remains owned by state during collection.
        unsafe {
            ffi::jbq_gc(ptr);
        }
        state.raw.take_fault().map_or(Ok(()), Err)
    }
    /// Reports the runtime heap's allocated bytes and memory limit.
    ///
    /// # Arguments
    /// * `context` - A live context of this engine.
    ///
    /// # Returns
    /// Allocated bytes including allocator overhead, and the limit (`None` when unlimited).
    ///
    /// # Errors
    /// A foreign or closed context, or a call off the owner thread.
    pub fn heap_usage(&self, context: ContextId) -> Result<HeapUsage, FailureReport> {
        let state = self.context(context)?;
        let ptr = state.raw.ptr()?;
        let (mut used, mut limit) = (0i64, 0i64);
        // SAFETY: The checked owner context remains owned by state; the shim only reads
        // runtime counters into the two out-parameters.
        unsafe { ffi::jbq_heap_usage(ptr, &mut used, &mut limit) };
        Ok(HeapUsage {
            used_bytes: used.max(0) as u64,
            limit_bytes: (limit > 0).then_some(limit as u64),
        })
    }
    /// Borrow abort metadata for this live context, without exposing native state.
    pub fn interrupt_handle(
        &self,
        context: ContextId,
    ) -> Result<QuickJsNgInterrupt, FailureReport> {
        let state = self.context(context)?;
        state.raw.ptr()?;
        Ok(QuickJsNgInterrupt {
            requested: state.raw.interrupt.clone(),
        })
    }
}
fn validate_owner(owner: thread::ThreadId) -> Result<(), FailureReport> {
    // The cached thread id: see `current_thread`.
    if crate::engine::current_thread() != owner {
        Err(FailureReport::fault(
            FailureCode::WrongOwner,
            "QuickJS owner thread required",
        ))
    } else {
        Ok(())
    }
}
impl JsEngine for QuickJsNgEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }
    fn abort_signal(&self, context: ContextId) -> Result<Option<AbortSignal>, FailureReport> {
        let interrupt = self.interrupt_handle(context)?;
        Ok(Some(Arc::new(move || interrupt.request_abort())))
    }
    /// Trait entry for the host's collection request; runs [`QuickJsNgEngine::collect_garbage`].
    ///
    /// # Arguments
    /// * `context` - A live context of this engine.
    ///
    /// # Errors
    /// A foreign or closed context, a call off the owner thread, or a native fault.
    fn collect_garbage(&self, context: ContextId) -> Result<(), FailureReport> {
        QuickJsNgEngine::collect_garbage(self, context)
    }
    /// Trait entry for the host's heap query; runs [`QuickJsNgEngine::heap_usage`].
    ///
    /// # Arguments
    /// * `context` - A live context of this engine.
    ///
    /// # Returns
    /// The heap's allocated bytes and ceiling.
    ///
    /// # Errors
    /// A foreign or closed context, a call off the owner thread, or a native fault.
    fn heap_usage(&self, context: ContextId) -> Result<HeapUsage, FailureReport> {
        QuickJsNgEngine::heap_usage(self, context)
    }
    fn create_context(&self, limits: ContextLimits) -> Result<ContextId, FailureReport> {
        self.owner()?;
        limits.validate()?;
        let values = ValueStore::new(&self.identity, limits)?;
        let id = values.context();
        let interrupt = Arc::new(AtomicBool::new(false));
        // Decision: the runtime is rquickjs's, so its value layer (classes, typed functions,
        // promises) works on the same heap as the bridge; the bridge only borrows it.
        let runtime = rquickjs_core::Runtime::new().map_err(|_| {
            FailureReport::new(
                FailureClass::Engine,
                FailureCode::Capacity,
                "native runtime allocation failed",
            )
        })?;
        // SAFETY: interrupt's allocation is retained by RawContext until native teardown.
        // The callback reads only this AtomicBool; no engine pointer crosses threads. The
        // runtime outlives the bridge context: RawContext::close frees the context first.
        let ptr = unsafe {
            ffi::jbq_context_new(
                runtime.as_raw().as_ptr().cast(),
                self.config.max_memory_bytes.unwrap_or(usize::MAX),
                self.config.max_stack_bytes,
                Some(interrupted),
                Arc::as_ptr(&interrupt).cast_mut().cast(),
            )
        };
        // SAFETY: a fresh context on this owner thread; null is rejected below.
        let ptr = if !ptr.is_null() && unsafe { !compiler::install(ptr, std::ptr::null_mut()) } {
            // SAFETY: the fresh context has no other owner yet.
            unsafe { ffi::jbq_context_free(ptr) };
            std::ptr::null_mut()
        } else {
            ptr
        };
        let pointer = NonNull::new(ptr).ok_or_else(|| {
            FailureReport::new(
                FailureClass::Engine,
                FailureCode::Capacity,
                "native context allocation failed",
            )
        })?;
        // SAFETY: jbq_js_context returns a context of `runtime` with one new reference,
        // which the rquickjs Context owns and releases on drop.
        let context = unsafe {
            rquickjs_core::Context::from_raw(
                NonNull::new_unchecked(ffi::jbq_js_context(pointer.as_ptr()).cast()),
                runtime.clone(),
            )
        };
        let raw = Rc::new(RawContext {
            pointer: Cell::new(Some(pointer)),
            // SAFETY: `pointer` is the live bridge context created above.
            oom: unsafe { ffi::jbq_oom_flag(pointer.as_ptr()) },
            rquickjs: RefCell::new(Some((context, runtime))),
            interrupt,
            pending: RefCell::new(None),
            finalizing: Cell::new(0),
            jobs: RefCell::new(VecDeque::new()),
            max_jobs: self.config.max_pending_jobs,
            lost_job: Cell::new(false),
            atoms: RefCell::new(HashMap::default()),
        });
        // SAFETY: the sink reads this RawContext's allocation, which outlives the native
        // runtime: RawContext::close frees the runtime, and jbq_context_free clears the sink
        // before anything else.
        unsafe {
            ffi::jbq_set_job_sink(ptr, Some(take_job), Rc::as_ptr(&raw).cast_mut().cast());
        }
        let debug = debug::QuickJsDebug::new(id, Rc::downgrade(&raw));
        // SAFETY: ContextState retains this backend until hooks and native context are closed.
        unsafe {
            compiler::install(ptr, Rc::as_ptr(&debug).cast_mut().cast());
        }
        self.contexts.borrow_mut().insert(
            id,
            Rc::new(ContextState {
                raw,
                debug,
                values,
                limits,
                modules: RefCell::new(HashMap::new()),
                module_attempts: Cell::new(0),
                host_values: RefCell::new(HashMap::new()),
            }),
        );
        Ok(id)
    }
    fn close_context(&self, id: ContextId) -> Result<(), FailureReport> {
        self.owner()?;
        if id.engine_epoch() != self.identity.epoch() {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign QuickJS context",
            ));
        }
        if self.closed.borrow().contains(&id) {
            return Ok(());
        }
        let state = self.context(id)?;
        let result = state.close();
        if state.values.is_closed() {
            self.recent.borrow_mut().take();
            self.contexts.borrow_mut().remove(&id);
            self.closed.borrow_mut().insert(id);
        }
        result
    }
    fn debug_backend(&self, id: ContextId) -> Result<Option<Rc<dyn DebugBackend>>, FailureReport> {
        let state = self.context(id)?;
        state.raw.ptr()?;
        Ok(Some(state.debug.clone()))
    }
    fn with_scope(
        &self,
        id: ContextId,
        host: &dyn HostServices,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        let state = self.context(id)?;
        state.raw.ptr()?;
        flush(&state)?;
        let result = callbacks::contain(|| {
            state.values.with_scope(|values| {
                let entry = NativeEntry { host };
                let _guard = EntryGuard::new(&state.raw, &entry)?;
                body.run(&Scope {
                    state: &state,
                    values,
                    host,
                })
            })
        })
        .unwrap_or_else(|_| {
            Err(FailureReport::new(
                FailureClass::Host,
                FailureCode::HostPanic,
                "QuickJS owner entry panicked",
            ))
        });
        let cleanup = flush(&state);
        let late = state.raw.take_fault().or_else(|| cleanup.err());
        match (result, late) {
            (Err(first), Some(late))
                if first.class == FailureClass::Script && late.class != FailureClass::Script =>
            {
                Err(late)
            }
            (Err(first), _) => Err(first.bounded()),
            (Ok(()), Some(error)) => Err(error),
            (Ok(()), None) => Ok(()),
        }
    }
}
fn flush(state: &ContextState) -> Result<(), FailureReport> {
    match state.values.flush_releases() {
        Err(error) if error.code == FailureCode::ActiveContext => Ok(()),
        other => other,
    }
}
/// One synchronous entry's borrowed host services; never retained beyond its guard.
struct NativeEntry<'a> {
    host: &'a dyn HostServices,
}
struct EntryGuard<'a> {
    raw: &'a RawContext,
    previous: *mut c_void,
}
impl<'a> EntryGuard<'a> {
    fn new(raw: &'a RawContext, entry: &NativeEntry<'_>) -> Result<Self, FailureReport> {
        let ptr = raw.ptr()?;
        // SAFETY: NativeEntry remains live on the stack until this guard drops;
        // nested entries restore the previous borrowed frame before returning.
        let previous = unsafe {
            let previous = ffi::jbq_get_entry(ptr);
            ffi::jbq_set_entry(ptr, (entry as *const NativeEntry<'_>).cast_mut().cast());
            previous
        };
        Ok(Self { raw, previous })
    }
}
impl Drop for EntryGuard<'_> {
    fn drop(&mut self) {
        if let Some(ptr) = self.raw.pointer.get() {
            // SAFETY: An active ValueStore scope prevents context close. This restores
            // only the prior synchronous frame; the pointer is never dereferenced here.
            unsafe {
                ffi::jbq_set_entry(ptr.as_ptr(), self.previous);
            }
        }
    }
}
enum HostValueState {
    Building,
    Ready(Rc<NativeValue>),
}
struct HostValueEntry {
    // Retention prevents address reuse while this identity remains cached.
    _definition: Rc<HostValueDefinition>,
    state: HostValueState,
}
struct ContextState {
    raw: Rc<RawContext>,
    debug: Rc<debug::QuickJsDebug>,
    values: ValueStore<NativeValue>,
    limits: ContextLimits,
    modules: RefCell<HashMap<u64, Rc<modules::ModuleRecord>>>,
    module_attempts: Cell<usize>,
    host_values: RefCell<HashMap<usize, HostValueEntry>>,
}
impl ContextState {
    fn close(&self) -> Result<(), FailureReport> {
        if self.raw.finalizing.get() != 0 {
            return Err(FailureReport::fault(
                FailureCode::ActiveContext,
                "native finalizer is active",
            ));
        }
        self.values.close()?;
        self.modules.borrow_mut().clear();
        self.debug.close();
        let host_values = std::mem::take(&mut *self.host_values.borrow_mut());
        drop(host_values);
        self.raw.close();
        self.raw.take_fault().map_or(Ok(()), Err)
    }
}
impl Drop for ContextState {
    fn drop(&mut self) {
        let _ = self.values.close();
        self.modules.get_mut().clear();
        self.debug.close();
        self.host_values.get_mut().clear();
        self.raw.close();
    }
}
struct RawContext {
    pointer: Cell<Option<NonNull<ffi::Context>>>,
    /// The address of the runtime's sticky out-of-memory flag (`jbq_oom_flag`); only read
    /// while `pointer` is set.
    oom: *const u8,
    /// The rquickjs context (a second reference to the bridge's `JSContext`) and the runtime
    /// both live on; released after the bridge context, the runtime last.
    rquickjs: RefCell<Option<(rquickjs_core::Context, rquickjs_core::Runtime)>>,
    interrupt: Arc<AtomicBool>,
    pending: RefCell<Option<FailureReport>>,
    finalizing: Cell<usize>,
    /// Every promise and microtask job the runtime created and nothing has run yet, oldest
    /// first. The adapter owns them: each leaves through `jbq_run_job` or `jbq_drop_job`.
    jobs: RefCell<VecDeque<NonNull<ffi::Job>>>,
    max_jobs: usize,
    /// A job was refused: some continuation will never run, so no entry may run again.
    lost_job: Cell<bool>,
    /// Atoms of string property names, by their code units, each holding one atom reference
    /// released in [`Self::close`].
    ///
    /// Taken from rquickjs's predefined atoms: a name such as `then` was turned into a new
    /// string and an atom lookup on every `get`/`set`. Decision: a per-context cache bounded
    /// by [`MAX_CACHED_ATOMS`] and [`MAX_CACHED_ATOM_UNITS`], so dynamic keys cannot grow it
    /// without bound; names past either limit take a temporary atom per access.
    ///
    /// Fix: each lookup hashed the name with the standard library's SipHash, several times
    /// the cost of the lookup's own work for names of a few letters. Decision: the map
    /// uses [`NameHasher`]. Its weakness to chosen collisions is bounded: the cache holds at
    /// most [`MAX_CACHED_ATOMS`] names, so a lookup never walks more than that.
    atoms: RefCell<HashMap<Vec<u16>, u32, std::hash::BuildHasherDefault<NameHasher>>>,
}
/// A multiply-rotate hash over eight bytes at a time, for [`RawContext::atoms`].
#[derive(Default)]
struct NameHasher(u64);
impl std::hash::Hasher for NameHasher {
    /// The hash of the bytes written so far.
    ///
    /// # Returns
    /// The hash.
    fn finish(&self) -> u64 {
        self.0
    }
    /// Mixes `bytes` into the hash.
    ///
    /// # Arguments
    /// * `bytes` - The next bytes of the key.
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(word))
                .wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
}
/// The most atoms one context caches; see [`RawContext::atoms`].
const MAX_CACHED_ATOMS: usize = 1024;
/// The longest property name, in code units, an atom is cached for; see [`RawContext::atoms`].
const MAX_CACHED_ATOM_UNITS: usize = 64;
impl RawContext {
    /// Whether the runtime's sticky out-of-memory flag is set.
    ///
    /// Fix: after every engine call the adapter called `jbq_take_oom` to learn that nothing
    /// had failed. Decision: the flag is read through its address; `jbq_take_oom` is called
    /// only when it is set, to take and clear it.
    ///
    /// # Returns
    /// `true` while an allocation failure is latched; `false` once the context is closed.
    fn oom_latched(&self) -> bool {
        // SAFETY: `oom` points into the runtime, which lives while `pointer` is set; this
        // owner thread is the only one that writes the byte.
        self.pointer.get().is_some() && unsafe { *self.oom } != 0
    }
    fn ptr(&self) -> Result<*mut ffi::Context, FailureReport> {
        if self.finalizing.get() != 0 {
            return Err(FailureReport::fault(
                FailureCode::ActiveContext,
                "native finalizer cannot enter JavaScript",
            ));
        }
        if self.lost_job.get() {
            return Err(self.jobs_full());
        }
        self.pointer.get().map(NonNull::as_ptr).ok_or_else(|| {
            FailureReport::fault(FailureCode::ClosedContext, "native context closed")
        })
    }
    fn jobs_full(&self) -> FailureReport {
        FailureReport::new(
            FailureClass::Engine,
            FailureCode::Capacity,
            &format!("promise job queue full ({} jobs)", self.max_jobs),
        )
    }
    /// The oldest queued job, now owned by the caller.
    fn next_job(&self) -> Option<NonNull<ffi::Job>> {
        self.jobs.borrow_mut().pop_front()
    }
    fn has_jobs(&self) -> bool {
        !self.jobs.borrow().is_empty()
    }
    fn record_fault(&self, error: FailureReport) {
        self.pending.borrow_mut().get_or_insert(error.bounded());
    }
    fn take_fault(&self) -> Option<FailureReport> {
        self.pending.borrow_mut().take()
    }
    fn close(&self) {
        if let Some(pointer) = self.pointer.take() {
            // Queued jobs hold values of this runtime: release them while it is alive. A
            // release can run finalizers that queue more jobs; those are released too.
            while let Some(job) = self.next_job() {
                // SAFETY: the job left the queue here and is released exactly once, on the
                // owner thread, before the runtime it belongs to.
                unsafe { ffi::jbq_drop_job(pointer.as_ptr(), job.as_ptr()) };
            }
            // Cached atoms hold references in this runtime's atom table: release them while
            // it is alive.
            for (_, atom) in self.atoms.take() {
                // SAFETY: each cached atom owns exactly one reference, released once here.
                unsafe { ffi::jbq_atom_free(pointer.as_ptr(), atom) };
            }
            // SAFETY: Owner teardown first invalidates/drops ValueStore roots. Marking
            // the pointer absent prevents callbacks/finalizers from reentering it.
            unsafe {
                ffi::jbq_context_free(pointer.as_ptr());
            }
            // JS_FreeRuntime requires every context gone: the rquickjs context reference
            // goes first, then the runtime.
            if let Some((context, runtime)) = self.rquickjs.take() {
                drop(context);
                drop(runtime);
            }
        }
    }
}
/// The job sink: takes ownership of each job the runtime creates. A job past the capacity is
/// refused and so lost; the context records a `Capacity` fault, stops the running JavaScript
/// and refuses every later entry.
unsafe extern "C" fn take_job(opaque: *mut c_void, job: *mut ffi::Job) -> std::ffi::c_int {
    // SAFETY: the sink's opaque is the RawContext installed in create_context, which
    // outlives the runtime; only its cells are touched, never across JavaScript.
    let raw = unsafe { &*opaque.cast::<RawContext>() };
    if let (Some(job), Ok(mut jobs)) = (NonNull::new(job), raw.jobs.try_borrow_mut())
        && jobs.len() < raw.max_jobs
        && jobs.try_reserve(1).is_ok()
    {
        jobs.push_back(job);
        return 0;
    }
    if !raw.lost_job.replace(true) {
        raw.record_fault(raw.jobs_full());
    }
    raw.interrupt.store(true, Ordering::Release);
    -1
}
impl Drop for RawContext {
    fn drop(&mut self) {
        self.close();
    }
}
struct NativeValue {
    value: ffi::Value,
    raw: Rc<RawContext>,
    resolver: bool,
    settled: Cell<bool>,
}
impl NativeValue {
    fn new(value: ffi::Value, raw: Rc<RawContext>) -> Self {
        Self {
            value,
            raw,
            resolver: false,
            settled: Cell::new(false),
        }
    }
}
impl Drop for NativeValue {
    fn drop(&mut self) {
        // Fix: numbers, booleans, null and undefined were released with a call into the
        // shim that did nothing. Decision: only reference-counted values are released.
        if !self.value.has_ref() {
            return;
        }
        if let Some(pointer) = self.raw.pointer.get() {
            // SAFETY: This wrapper owns exactly one native reference and releases
            // it only on the owner thread while the independent raw context is live.
            unsafe {
                ffi::jbq_free(pointer.as_ptr(), self.value);
            }
        }
    }
}
unsafe extern "C" fn interrupted(opaque: *mut c_void) -> std::ffi::c_int {
    // SAFETY: The callback's opaque points at the Arc allocation retained by
    // RawContext for the entire runtime lifetime. Only AtomicBool is accessed.
    i32::from(unsafe { &*opaque.cast::<AtomicBool>() }.load(Ordering::Acquire))
}

#[cfg(test)]
#[path = "../../../tests/unit/quickjs_ng_lifecycle_coverage.rs"]
mod tests;

// lunarstorm patch: jetbunny ran its public acceptance tests (quickjs_ng_data, _vm,
// _module_vm, _modules, _jbn) through this build here. They drive jetbunny's
// `VirtualMachine`, which Lunarstorm does not take, and are archived in
// `.archive/2026-11-24/runtime/debug/tests/`.

/// Factory registered by applications that compile in the native QuickJS adapter.
#[derive(Default)]
pub struct QuickJsNgFactory;
impl crate::engines::EngineFactory for QuickJsNgFactory {
    fn id(&self) -> &str {
        "quickjs-ng"
    }
    fn create(&self) -> Result<Box<dyn JsEngine>, FailureReport> {
        Ok(Box::new(QuickJsNgEngine::new()?))
    }
    fn create_with(
        &self,
        limits: &crate::engines::EngineLimits,
    ) -> Result<Box<dyn JsEngine>, FailureReport> {
        let default = QuickJsNgConfig::default();
        Ok(Box::new(QuickJsNgEngine::with_config(QuickJsNgConfig {
            max_memory_bytes: limits.heap_bytes,
            max_stack_bytes: limits.stack_bytes.unwrap_or(default.max_stack_bytes),
            max_pending_jobs: limits.pending_jobs.unwrap_or(default.max_pending_jobs),
        })?))
    }
}


#[cfg(test)]
mod debug_tests;
