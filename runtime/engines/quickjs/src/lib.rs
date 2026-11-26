//! VM adapter for QuickJS (quickjs-ng), through the `rquickjs` binding vendored in
//! `runtime/engines/rquickjs`.
//!
//! Mapping of the contract onto the engine:
//!
//! | contract            | QuickJS                                                  |
//! |---------------------|----------------------------------------------------------|
//! | `Instance`          | `JSRuntime` (`rquickjs::Runtime`)                         |
//! | `Context`           | `JSContext` with the full standard library                |
//! | `Scope<'s>`         | `rquickjs::Ctx<'s>`                                       |
//! | `Value<'s>`         | `rquickjs::Value<'s>` (a reference-counted `JSValue`)     |
//! | `Persistent`        | `rquickjs::Persistent<Value>`                             |
//! | native function     | a `JSCFunctionData` holding the closure                   |
//! | host object         | an instance of one opaque class owning a `Box<dyn Any>`   |
//!
//! Performance decisions:
//! - Every trait method is a thin `#[inline]` wrapper; with the Runtime
//!   monomorphized over [`QuickJs`] the abstraction compiles away.
//! - [`Scope::call`] calls `JS_Call` directly with the arguments in a stack array.
//!   The binding's own argument builder moves to the heap above four arguments.
//! - Native function arguments are collected into a stack array of
//!   [`INLINE_ARGS`] slots and passed as a slice; only longer calls allocate.
//! - Strings and `Uint8Array`s are lent to the caller in place.
//!
//! Limits of this engine, reported by [`QuickJs::capabilities`]:
//! - Modules load synchronously: `Load::Later` is refused.
//! - No JIT and no snapshots. Module bytecode is written and loaded
//!   (`bytecode_cache`); the debugger is jetbunny's, from `lunarstorm-debug`
//!   (`debug_hooks`; [`Instance::debug`] and the `debug_*` hooks).

use lunarstorm_vm::{
    Call, Callbacks, Capabilities, ClassSpec, Context, Descriptor, ErrorKind, Heap, Instance, Interrupt, Kind, Limits, Load,
    ModuleSource, Outcome, Scope, Thrown, Vm, CONTRACT_VERSION,
};
use rquickjs::class::{JsClass, Readable, Trace, Tracer};

// lunarstorm patch: jetbunny leaves its own engine additions unbound in rquickjs-sys
// (its binding files are upstream's) and declares the ones it calls next to the
// caller; this is the one this adapter calls (`quickjs.h`, jetbunny-sticky-oom-v1).
unsafe extern "C" {
    /// Takes the runtime's sticky out-of-memory flag.
    ///
    /// # Arguments
    /// * `rt` - the runtime, on its owner thread.
    ///
    /// # Returns
    /// `true` once after `JS_ThrowOutOfMemory`, then `false` until the next one.
    fn JS_JetbunnyTakeOutOfMemory(rt: *mut qjs::JSRuntime) -> bool;

    /// Creates a symbol only C code can see (`quickjs.h`): properties keyed by it
    /// are absent from every JS reflection API. Upstream QuickJS-ng API that the
    /// vendored binding files do not bind.
    ///
    /// # Arguments
    /// * `ctx` - the context.
    /// * `description` - NUL-terminated description.
    ///
    /// # Returns
    /// The symbol, or the exception sentinel when allocation fails.
    fn JS_NewPrivateSymbol(ctx: *mut qjs::JSContext, description: *const std::ffi::c_char) -> qjs::JSValue;
}
use rquickjs::function::{Constructor, IntoJsFunc, ParamRequirement, Params};
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::{Declarations, Declared, ModuleDef};
use rquickjs::runtime::RejectionTracker;
use rquickjs::object::Property;
use rquickjs::{
    qjs, Array, Atom, Class, Ctx, Error, Exception, Function, JsLifetime, Module, Object, Persistent,
    String as JsString, Type, TypedArray, Value, WriteOptions,
};
use lunarstorm_debug::debug::{DebugLimits, DebugSessionOptions, VmDebugHandle};
use lunarstorm_debug::engines::quickjs_ng::Debugger;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::num::NonZeroU64;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Arguments a call can carry, in either direction, without a heap allocation.
const INLINE_ARGS: usize = 8;

/// The QuickJS engine. Use as the `V` parameter of the Runtime.
pub struct QuickJs;

impl Vm for QuickJs {
    type Instance = QuickInstance;
    type Context = QuickContext;
    type Scope<'s> = Ctx<'s>;
    type Value<'s> = Value<'s>;
    type Persistent = Persistent<Value<'static>>;

    /// # Returns
    /// The adapter's identity.
    ///
    /// Decision: the engine version names the vendored engine source
    /// (`rquickjs/sys/quickjs/REVISION`), not the binding, because packaged
    /// bytecode is only valid for the engine that wrote it. Bump the `lunarstorm.N`
    /// suffix whenever an edit to the vendored engine changes its bytecode or
    /// opcodes, so old packages are refused instead of misread.
    fn descriptor() -> Descriptor {
        Descriptor {
            adapter: "quickjs",
            engine: "quickjs-ng",
            engine_version: "0.17.0+lunarstorm.1",
            contract: CONTRACT_VERSION,
        }
    }

    /// # Returns
    /// Memory limits and termination; see the module docs for what is absent.
    fn capabilities() -> Capabilities {
        Capabilities {
            memory_limits: true,
            termination: true,
            bytecode_cache: true,
            debug_hooks: true,
            ..Capabilities::default()
        }
    }
}

/// The bridge wrapper of every context of an instance, keyed by the context's
/// `JSContext` address; shared with the engine hooks (interrupt, rejection tracker)
/// and, through context userdata, with compiles.
///
/// Decision: each context is wrapped when it is created, because the wrapper is
/// what makes jetbunny's Rust compiler the context's compiler
/// ([`Debugger::wrap`]). A debugger session is opened on a wrapper later, by
/// [`Instance::debug`]. The list sits in a `RefCell` so [`QuickInstance`]'s drop can
/// empty it and release the context references before the runtime goes.
#[derive(Clone, Default)]
struct DebugSlot(Rc<RefCell<Vec<(usize, Debugger)>>>);

impl DebugSlot {
    /// Runs `f` on the wrapper of one context.
    ///
    /// # Arguments
    /// * `context` - the `JSContext` address.
    /// * `f` - what to do with the wrapper.
    ///
    /// # Returns
    /// `f`'s result, or `None` for a context this instance did not create.
    fn with<R>(&self, context: usize, f: impl FnOnce(&Debugger) -> R) -> Option<R> {
        self.0.borrow().iter().find(|(key, _)| *key == context).map(|(_, debugger)| f(debugger))
    }

