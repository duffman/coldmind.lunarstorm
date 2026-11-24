//! lunarstorm patch: jetbunny's QuickJS debug backend on a context another owner
//! created.
//!
//! jetbunny's engine creates every context itself (`jbq_context_new`) and keeps its
//! `RawContext` and `QuickJsDebug` in a `ContextState`. A Lunarstorm Runtime's
//! context is created by rquickjs, which keeps the runtime's interrupt handler,
//! rejection tracker, module loader and job queue. [`Debugger::wrap`] builds the
//! same `RawContext` and `QuickJsDebug` around that context with
//! `jbq_context_wrap`, and a [`DebugCoordinator`] over them, so the backend runs
//! unchanged.
//!
//! What jetbunny's runtime hooks did, the owner now does by calling in:
//! - its interrupt handler calls [`Debugger::poll`] (pause requests, evaluation
//!   deadlines);
//! - its rejection tracker calls [`Debugger::rejection`];
//! - its compile path calls [`Debugger::register`] and [`Debugger::loaded`] (what
//!   jetbunny's Rust compiler callback did) and compiles with
//!   [`Debugger::EVAL_FLAG`] so the engine emits statement markers;
//! - its loop calls [`Debugger::before_entry`], [`Debugger::service`] and
//!   [`Debugger::checkpoint`] where jetbunny's `EngineBridge` did.