    /// Runs `f` on the wrapper a debugger session is open on.
    ///
    /// # Arguments
    /// * `f` - what to do with the wrapper.
    ///
    /// # Returns
    /// `f`'s result, or `None` while no session is open.
    fn attached<R>(&self, f: impl FnOnce(&Debugger) -> R) -> Option<R> {
        self.0.borrow().iter().find(|(_, debugger)| debugger.compiling()).map(|(_, debugger)| f(debugger))
    }
}

/// Wakes the owner's loop for the debugger; empty until [`Instance::debug`] supplies
/// the owner's waker, because the wrappers exist before any session does.
type Waker = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

// SAFETY: `DebugSlot` holds no JS values and no `'js` lifetime.
unsafe impl<'js> JsLifetime<'js> for DebugSlot {
    type Changed<'to> = DebugSlot;
}

/// Identity of each debugged run, distinct per instance in this process.
static DEBUG_EPOCH: AtomicU64 = AtomicU64::new(1);

/// A QuickJS runtime: heap, GC, job queue, limits and interrupt.
pub struct QuickInstance {
    /// The contexts' bridge wrappers. Emptied first in `drop`.
    debugger: DebugSlot,
    /// The owner's waker, handed to every wrapper; set by [`Instance::debug`].
    waker: Waker,
    /// The engine runtime.
    runtime: rquickjs::Runtime,
    /// Shared with the engine's interrupt hook.
    interrupt: Interrupt,
    /// The memory limit that was set, for [`Instance::heap`].
    memory: Option<usize>,
}

impl Instance<QuickJs> for QuickInstance {
    /// Creates the runtime and installs the owner's callbacks as the engine's module
    /// loader, rejection tracker and interrupt handler.
    ///
    /// # Arguments
    /// * `limits` - memory and stack limits.
    /// * `callbacks` - the owner's callbacks.
    ///
    /// # Returns
    /// The instance.
    ///
    /// # Errors
    /// The engine's message when the runtime cannot be allocated.
    fn new<C: Callbacks<QuickJs>>(limits: &Limits, callbacks: C) -> Result<Self, String> {
        let runtime = rquickjs::Runtime::new().map_err(|error| error.to_string())?;
        if let Some(bytes) = limits.memory {
            runtime.set_memory_limit(bytes);
        }
        if let Some(bytes) = limits.stack {
            runtime.set_max_stack_size(bytes);
        }
        // Decision: applied only when the owner asks; `None` keeps the engine's
        // default (256 KiB, growing 1.5x after each collection), so existing runtimes
        // are unchanged. Adopted from `quickjs_es_runtime`'s builder.
        if let Some(bytes) = limits.gc_threshold {
            runtime.set_gc_threshold(bytes);
        }
        // The three engine hooks share the callbacks; they take `&self`, so a plain
        // `Rc` is enough and a rejection reported during a module load cannot
        // conflict with the load in progress.
        let callbacks = Rc::new(callbacks);
        let debugger = DebugSlot::default();
        runtime.set_loader(Resolve(callbacks.clone()), Loading(callbacks.clone()));
        let tracker = callbacks.clone();
        let slot = debugger.clone();
        let tracker: RejectionTracker = Box::new(move |ctx, promise, reason, handled| {
            let (raw_promise, raw_reason) = (promise.as_raw(), reason.as_raw());
            slot.with(ctx.as_raw().as_ptr() as usize, |debugger| {
                // SAFETY: both values are live for this call of the tracker.
                unsafe {
                    debugger.rejection(
                        (&raw_promise as *const qjs::JSValue).cast(),
                        (&raw_reason as *const qjs::JSValue).cast(),
                        handled,
                    )
                };
            });
            tracker.rejection(&ctx, Scope::identity(&ctx, &promise), reason, handled);
        });
        runtime.set_host_promise_rejection_tracker(Some(tracker));
        let interrupt = Interrupt::default();
        let flag = interrupt.clone();
        let slot = debugger.clone();
        // The debugger's pause requests and evaluation deadlines arrive through the
        // same poll; jetbunny's bridge installed it as its own interrupt handler.
        runtime.set_interrupt_handler(Some(Box::new(move || {
            flag.take() || callbacks.interrupt() || slot.0.borrow().iter().any(|(_, debugger)| debugger.poll())
        })));
        Ok(Self { debugger, waker: Waker::default(), runtime, interrupt, memory: limits.memory })
    }

    /// # Returns
    /// A context with the full standard library, compiling with jetbunny's Rust
    /// compiler.
    ///
    /// # Errors
    /// The engine's message when allocation fails, or the bridge's when the context
    /// cannot be wrapped.
    fn context(&self) -> Result<QuickContext, String> {
        let context = rquickjs::Context::full(&self.runtime).map_err(|error| error.to_string())?;
        let epoch = NonZeroU64::new(DEBUG_EPOCH.fetch_add(1, Ordering::Relaxed)).expect("the counter starts at 1");
        let pointer: NonNull<c_void> = context.as_raw().cast();
        let waker = self.waker.clone();
        // SAFETY: the context is live on this thread; the wrapper holds a reference to
        // it and `QuickInstance::drop` releases every wrapper before the runtime.
        let debugger = unsafe {
            Debugger::wrap(
                pointer,
                DebugLimits::default(),
                Arc::new(move || {
                    if let Some(wake) = &*waker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
                        wake();
                    }
                    Ok(())
                }),
                epoch,
            )
        }
        .map_err(|error| error.to_string())?;
        self.debugger.0.borrow_mut().push((pointer.as_ptr() as usize, debugger));
        // Compiles reach the debugger through the context: `declare` has only a `Ctx`.
        context.with(|ctx| ctx.store_userdata(self.debugger.clone()).map(|_| ()).map_err(|error| error.to_string()))?;
        Ok(QuickContext(context))
    }

    /// Runs a full collection (cycle removal; reference counts free the rest eagerly).
    fn collect_garbage(&self) {
        self.runtime.run_gc();
    }

    /// # Returns
    /// Bytes in use by the engine's allocator and the configured limit.
    fn heap(&self) -> Heap {
        Heap { used: self.runtime.memory_usage().memory_used_size.max(0) as usize, limit: self.memory }
    }

    /// # Returns
    /// The handle the engine's interrupt hook polls.
    fn interrupt(&self) -> Interrupt {
        self.interrupt.clone()
    }

    /// Opens a debugger session on the context's bridge wrapper
    /// (`lunarstorm_debug::engines::quickjs_ng::Debugger`).
    fn debug(
        &self,
        context: &QuickContext,
        wake: Arc<dyn Fn() + Send + Sync>,
        options: DebugSessionOptions,
    ) -> Result<VmDebugHandle, String> {
        if self.debugger.attached(|_| ()).is_some() {
            return Err(String::from("a debugger is already attached"));
        }
        *self.waker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(wake);
        self.debugger
            .with(context.0.as_raw().as_ptr() as usize, |debugger| debugger.attach(options).map_err(|error| error.to_string()))
            .unwrap_or_else(|| Err(String::from("the context does not belong to this instance")))
    }

    fn debug_entry(&self) -> Result<(), String> {
        self.debugger.attached(|debugger| debugger.before_entry().map_err(|error| error.to_string())).unwrap_or(Ok(()))
    }

    fn debug_service(&self) -> Result<bool, String> {
        self.debugger
            .attached(|debugger| {
                debugger.service().map_err(|error| error.to_string())?;
                Ok(debugger.is_stopping())
            })
            .unwrap_or(Ok(false))
    }

    fn debug_checkpoint(&self) -> Result<(), String> {
        self.debugger.attached(|debugger| debugger.checkpoint(true).map_err(|error| error.to_string())).unwrap_or(Ok(()))
    }
}

impl Drop for QuickInstance {
    /// Releases the bridge wrappers, and with them their context references, hooks
    /// and compilers, while the runtime is still alive.
    fn drop(&mut self) {
        let wrappers = std::mem::take(&mut *self.debugger.0.borrow_mut());
        drop(wrappers);
    }
}

/// A QuickJS context: one realm.
pub struct QuickContext(rquickjs::Context);

impl Context<QuickJs> for QuickContext {
    /// Enters the realm.
    ///
    /// # Arguments
    /// * `f` - the code to run with the context handle.
    ///
    /// # Returns
    /// The closure's result.
    #[inline]
    fn enter<R>(&self, f: impl for<'s> FnOnce(Ctx<'s>) -> R) -> R {
        self.0.with(f)
    }
}

/// The engine's resolver hook, forwarding to [`Callbacks::resolve`].
struct Resolve<C>(Rc<C>);

impl<C: Callbacks<QuickJs>> Resolver for Resolve<C> {
    /// # Arguments
    /// * `ctx` - the importing context.
    /// * `base` - the importing module.
    /// * `name` - the specifier.
    /// * `_attributes` - import attributes; not part of the contract.
    ///
    /// # Returns
    /// The canonical module name.
    ///
    /// # Errors
    /// A resolving error carrying the callback's message.
    fn resolve<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        self.0.resolve(ctx, base, name).map_err(|message| Error::new_resolving_message(base, name, message))
    }
}

/// The engine's loader hook, forwarding to [`Callbacks::load`].
struct Loading<C>(Rc<C>);

impl<C: Callbacks<QuickJs>> Loader for Loading<C> {
    /// # Arguments
    /// * `ctx` - the importing context.
    /// * `name` - canonical module name.
    /// * `_attributes` - import attributes; not part of the contract.
    ///
    /// # Returns
    /// The declared module.
    ///
    /// # Errors
    /// A loading error carrying the callback's message, a syntax error, or a
    /// refusal of `Load::Later` (QuickJS needs the module before the hook returns).
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        match self.0.load(ctx, name) {
            Ok(Load::Ready(source)) => declare(ctx, name, source),
            Ok(Load::Later(_)) => Err(Error::new_loading_message(name, "QuickJS loads modules synchronously")),
            Ok(Load::Native) => Module::declare_def::<NativeDef, _>(ctx.clone(), name),
            Err(message) => Err(Error::new_loading_message(name, message)),
        }
    }
}

/// The exports of native modules that were declared by [`Scope::native_module`] and
/// not yet evaluated, by canonical module name.
///
/// Decision: rquickjs's `ModuleDef` is a static trait (no `self`), so the values of
/// one module cannot travel in the definition. They wait here, in the context's
/// userdata, between the callback that built them (`Loader::load`) and the engine's
/// evaluation of the module (`NativeDef::evaluate`), which takes them out. Dropped
/// alternative: building the C module by hand with `JS_NewCModule`, which needs the
/// binding's private `Module::from_ptr` to return the module to the loader.
#[derive(Default)]
struct NativePending(RefCell<HashMap<String, Exports>>);

/// The `(export name, value)` pairs of one native module.
type Exports = Vec<(String, Persistent<Value<'static>>)>;

// SAFETY: `NativePending` holds no scoped handles: persistent handles are `'static`
// and everything else is owned text, so it is the same type under any lifetime.
unsafe impl<'js> JsLifetime<'js> for NativePending {
    type Changed<'to> = NativePending;
}

/// The one native module definition: it declares and fills whatever
/// [`Scope::native_module`] stored under the module's name.
struct NativeDef;

impl ModuleDef for NativeDef {
    /// Declares the export names stored for this module.
    ///
    /// # Arguments
    /// * `decl` - the module being declared.
    ///
    /// # Errors
    /// An error when nothing is stored for the module's name (a `Load::Native` answer
    /// without a prior `native_module` call).
    fn declare<'js>(decl: &Declarations<'js>) -> rquickjs::Result<()> {
        let module = decl.module();
        let name = module.name::<String>()?;
        let ctx = module.ctx();
        let pending = ctx.userdata::<NativePending>().ok_or(Error::Unknown)?;
        let pending = pending.0.borrow();
        let exports = pending.get(&name).ok_or_else(|| Error::new_loading_message(&name, "no native module was declared under this name"))?;
        for (export, _) in exports {
            decl.declare(export.as_str())?;
        }
        Ok(())
    }

    /// Sets the export values; each importer then sees the same values.
    ///
    /// # Arguments
    /// * `ctx` - the context.
    /// * `exports` - the module being evaluated.
    ///
    /// # Errors
    /// An error when the stored values are gone or belong to another instance.
    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> rquickjs::Result<()> {
        let name = exports.module().name::<String>()?;
        let values = {
            let pending = ctx.userdata::<NativePending>().ok_or(Error::Unknown)?;
            let taken = pending.0.borrow_mut().remove(&name);
            taken.ok_or_else(|| Error::new_loading_message(&name, "native module evaluated twice"))?
        };
        for (export, value) in values {
            exports.export(export, value.restore(ctx)?)?;
        }
        Ok(())
    }
}

/// Compiles a module and fills its `import.meta`.
///
/// # Arguments
/// * `ctx` - the context.
/// * `name` - canonical module name.
/// * `source` - source bytes and metadata.
///
/// # Returns
/// The declared, not yet evaluated module.
///
/// # Errors
/// The engine's syntax error.
fn declare<'js>(ctx: &Ctx<'js>, name: &str, source: ModuleSource) -> rquickjs::Result<Module<'js, Declared>> {
    let module = if let Some(image) = &source.image {
        // SAFETY: an `Image` comes only from a verified package or from
        // `Scope::compile` of this same adapter and engine (same `Descriptor`).
        // Decision: packaged bytecode has no statement markers, so it is not
        // registered with a debugger.
        unsafe { Module::load(ctx.clone(), image.bytes())? }
    } else {
        // While a session is open the source is staged, so jetbunny's compiler
        // callback registers it under its file URI and source map; the callback then
        // compiles it with statement markers and hands the root to the debugger.
        let slot = ctx.userdata::<DebugSlot>().map(|slot| slot.clone());
        let staged = slot.as_ref().and_then(|slot| {
            slot.with(ctx.as_raw().as_ptr() as usize, |debugger| {
                if !debugger.compiling() {
                    return Ok(());
                }
                let text = std::str::from_utf8(&source.source)
                    .map_err(|_| Error::new_loading_message(name, "module source is not UTF-8"))?;
                debugger
                    .stage(text, name, true, source.source_map.clone())
                    .map_err(|error| Error::new_loading_message(name, error.to_string()))
            })
        });
        if let Some(staged) = staged {
            staged?;
        }
        Module::declare(ctx.clone(), name, source.source)?
    };
    if let Some(meta) = source.meta {
        let object = module.meta()?;
        let directory = Path::new(&meta.filename).parent().and_then(Path::to_str).unwrap_or("/");
        object.set("url", format!("file://{}", meta.filename))?;
        object.set("dirname", directory)?;
        object.set("filename", meta.filename)?;
        object.set("main", meta.main)?;
    }
    Ok(module)
}