use super::{RawContext, debug::QuickJsDebug, ffi};
use crate::debug::{
    DebugBackend, DebugCoordinator, DebugError, DebugExecutionState, DebugLimits, DebugSessionOptions, ScriptKind,
    ScriptRegistration, VmDebugHandle,
};
use crate::engine::{
    ContextLimits, EngineIdentity, FailureClass, FailureCode, FailureReport, JsString, ScriptSource, SourceMapAttachment,
    ValueStore,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::num::NonZeroU64;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The debugger of one context, owned by that context's thread.
pub struct Debugger {
    /// Protocol coordinator over the backend; serviced by the owner's loop.
    coordinator: Rc<DebugCoordinator>,
    /// jetbunny's backend: hooks, breakpoints, stepping, inspection.
    backend: Rc<QuickJsDebug>,
    /// The bridge context around the owner's `JSContext`.
    raw: Rc<RawContext>,
    /// Gives the context its identity; holds no values (the backend roots its own).
    _values: ValueStore<super::NativeValue>,
    /// Identity of this owner's engine instance.
    _identity: EngineIdentity,
}

impl Debugger {
    /// `JS_EVAL_FLAG_DEBUG`: compile with statement markers and debugger-reachable
    /// variables. The owner adds it to every compile while [`Debugger::compiling`].
    pub const EVAL_FLAG: i32 = 1 << 8;

    /// Builds the debugger around a live context.
    ///
    /// # Arguments
    /// * `js_context` - the owner's `JSContext`; one reference is taken until drop.
    /// * `limits` - protocol and inspection bounds.
    /// * `wake_owner` - wakes the owner's loop from any thread so it services
    ///   debugger requests; must not block.
    /// * `vm_epoch` - identity of the owner's run, distinct per Runtime.
    ///
    /// # Returns
    /// The debugger, initialized and ready to [`Debugger::attach`].
    ///
    /// # Errors
    /// Bridge allocation failed or the limits are invalid.
    ///
    /// # Safety
    /// `js_context` is live, belongs to the calling thread and outlives the returned
    /// value; the owner calls this value's methods only on that thread.
    pub unsafe fn wrap(
        js_context: NonNull<c_void>,
        limits: DebugLimits,
        wake_owner: Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync>,
        vm_epoch: NonZeroU64,
    ) -> Result<Self, FailureReport> {
        let identity = EngineIdentity::new()?;
        let values = ValueStore::new(&identity, ContextLimits::default())?;
        let context = values.context();
        // SAFETY: the caller guarantees a live owner-thread context.
        let pointer = NonNull::new(unsafe { ffi::jbq_context_wrap(js_context.as_ptr()) }).ok_or_else(|| {
            FailureReport::new(FailureClass::Engine, FailureCode::Capacity, "debugger bridge allocation failed")
        })?;
        let raw = Rc::new(RawContext {
            pointer: Cell::new(Some(pointer)),
            // SAFETY: `pointer` is the live bridge context created above.
            oom: unsafe { ffi::jbq_oom_flag(pointer.as_ptr()) },
            // The owner keeps its own rquickjs context and runtime.
            rquickjs: RefCell::new(None),
            interrupt: Arc::new(AtomicBool::new(false)),
            pending: RefCell::new(None),
            finalizing: Cell::new(0),
            // No job sink is installed: jobs stay in the owner's runtime queue.
            jobs: RefCell::new(VecDeque::new()),
            max_jobs: usize::MAX,
            lost_job: Cell::new(false),
            atoms: RefCell::new(HashMap::default()),
        });
        let backend = QuickJsDebug::new(context, Rc::downgrade(&raw));
        let coordinator = DebugCoordinator::new(backend.clone(), context, limits, wake_owner, vm_epoch)
            .map_err(super::debug::failure)?;
        Ok(Self { coordinator, backend, raw, _values: values, _identity: identity })
    }

    /// Opens a debugger session; the protocol side drives the returned handle.
    ///
    /// # Arguments
    /// * `options` - whether to pause before the first application entry.
    ///
    /// # Returns
    /// The session controller, usable from any thread.
    ///
    /// # Errors
    /// A session is already open or the debugger is closed.
    pub fn attach(&self, options: DebugSessionOptions) -> Result<VmDebugHandle, DebugError> {
        self.coordinator.attach(options)
    }

    /// # Returns
    /// Whether compiles must carry [`Debugger::EVAL_FLAG`] and be registered.
    pub fn compiling(&self) -> bool {
        self.backend.is_development()
    }

    /// Registers a source before it is compiled, as jetbunny's compiler callback did.
    ///
    /// Decision: an absolute path name is registered as its `file://` URI, the form
    /// the DAP side turns client paths into, so breakpoints set on the file match.
    /// A source with a map is registered as `<file URI>.js`, mapped back to the
    /// file: the client's breakpoints and stack frames then land in the original.
    ///
    /// # Arguments
    /// * `text` - the source text.
    /// * `name` - the name the engine records (module name or file).
    /// * `module` - whether it compiles as a module.
    /// * `source_map` - Source Map v3 JSON from `text` to the file `name` names.
    ///
    /// # Returns
    /// The registration to hand to [`Debugger::loaded`]; `None` while not debugging.
    ///
    /// # Errors
    /// The protocol side refused the source (limits, or an invalid map).
    pub fn register(
        &self,
        text: &str,
        name: &str,
        module: bool,
        source_map: Option<String>,
    ) -> Result<Option<ScriptRegistration>, DebugError> {
        let kind = if module { ScriptKind::Module } else { ScriptKind::Script };
        let origin = if name.starts_with('/') {
            url::Url::from_file_path(name).map_or_else(|()| name.to_owned(), String::from)
        } else {
            name.to_owned()
        };
        let units: Vec<u16> = text.encode_utf16().collect();
        match source_map {
            Some(json) => self.backend.register_host_source(&ScriptSource {
                text: JsString::from_units(units),
                origin: format!("{origin}.js"),
                source_map: Some(SourceMapAttachment {
                    json: json.into_bytes(),
                    originals: Vec::new(),
                    preceding_maps: Vec::new(),
                }),
            }, kind),
            // jetbunny's path: debugger evaluations and staged host sources too.
            None => self.backend.register_source(&units, &origin, kind),
        }
    }

    /// Records the compiled root of a registered source: its statement markers
    /// become breakpoint locations.
    ///
    /// # Arguments
    /// * `root` - the compiled module or function, as the engine's `JSValue`.
    /// * `registration` - what [`Debugger::register`] returned.
    ///
    /// # Errors
    /// The engine could not enumerate markers.
    ///
    /// # Safety
    /// `root` points at a live `JSValue` of this context; it is borrowed.
    pub unsafe fn loaded(&self, root: *const c_void, registration: Option<ScriptRegistration>) -> Result<(), DebugError> {
        // SAFETY: the bridge packs a `JSValue` into these words byte for byte, and the
        // caller lends a live one.
        let value = unsafe { std::ptr::read_unaligned(root.cast::<ffi::Value>()) };
        self.backend.loaded(value, registration)
    }

    /// Before each JS entry: holds the entry while a break-on-start pause is pending.
    ///
    /// # Errors
    /// The session failed.
    pub fn before_entry(&self) -> Result<(), DebugError> {
        self.coordinator.before_entry()
    }

    /// At a loop boundary: services pause, resume and inspection requests.
    ///
    /// # Errors
    /// The session failed.
    pub fn service(&self) -> Result<(), DebugError> {
        self.coordinator.service()
    }

    /// After the owner drained its microtask queue: qualifies unhandled rejections.
    ///
    /// # Arguments
    /// * `drained` - whether the queue is empty.
    ///
    /// # Errors
    /// The session failed.
    pub fn checkpoint(&self, drained: bool) -> Result<(), DebugError> {
        self.backend.checkpoint(drained)
    }

    /// The debugger's share of the interrupt poll; called by the owner's interrupt
    /// handler.
    ///
    /// # Returns
    /// `true` to stop the running JavaScript.
    pub fn poll(&self) -> bool {
        if self.raw.interrupt.load(Ordering::Acquire) {
            return true;
        }
        match self.raw.pointer.get() {
            // SAFETY: the bridge context is live while `pointer` is set; owner thread.
            Some(pointer) => unsafe { ffi::jbq_debug_poll(pointer.as_ptr()) != 0 },
            None => false,
        }
    }

    /// Forwards a rejection-tracker event of the owner's runtime.
    ///
    /// # Arguments
    /// * `promise`, `reason` - the engine's `JSValue`s, borrowed.
    /// * `handled` - a handler was attached to a promise reported earlier.
    ///
    /// # Safety
    /// Both point at live `JSValue`s of this context.
    pub unsafe fn rejection(&self, promise: *const c_void, reason: *const c_void, handled: bool) {
        if let Some(pointer) = self.raw.pointer.get() {
            // SAFETY: live bridge context; the caller lends live values.
            unsafe { ffi::jbq_debug_rejection(pointer.as_ptr(), promise, reason, i32::from(handled)) };
        }
    }

    /// # Returns
    /// Whether the client asked the program to stop.
    pub fn is_stopping(&self) -> bool {
        self.coordinator.is_stopping()
    }

    /// # Returns
    /// The session's execution state.
    pub fn state(&self) -> DebugExecutionState {
        self.coordinator.state()
    }

    /// # Returns
    /// Whether requests wait for [`Debugger::service`].
    pub fn pending(&self) -> bool {
        self.coordinator.pending()
    }
}

impl Drop for Debugger {
    /// Ends the session, removes the hooks and releases the bridge context; the
    /// owner's context and runtime stay.
    fn drop(&mut self) {
        let _ = self.coordinator.shutdown();
        self.backend.close();
        self.raw.close();
    }
}