/// The bytes QuickJS lent for a string, without asserting they are UTF-8.
///
/// Decision: the binding's `CString::as_str` builds a `&str` unchecked, which is
/// undefined behaviour for a string with an unpaired surrogate (QuickJS encodes it
/// as WTF-8); reading the pointer and length avoids ever forming that `&str`.
///
/// # Arguments
/// * `text` - the lent string.
///
/// # Returns
/// Its bytes, without the terminating NUL; valid while `text` lives.
#[inline]
fn cstring_bytes<'a>(text: &'a rquickjs::CString<'_>) -> &'a [u8] {
    // SAFETY: `as_ptr` and `len` describe the buffer QuickJS lent, which stays
    // alive until `text` is dropped.
    unsafe { std::slice::from_raw_parts(text.as_ptr().cast::<u8>(), text.len()) }
}

/// Converts a binding error into the contract's marker, leaving an exception
/// pending in the context in every case.
///
/// # Arguments
/// * `ctx` - the context.
/// * `error` - the binding's error.
///
/// # Returns
/// The marker.
#[cold]
fn thrown(ctx: &Ctx<'_>, error: Error) -> Thrown {
    if !error.is_exception() {
        // A Rust-side failure (conversion, allocation): surface it as a JS error so
        // the caller finds something behind `Thrown`.
        let _ = Exception::throw_message(ctx, &error.to_string());
    }
    Thrown
}

/// A native closure, adapted to the binding's function trait.
struct Native<F>(F);

impl<'js, F> IntoJsFunc<'js, ()> for Native<F>
where
    F: for<'a> Fn(&Call<'a, QuickJs>) -> Outcome<Value<'a>> + 'static,
{
    /// # Returns
    /// "Any number of arguments": the closure checks what it needs.
    fn param_requirements() -> ParamRequirement {
        ParamRequirement::any()
    }

    /// Calls the closure with the arguments as a slice.
    ///
    /// # Arguments
    /// * `params` - the engine's view of the call.
    ///
    /// # Returns
    /// The closure's return value.
    ///
    /// # Errors
    /// `Error::Exception` when the closure reports [`Thrown`].
    #[inline]
    fn call<'a>(&self, params: Params<'a, 'js>) -> rquickjs::Result<Value<'js>> {
        let this = params.this();
        // Plain functions are not constructors (no constructor bit), so `new.target`
        // is always undefined here.
        let new_target = Value::new_undefined(params.ctx().clone());
        // SAFETY: `params`, `this`, `new_target` and the lent arguments outlive this
        // call of the closure.
        with_args(&params, |args| (self.0)(&unsafe { Call::new(params.ctx(), &this, &new_target, args) }))
            .map_err(|Thrown| Error::Exception)
    }
}

/// Lends a call's arguments to a closure as a slice, from a stack array of
/// [`INLINE_ARGS`] slots; only longer calls allocate.
///
/// # Arguments
/// * `params` - the engine's view of the call.
/// * `f` - receives the arguments.
///
/// # Returns
/// The closure's result.
#[inline]
fn with_args<'js, R>(params: &Params<'_, 'js>, f: impl FnOnce(&[Value<'js>]) -> R) -> R {
    let count = params.len();
    if count <= INLINE_ARGS {
        let mut slots = [const { MaybeUninit::<Value<'js>>::uninit() }; INLINE_ARGS];
        for (index, slot) in slots.iter_mut().enumerate().take(count) {
            slot.write(params.arg(index).unwrap_or_else(|| Value::new_undefined(params.ctx().clone())));
        }
        // SAFETY: the first `count` slots were just initialized, and
        // `MaybeUninit<T>` has the layout of `T`.
        let args = unsafe { std::slice::from_raw_parts(slots.as_ptr().cast::<Value<'js>>(), count) };
        let result = f(args);
        for slot in &mut slots[..count] {
            // SAFETY: initialized above and not used again.
            unsafe { slot.assume_init_drop() };
        }
        result
    } else {
        let args: Vec<Value<'js>> = (0..count).filter_map(|index| params.arg(index)).collect();
        f(&args)
    }
}

/// The constructor of a native class ([`Scope::class`]).
struct ClassConstructor<T, F> {
    /// The user's constructor: returns the instance data.
    f: F,
    /// The class name, for the "without new" error.
    name: String,
    /// Whether the class has a base that builds its instances.
    derived: bool,
    /// The data type the constructor returns.
    data: std::marker::PhantomData<fn() -> T>,
}

impl<'js, T, F> IntoJsFunc<'js, ()> for ClassConstructor<T, F>
where
    T: 'static,
    F: for<'a> Fn(&Call<'a, QuickJs>) -> Outcome<T> + 'static,
{
    /// # Returns
    /// "Any number of arguments": the constructor checks what it needs.
    fn param_requirements() -> ParamRequirement {
        ParamRequirement::any()
    }

    /// Builds an instance: refuses a call without `new`, makes the object with the
    /// prototype of `new.target` (or lets the base make it), runs the user's
    /// constructor and attaches its data.
    ///
    /// Decision: a derived class reads its base from its own `[[Prototype]]` at
    /// construction time, which is where JS's `super()` looks too, instead of
    /// keeping a persistent handle to the base in the closure; a persistent here
    /// would be an untraced root held by a GC-managed function.
    ///
    /// # Arguments
    /// * `params` - the engine's view of the call; `this` is `new.target`.
    ///
    /// # Returns
    /// The instance.
    ///
    /// # Errors
    /// A `TypeError` without `new` or with a base that is not a constructor; any
    /// exception from the base or the user's constructor.
    fn call<'a>(&self, params: Params<'a, 'js>) -> rquickjs::Result<Value<'js>> {
        let ctx = params.ctx().clone();
        if !params.is_constructor() {
            return Err(Exception::throw_type(&ctx, &format!("Class constructor {} cannot be invoked without 'new'", self.name)));
        }
        let new_target = params.this();
        let class = params.function().into_object().ok_or(Error::Unknown)?;
        let this = with_args(&params, |args| -> rquickjs::Result<Value<'js>> {
            if self.derived {
                let base = class.get_prototype().and_then(|base| base.into_value().into_function()).filter(Function::is_constructor);
                let Some(base) = base else {
                    return Err(Exception::throw_type(&ctx, "Super constructor is not a constructor"));
                };
                let mut raw: Vec<qjs::JSValue> = args.iter().map(Value::as_raw).collect();
                // SAFETY: the context, base, new.target and arguments are live for the
                // whole call; the result is owned by us unless it is the sentinel.
                unsafe {
                    let made = qjs::JS_CallConstructor2(ctx.as_raw().as_ptr(), base.as_raw(), new_target.as_raw(), raw.len() as _, raw.as_mut_ptr());
                    if qjs::JS_IsException(made) {
                        return Err(Error::Exception);
                    }
                    Ok(Value::from_raw(ctx.clone(), made))
                }
            } else {
                // `new.target.prototype`, falling back to the class's own when it is
                // not an object (GetPrototypeFromConstructor).
                let proto = match new_target.as_object().map(|target| target.get::<_, Value<'js>>("prototype")).transpose()?.and_then(Value::into_object) {
                    Some(proto) => proto,
                    None => class.get::<_, Object<'js>>("prototype")?,
                };
                Ok(Class::instance_proto(HostBox::default(), proto)?.into_value())
            }
        })?;
        let data = with_args(&params, |args| (self.f)(&unsafe { Call::new(&ctx, &this, &new_target, args) }))
            .map_err(|Thrown| Error::Exception)?;
        attach(&ctx, &this, Box::new(data))?;
        Ok(this)
    }
}

/// Attaches one native class's data to an instance: into the instance itself when
/// it is a host object, else into a host object hung off the instance under the
/// private [`HostKey`].
///
/// # Arguments
/// * `ctx` - the context.
/// * `this` - the instance.
/// * `data` - the data.
///
/// # Errors
/// A `TypeError` when the base returned a non-object or a proxy, or when the
/// instance's data is being read at this moment.
fn attach<'js>(ctx: &Ctx<'js>, this: &Value<'js>, data: Box<dyn Any>) -> rquickjs::Result<()> {
    let holder = match Class::<HostBox>::from_value(this) {
        Ok(holder) => holder,
        Err(_) => {
            // Decision: a proxy is refused, because reading the key through it would
            // run its traps, and `host_data` must not run JS.
            let object = this.as_object().filter(|_| !this.is_proxy());
            let Some(object) = object else {
                return Err(Exception::throw_type(ctx, "a native class instance must be a plain object, not a proxy"));
            };
            let key = host_key(ctx)?;
            match Class::<HostBox>::from_value(&object.get::<_, Value<'js>>(key.clone())?) {
                Ok(holder) => holder,
                Err(_) => {
                    let holder = Class::instance(ctx.clone(), HostBox::default())?;
                    // Not writable, enumerable or configurable: scripts cannot see the key
                    // (it is private) nor replace the holder.
                    object.prop(key, Property::from(holder.clone().into_value()))?;
                    holder
                }
            }
        }
    };
    let cell = holder.borrow();
    let Ok(mut slots) = cell.0.try_borrow_mut() else {
        return Err(Exception::throw_type(ctx, "instance data is in use"));
    };
    slots.push(data);
    Ok(())
}

/// The private symbol under which an instance that is not itself a host object
/// (one built by a base such as `Uint8Array`) carries its [`HostBox`]; one per
/// runtime, kept in its userdata.
///
/// Decision: a private symbol (`JS_NewPrivateSymbol`) rather than a JS symbol or
/// a side table: scripts cannot reach or forge it, the engine collects the box
/// with its instance, and lookup is one property read. Dropped alternative: an
/// ordinary `Symbol`, which `Object.getOwnPropertySymbols` exposes to scripts.
struct HostKey(Persistent<Value<'static>>);

// SAFETY: a persistent handle is `'static`, so `HostKey` is the same type under any
// lifetime.
unsafe impl<'js> JsLifetime<'js> for HostKey {
    type Changed<'to> = HostKey;
}

/// The runtime's [`HostKey`], created on first use.
///
/// # Arguments
/// * `ctx` - the context.
///
/// # Returns
/// The key as an atom.
///
/// # Errors
/// An allocation failure.
fn host_key<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Atom<'js>> {
    if let Some(key) = existing_host_key(ctx) {
        return key;
    }
    // SAFETY: the context is live; the description is NUL-terminated; the result is
    // owned by us unless it is the sentinel.
    let symbol = unsafe {
        let raw = JS_NewPrivateSymbol(ctx.as_raw().as_ptr(), c"lunarstorm.host".as_ptr());
        if qjs::JS_IsException(raw) {
            return Err(Error::Exception);
        }
        Value::from_raw(ctx.clone(), raw)
    };
    ctx.store_userdata(HostKey(Persistent::save(ctx, symbol.clone()))).map_err(|_| Error::Unknown)?;
    Atom::from_value(ctx.clone(), &symbol)
}

/// The runtime's [`HostKey`] if one was made; reading never creates it.
///
/// # Arguments
/// * `ctx` - the context.
///
/// # Returns
/// The key as an atom, or `None` when no native instance was ever built.
fn existing_host_key<'js>(ctx: &Ctx<'js>) -> Option<rquickjs::Result<Atom<'js>>> {
    let symbol = ctx.userdata::<HostKey>()?.0.clone();
    Some(symbol.restore(ctx).and_then(|symbol| Atom::from_value(ctx.clone(), &symbol)))
}

/// Defines an accessor property: configurable, not enumerable, like a getter or
/// setter of a JS `class`.
///
/// # Arguments
/// * `ctx` - the context.
/// * `object` - where to define it.
/// * `name` - the property name.
/// * `get` - the getter, if any.
/// * `set` - the setter, if any.
///
/// # Errors
/// The engine's exception when the definition fails.
fn define_accessor<'js>(ctx: &Ctx<'js>, object: &Object<'js>, name: &str, get: Option<&Value<'js>>, set: Option<&Value<'js>>) -> rquickjs::Result<()> {
    let flags = qjs::JS_PROP_HAS_GET | qjs::JS_PROP_HAS_SET | qjs::JS_PROP_HAS_CONFIGURABLE | qjs::JS_PROP_CONFIGURABLE | qjs::JS_PROP_HAS_ENUMERABLE | qjs::JS_PROP_THROW;
    let raw = |half: Option<&Value<'js>>| half.map_or(qjs::JS_UNDEFINED, Value::as_raw);
    // SAFETY: the context and values are live; `JS_DefineProperty` borrows the getter
    // and setter, and the atom made here is freed after the call.
    unsafe {
        let ctx_ptr = ctx.as_raw().as_ptr();
        let atom = qjs::JS_NewAtomLen(ctx_ptr, name.as_ptr().cast(), name.len() as _);
        if atom == qjs::JS_ATOM_NULL {
            return Err(Error::Exception);
        }
        let result = qjs::JS_DefineProperty(ctx_ptr, object.as_raw(), atom, qjs::JS_UNDEFINED, raw(get), raw(set), flags as _);
        qjs::JS_FreeAtom(ctx_ptr, atom);
        if result < 0 {
            return Err(Error::Exception);
        }
    }
    Ok(())
}

/// The one class behind every host object and native class instance: owned,
/// type-erased Rust values. Dropping it is the finalizer.
///
/// Decision: a list, not one value, because each native class in an instance's
/// chain attaches its own data (`class B extends A`, both native), told apart by
/// type when read. The `RefCell` is borrowed mutably only while one native
/// constructor attaches its data, when no JS runs.
#[derive(Default)]
struct HostBox(RefCell<Vec<Box<dyn Any>>>);

// SAFETY: `HostBox` holds no JS values and no `'js` lifetime, so it is the same
// type under any lifetime.
unsafe impl<'js> JsLifetime<'js> for HostBox {
    type Changed<'to> = HostBox;
}

impl<'js> Trace<'js> for HostBox {
    /// Nothing to trace: host data never holds scoped handles (they cannot be
    /// `'static`), and persistent handles are roots of their own.
    ///
    /// # Arguments
    /// * `_tracer` - unused.
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

impl<'js> JsClass<'js> for HostBox {
    const NAME: &'static str = "HostObject";
    type Mutable = Readable;

    /// # Arguments
    /// * `_ctx` - unused.
    ///
    /// # Returns
    /// `None`: host objects are created from Rust only.
    ///
    /// # Errors
    /// Never fails.
    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}

impl<'s> Scope<'s, QuickJs> for Ctx<'s> {
    #[inline]
    fn undefined(&self) -> Value<'s> {
        Value::new_undefined(self.clone())
    }

    #[inline]
    fn null(&self) -> Value<'s> {
        Value::new_null(self.clone())
    }

    #[inline]
    fn boolean(&self, value: bool) -> Value<'s> {
        Value::new_bool(self.clone(), value)
    }

    #[inline]
    fn int(&self, value: i32) -> Value<'s> {
        Value::new_int(self.clone(), value)
    }

    /// Boxes a number with the engine's own rule (`JS_NewNumber`): integral values
    /// become engine integers, which the interpreter handles faster than doubles,
    /// and everything else stays a double.
    ///
    /// Defect fixed: the binding's `Value::new_number` normalized with
    /// `value as i32` and `value == int as f64`, which stored `-0.0` as integer `+0`,
    /// so `1 / -0` came out as `+Infinity` in JS. Decision: call the engine's
    /// normalization instead of keeping a copy of it here, so both sides apply one
    /// rule. Dropped alternative: patching the binding's copy, which other callers
    /// of `rquickjs` would then depend on.
    ///
    /// # Arguments
    /// * `value` - the number.
    ///
    /// # Returns
    /// The boxed value, owned by this scope.
    #[inline]
    fn number(&self, value: f64) -> Value<'s> {
        // SAFETY: the context is live for the scope's lifetime; `JS_NewNumber` returns
        // an owned reference, which `Value` takes over without a second increment.
        unsafe { Value::from_raw(self.clone(), qjs::JS_NewNumber(self.as_raw().as_ptr(), value)) }
    }

    #[inline]
    fn string(&self, text: &str) -> Outcome<Value<'s>> {
        JsString::from_str(self.clone(), text).map(JsString::into_value).map_err(|error| thrown(self, error))
    }

    #[inline]
    fn bytes(&self, bytes: Vec<u8>) -> Outcome<Value<'s>> {
        TypedArray::<u8>::new(self.clone(), bytes).map(TypedArray::into_value).map_err(|error| thrown(self, error))
    }

    #[inline]
    fn bytes_copy(&self, bytes: &[u8]) -> Outcome<Value<'s>> {
        TypedArray::<u8>::new_copy(self.clone(), bytes).map(TypedArray::into_value).map_err(|error| thrown(self, error))
    }

    fn array(&self, items: &[Value<'s>]) -> Outcome<Value<'s>> {
        let build = || -> rquickjs::Result<Value<'s>> {
            let array = Array::new(self.clone())?;
            for (index, item) in items.iter().enumerate() {
                array.set(index, item.clone())?;
            }
            Ok(array.into_value())
        };
        build().map_err(|error| thrown(self, error))
    }

    #[inline]
    fn object(&self) -> Outcome<Value<'s>> {
        Object::new(self.clone()).map(Object::into_value).map_err(|error| thrown(self, error))
    }

    /// The binding only constructs typed errors by throwing them, so the error is
    /// thrown and immediately taken back; nothing is left pending.
    fn error(&self, kind: ErrorKind, message: &str) -> Outcome<Value<'s>> {
        let _ = match kind {
            ErrorKind::Error => Exception::throw_message(self, message),
            ErrorKind::Type => Exception::throw_type(self, message),
            ErrorKind::Range => Exception::throw_range(self, message),
            ErrorKind::Syntax => Exception::throw_syntax(self, message),
            ErrorKind::Reference => Exception::throw_reference(self, message),
        };
        Ok(self.catch())
    }

    fn function<F>(&self, name: &str, f: F) -> Outcome<Value<'s>>
    where
        F: for<'a> Fn(&Call<'a, QuickJs>) -> Outcome<Value<'a>> + 'static,
    {
        Function::new(self.clone(), Native(f))
            .and_then(|function| function.with_name(name))
            .map(Function::into_value)
            .map_err(|error| thrown(self, error))
    }

    fn host_object<T: 'static>(&self, data: T) -> Outcome<Value<'s>> {
        let data: Box<dyn Any> = Box::new(data);
        Class::instance(self.clone(), HostBox(RefCell::new(vec![data]))).map(Class::into_value).map_err(|error| thrown(self, error))
    }

    /// Looks in the value itself when it is a host object, else in the holder under
    /// the private [`HostKey`] (a native instance built by a non-native base).
    fn host_data<T: 'static, R>(&self, value: &Value<'s>, f: impl FnOnce(&T) -> R) -> Option<R> {
        let holder = match Class::<HostBox>::from_value(value) {
            Ok(holder) => holder,
            Err(_) => {
                let object = value.as_object().filter(|_| !value.is_proxy())?;
                let key = existing_host_key(self)?.ok()?;
                // A private key has no getter and no proxy is involved, so this read
                // runs no JS.
                Class::<HostBox>::from_value(&object.get::<_, Value<'s>>(key).ok()?).ok()?
            }
        };
        let cell = holder.borrow();
        let slots = cell.0.borrow();
        slots.iter().find_map(|data| data.downcast_ref::<T>()).map(f)
    }

    /// A promise capability; the resolver is the pair `[resolve, reject]` kept as
    /// one persistent array.
    fn promise(&self) -> Outcome<(Value<'s>, Persistent<Value<'static>>)> {
        let make = || -> rquickjs::Result<(Value<'s>, Persistent<Value<'static>>)> {
            let (promise, resolve, reject) = rquickjs::Promise::new(self)?;
            let pair = Array::new(self.clone())?;
            pair.set(0, resolve)?;
            pair.set(1, reject)?;
            Ok((promise.into_value(), Persistent::save(self, pair.into_value())))
        };
        make().map_err(|error| thrown(self, error))
    }

    /// Calls `resolve` or `reject` of the pair; the engine's resolving functions
    /// ignore every call after the first.
    fn settle(&self, resolver: &Persistent<Value<'static>>, result: Result<Value<'s>, Value<'s>>) -> Outcome<()> {
        let pair = self.local(resolver);
        let (index, value) = match result {
            Ok(value) => (0, value),
            Err(reason) => (1, reason),
        };
        let function = pair.as_array().and_then(|pair| pair.get::<Value<'s>>(index).ok()).filter(Value::is_function);
        let Some(function) = function else {
            return Err(self.throw_error(ErrorKind::Type, "not a promise resolver"));
        };
        self.call(&function, &[value]).map(|_| ())
    }

    fn class<T: 'static, F>(&self, spec: ClassSpec<'s, QuickJs>, constructor: F) -> Outcome<Value<'s>>
    where
        F: for<'a> Fn(&Call<'a, QuickJs>) -> Outcome<T> + 'static,
    {
        let build = || -> rquickjs::Result<Value<'s>> {
            let base = match &spec.extends {
                Some(base) => match base.as_function().filter(|base| base.is_constructor()) {
                    Some(base) => Some(base.clone()),
                    None => return Err(Exception::throw_type(self, "Class extends value is not a constructor")),
                },
                None => None,
            };
            let derived = base.is_some();
            let shell = ClassConstructor { f: constructor, name: spec.name.clone(), derived, data: std::marker::PhantomData };
            let class = Function::new(self.clone(), shell)?.with_name(&spec.name)?.with_constructor(true);
            let proto = Object::new(self.clone())?;
            if let Some(base) = &base {
                // As `class C extends B`: C.prototype inherits B.prototype, and C itself
                // inherits B (statics, and where the constructor finds its base).
                let parent = base.get::<_, Value<'s>>("prototype")?;
                match parent.as_object() {
                    Some(parent) => proto.set_prototype(Some(parent))?,
                    None if parent.is_null() => proto.set_prototype(None)?,
                    None => return Err(Exception::throw_type(self, "Class extends value does not have a valid prototype")),
                }
                class.set_prototype(Some(base))?;
            }
            // SAFETY: both are live objects of this context; the call defines
            // `class.prototype` and `proto.constructor` and takes no ownership.
            unsafe { qjs::JS_SetConstructor(self.as_raw().as_ptr(), class.as_raw(), proto.as_raw()) };
            for (name, method) in &spec.methods {
                proto.prop(name.as_str(), Property::from(method.clone()).writable().configurable())?;
            }
            for (name, get, set) in &spec.accessors {
                define_accessor(self, &proto, name, get.as_ref(), set.as_ref())?;
            }
            for (name, value) in &spec.statics {
                class.prop(name.as_str(), Property::from(value.clone()).writable().configurable())?;
            }
            Ok(class.into_value())
        };
        build().map_err(|error| thrown(self, error))
    }

    fn kind(&self, value: &Value<'s>) -> Kind {
        match value.type_of() {
            Type::Undefined | Type::Uninitialized => Kind::Undefined,
            Type::Null => Kind::Null,
            Type::Bool => Kind::Bool,
            Type::Int | Type::Float => Kind::Number,
            Type::String => Kind::String,
            Type::Function | Type::Constructor => Kind::Function,
            Type::Object | Type::Array | Type::Exception | Type::Promise | Type::Proxy => {
                match value.as_object().and_then(|object| object.as_typed_array::<u8>()) {
                    Some(_) => Kind::Bytes,
                    None => Kind::Object,
                }
            }
            _ => Kind::Other,
        }
    }

    #[inline]
    fn as_bool(&self, value: &Value<'s>) -> Option<bool> {
        value.as_bool()
    }

    #[inline]
    fn as_number(&self, value: &Value<'s>) -> Option<f64> {
        value.as_number()
    }

    /// Defect fixed: the binding's `CString::as_str` assumes QuickJS's encoding is
    /// UTF-8, but QuickJS keeps an unpaired surrogate as its 3-byte WTF-8 form
    /// ("Keep unmatched surrogate code points", quickjs.c `JS_ToCStringLen2`), so
    /// `'a\uD800b'` reached natives as an invalid `&str`. Decision: validate the
    /// lent bytes and throw a `TypeError` for an unpaired surrogate. Dropped
    /// alternative: replacing it with U+FFFD, which needs a copy and silently
    /// changes the text a native sees (a path, a key).
    #[inline]
    fn with_str<R>(&self, value: &Value<'s>, f: impl FnOnce(&str) -> R) -> Outcome<R> {
        match value.as_string() {
            Some(string) => match string.clone().to_cstring() {
                Ok(text) => match std::str::from_utf8(cstring_bytes(&text)) {
                    Ok(text) => Ok(f(text)),
                    Err(_) => Err(thrown(self, Exception::throw_type(self, "string contains an unpaired surrogate"))),
                },
                Err(error) => Err(thrown(self, error)),
            },
            None => Err(thrown(self, Exception::throw_type(self, "expected a string"))),
        }
    }

    #[inline]
    fn with_bytes<R>(&self, value: &Value<'s>, f: impl FnOnce(&[u8]) -> R) -> Outcome<R> {
        if let Some(string) = value.as_string() {
            return match string.clone().to_cstring() {
                // Raw bytes, not `as_str()`: see `cstring_bytes`.
                Ok(text) => Ok(f(cstring_bytes(&text))),
                Err(error) => Err(thrown(self, error)),
            };
        }
        if let Some(array) = value.as_object().and_then(|object| object.as_typed_array::<u8>()) {
            // SAFETY: the slice aliases the engine's heap and is valid until JS
            // runs again. The contract forbids `f` from calling into the engine,
            // so nothing can move or detach the buffer while it is lent.
            return Ok(f(unsafe { array.as_bytes() }.unwrap_or(&[])));
        }
        if value.is_undefined() || value.is_null() {
            return Ok(f(&[]));
        }
        Err(thrown(self, Exception::throw_type(self, "data must be a string or Uint8Array")))
    }

    #[inline]
    fn identity(&self, value: &Value<'s>) -> usize {
        // SAFETY: the pointer is only used as a number; it is never dereferenced.
        unsafe { qjs::JS_VALUE_GET_PTR(value.as_raw()) as usize }
    }

    #[inline]
    fn global(&self) -> Value<'s> {
        self.globals().into_value()
    }

    #[inline]
    fn get(&self, object: &Value<'s>, key: &str) -> Outcome<Value<'s>> {
        match object.as_object() {
            Some(object) => object.get::<_, Value<'s>>(key).map_err(|error| thrown(self, error)),
            None => Err(thrown(self, Exception::throw_type(self, "cannot read a property of a non-object"))),
        }
    }

    #[inline]
    fn set(&self, object: &Value<'s>, key: &str, value: Value<'s>) -> Outcome<()> {
        match object.as_object() {
            Some(object) => object.set(key, value).map_err(|error| thrown(self, error)),
            None => Err(thrown(self, Exception::throw_type(self, "cannot set a property of a non-object"))),
        }
    }

    /// Calls straight into `JS_Call`. The arguments are borrowed: the engine does
    /// not consume them, so their reference counts are untouched.
    #[inline]
    fn call(&self, function: &Value<'s>, args: &[Value<'s>]) -> Outcome<Value<'s>> {
        let mut inline = [qjs::JS_UNDEFINED; INLINE_ARGS];
        let mut spilled;
        let raw: &mut [qjs::JSValue] = if args.len() <= INLINE_ARGS {
            &mut inline[..args.len()]
        } else {
            spilled = vec![qjs::JS_UNDEFINED; args.len()];
            &mut spilled
        };
        for (slot, arg) in raw.iter_mut().zip(args) {
            *slot = arg.as_raw();
        }
        // SAFETY: the context, callee and arguments are live for the whole call,
        // `raw` holds exactly `args.len()` values, and the returned value is owned
        // by us: it is either wrapped or, for the exception sentinel, not a
        // reference at all.
        unsafe {
            let result = qjs::JS_Call(self.as_raw().as_ptr(), function.as_raw(), qjs::JS_UNDEFINED, raw.len() as _, raw.as_mut_ptr());
            if qjs::JS_IsException(result) {
                Err(Thrown)
            } else {
                Ok(Value::from_raw(self.clone(), result))
            }
        }
    }

    fn eval(&self, source: &str) -> Outcome<Value<'s>> {
        Ctx::eval::<Value<'s>, _>(self, source).map_err(|error| thrown(self, error))
    }

    fn module(&self, name: &str, source: ModuleSource) -> Outcome<(Value<'s>, Value<'s>)> {
        let run = || -> rquickjs::Result<(Value<'s>, Value<'s>)> {
            let (module, promise) = declare(self, name, source)?.eval()?;
            Ok((module.namespace()?.into_value(), promise.into_value()))
        };
        run().map_err(|error| thrown(self, error))
    }

    /// Stores the exports under the module's name; the loader then declares the C
    /// module from them (`Load::Native`).
    fn native_module(&self, name: &str, exports: &[(&str, Value<'s>)]) -> Outcome<()> {
        let stored = exports.iter().map(|(export, value)| ((*export).to_owned(), Persistent::save(self, value.clone()))).collect();
        let store = || -> Result<(), Error> {
            if self.userdata::<NativePending>().is_none() {
                self.store_userdata(NativePending::default()).map_err(|_| Error::Unknown)?;
            }
            let pending = self.userdata::<NativePending>().ok_or(Error::Unknown)?;
            pending.0.borrow_mut().insert(name.to_owned(), stored);
            Ok(())
        };
        store().map_err(|error| thrown(self, error))
    }

    /// Compiles without linking: QuickJS resolves a module's imports only when it
    /// is evaluated, so nothing is loaded here.
    ///
    /// Decision: source text is stripped and debug information (file name, line and
    /// column tables) is kept, so stack traces of loaded bytecode keep positions.
    fn compile(&self, name: &str, source: ModuleSource) -> Outcome<Vec<u8>> {
        let run = || -> rquickjs::Result<Vec<u8>> {
            Module::declare(self.clone(), name, source.source)?
                .write(WriteOptions { strip_source: true, ..WriteOptions::default() })
        };
        run().map_err(|error| thrown(self, error))
    }

    /// QuickJS never hands out a ticket, so there is nothing to complete.
    fn complete_load(&self, _ticket: u64, _result: Result<ModuleSource, String>) -> Outcome<()> {
        Err(self.throw_error(ErrorKind::Error, "QuickJS loads modules synchronously"))
    }

    #[inline]
    fn run_job(&self) -> bool {
        self.execute_pending_job()
    }

    /// Reads the engine's sticky out-of-memory flag (`jetbunny-sticky-oom-v1` in
    /// the vendored sources), which `JS_ThrowOutOfMemory` sets.
    #[inline]
    fn take_out_of_memory(&self) -> bool {
        // SAFETY: the context is entered on its owner thread, which is the only
        // thread the patch allows to take the flag; the call neither allocates nor
        // runs JS.
        unsafe { JS_JetbunnyTakeOutOfMemory(qjs::JS_GetRuntime(self.as_raw().as_ptr())) }
    }

    #[inline]
    fn throw(&self, value: Value<'s>) -> Thrown {
        let _ = Ctx::throw(self, value);
        Thrown
    }

    /// The engine reports "nothing pending" with its uninitialized sentinel, which
    /// the contract presents as `undefined`.
    #[inline]
    fn exception(&self) -> Value<'s> {
        let value = self.catch();
        if value.type_of() == Type::Uninitialized {
            Value::new_undefined(self.clone())
        } else {
            value
        }
    }

    #[inline]
    fn persist(&self, value: &Value<'s>) -> Persistent<Value<'static>> {
        Persistent::save(self, value.clone())
    }

    /// # Panics
    /// When the handle was created in a different instance; that is a bug in the
    /// caller, not a runtime condition.
    #[inline]
    fn local(&self, handle: &Persistent<Value<'static>>) -> Value<'s> {
        handle.clone().restore(self).expect("persistent handle used in the instance that created it")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Callbacks for an instance that never loads a module or sees a rejection.
    struct Quiet;

    impl Callbacks<QuickJs> for Quiet {
        fn resolve<'s>(&self, _scope: &Ctx<'s>, _base: &str, _name: &str) -> Result<String, String> {
            Err(String::from("no modules in this test"))
        }

        fn load<'s>(&self, _scope: &Ctx<'s>, _name: &str) -> Result<Load, String> {
            Err(String::from("no modules in this test"))
        }

        fn rejection<'s>(&self, _scope: &Ctx<'s>, _promise: usize, _reason: Value<'s>, _handled: bool) {}
    }

    /// A configured threshold reaches the engine unchanged, read back from the
    /// runtime the instance created.
    #[test]
    fn gc_threshold_reaches_the_engine() {
        let limits = Limits { gc_threshold: Some(1 << 20), ..Limits::default() };
        let instance = QuickInstance::new(&limits, Quiet).expect("instance creation");
        // SAFETY: the runtime is alive for the instance's lifetime, and the getter only reads it.
        let threshold = unsafe { qjs::JS_GetGCThreshold(instance.runtime.as_raw().as_ptr()) };
        assert_eq!(threshold, 1 << 20);
    }
}
