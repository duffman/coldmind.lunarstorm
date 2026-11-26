//! The VM interface: the contract between the Runtime and a JavaScript engine.
//!
//! ```text
//! RUNTIME        generic over the VM, bound at compile time
//!    | owns VM (calls down)            ^ implements VM callbacks (calls up)
//!    v                                 |
//! VM INTERFACE                       VM CALLBACKS
//!   Instance: heap, GC, jobs,          module resolve/load,
//!     limits, interrupt                unhandled rejection,
//!   Context: one realm                 interrupt check
//!   Handles: scoped + persistent       each receives a context handle,
//!   Native function signature          never the Runtime
//!   Host objects with finalizers
//!   Capabilities query
//!   Conformance suite
//!    | conforms to (static dispatch)
//!    v
//! VM ADAPTER    wraps one engine
//! ```
//!
//! Design decisions:
//! - **Static dispatch.** [`Vm`] is a marker type with associated types; the Runtime
//!   is generic over it and every call is monomorphized. There is no `dyn` anywhere
//!   in the contract, so an adapter whose methods inline costs nothing.
//! - **Two kinds of handle.** A scoped handle ([`Vm::Value`]) borrows the scope it
//!   was made in and cannot outlive it; a persistent handle ([`Vm::Persistent`]) is
//!   `'static` and is turned back into a scoped one with [`Scope::local`].
//! - **Exceptions stay in the engine.** A failing operation returns [`Thrown`], a
//!   zero-sized marker meaning "an exception is pending in the context"; the value
//!   is fetched with [`Scope::exception`] only when someone wants it. Nothing is
//!   allocated or converted on the error path until then.
//! - **Borrowed data.** [`Scope::with_str`] and [`Scope::with_bytes`] lend the
//!   engine's own memory to a closure instead of copying it out.
//! - **Callbacks take `&self`.** A callback may run while another is active (a
//!   rejection reported during a module load), so implementations use interior
//!   mutability rather than `&mut self`.
//! - **Isolation.** A [`Context`] is a realm. Limits, the interrupt and the job
//!   queue belong to the [`Instance`]; isolation and reload use a new Instance.

pub mod conformance;

/// The debugger crate, whose protocol types [`Instance::debug`] hands out.
pub use lunarstorm_debug;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Version of this contract. An adapter reports the version it implements in its
/// [`Descriptor`]; the conformance suite rejects a mismatch.
pub const CONTRACT_VERSION: u32 = 1;

/// Marker: the operation failed and an exception is pending in the context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thrown;

/// Result of an operation that can throw.
pub type Outcome<T> = Result<T, Thrown>;

/// Identity of an adapter and the engine it wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Descriptor {
    /// Stable name of the adapter, e.g. `quickjs`.
    pub adapter: &'static str,
    /// Name of the wrapped engine, e.g. `quickjs-ng`.
    pub engine: &'static str,
    /// Version of the binding or engine the adapter was built against.
    pub engine_version: &'static str,
    /// The [`CONTRACT_VERSION`] the adapter implements.
    pub contract: u32,
}

/// Optional abilities of an engine. The Runtime asks before relying on one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Compiled code can be serialized and loaded again.
    pub bytecode_cache: bool,
    /// A whole heap can be snapshotted and restored.
    pub snapshots: bool,
    /// The engine compiles to machine code.
    pub jit: bool,
    /// [`Limits::memory`] is enforced.
    pub memory_limits: bool,
    /// Running code can be stopped through [`Interrupt`] or [`Callbacks::interrupt`].
    pub termination: bool,
    /// A debugger can attach.
    pub debug_hooks: bool,
    /// [`Load::Later`] is supported: a module's source may arrive after the load
    /// callback returns. Without it the callback must answer [`Load::Ready`].
    pub async_module_load: bool,
}

/// Resource limits of an [`Instance`]. `None` leaves the engine's default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// Heap size in bytes above which allocation fails.
    pub memory: Option<usize>,
    /// Native stack size in bytes available to JS.
    pub stack: Option<usize>,
    /// Bytes of live memory at which the engine starts a collection. `None` keeps the
    /// engine's default, which then grows by 1.5x after each collection.
    ///
    /// Decision: a knob, not a tuned value. Adopted from the `quickjs_es_runtime`
    /// builder (`gc_threshold`), which sets the same engine threshold; no benchmark
    /// shows a gain from changing it yet.
    pub gc_threshold: Option<usize>,
}

/// Heap statistics of an [`Instance`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Heap {
    /// Bytes currently in use.
    pub used: usize,
    /// The enforced limit, if any.
    pub limit: Option<usize>,
}

/// Requests termination of running JS from any thread.
///
/// The engine polls the flag at its interrupt points; when set, the current JS
/// entry fails with an uncatchable error and the flag is cleared.
#[derive(Debug, Clone, Default)]
pub struct Interrupt(Arc<AtomicBool>);

impl Interrupt {
    /// Asks the engine to stop the JS that is running, or the next JS to run.
    pub fn request(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Reads and clears the request. Called by adapters from the engine's
    /// interrupt hook.
    ///
    /// # Returns
    /// Whether termination was requested since the last call.
    pub fn take(&self) -> bool {
        // A plain load first keeps the common, unrequested case to one relaxed read.
        self.0.load(Ordering::Relaxed) && self.0.swap(false, Ordering::Relaxed)
    }
}

/// The standard error constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// `Error`
    Error,
    /// `TypeError`
    Type,
    /// `RangeError`
    Range,
    /// `SyntaxError`
    Syntax,
    /// `ReferenceError`
    Reference,
}

/// Coarse type of a value, enough to convert it to a host representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `undefined`
    Undefined,
    /// `null`
    Null,
    /// A boolean.
    Bool,
    /// A number.
    Number,
    /// A string.
    String,
    /// A `Uint8Array`.
    Bytes,
    /// A callable object.
    Function,
    /// Any other object.
    Object,
    /// Symbols, big integers and engine-internal values.
    Other,
}

/// `import.meta` of a file module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMeta {
    /// Absolute path of the module.
    pub filename: String,
    /// Whether it is the entry module.
    pub main: bool,
}

/// Bytecode an engine may load as a module ([`ModuleSource::image`]).
///
/// Decision (lunarstorm patch, bytecode trust): the engine does not validate
/// bytecode, so the type admits only two trusted origins and its bytes are
/// private: a module of a `.jbn` package, which exists only after
/// `Package::open` checked its signature and every blob digest, and an image
/// this process's engine wrote with [`Image::compile`]. An unverified buffer
/// cannot reach the engine's bytecode loader. Before, `ModuleSource` carried a
/// `bytecode: bool` that any caller could set over any bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image(Vec<u8>);

impl Image {
    /// A module's bytecode from a verified package.
    ///
    /// # Arguments
    /// * `package` - a package `Package::open` verified.
    /// * `module` - the module's name in its manifest.
    ///
    /// # Returns
    /// The image, or `None` when the package has no such module.
    pub fn from_package(package: &lunarstorm_jbn::Package, module: &str) -> Option<Image> {
        package.bytecode(module).map(|bytes| Image(bytes.to_vec()))
    }

    /// Compiles a module with [`Scope::compile`] and keeps its bytecode as an image.
    ///
    /// # Arguments
    /// * `scope` - the engine that writes the image.
    /// * `name` - the canonical name the module will be loaded under.
    /// * `source` - its source text.
    ///
    /// # Errors
    /// What [`Scope::compile`] reports.
    pub fn compile<'s, V: Vm, S: Scope<'s, V>>(scope: &S, name: &str, source: ModuleSource) -> Outcome<Image> {
        scope.compile(name, source).map(Image)
    }

    /// # Returns
    /// The bytecode.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A module as handed to the engine: source text, or an [`Image`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSource {
    /// The source bytes (UTF-8); ignored when `image` is set.
    pub source: Vec<u8>,
    /// `import.meta` contents; `None` for generated modules.
    pub meta: Option<ModuleMeta>,
    /// Bytecode to load instead of `source`, written by an engine with the same
    /// [`Descriptor`]. Only valid when [`Capabilities::bytecode_cache`] is set.
    pub image: Option<Image>,
    /// Source Map v3 JSON from `source` to the file it was made from (TypeScript),
    /// for an attached debugger; `None` when `source` is the file itself.
    pub source_map: Option<String>,
}

/// Answer of the module load callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load {
    /// The source is available now.
    Ready(ModuleSource),
    /// The source will be delivered with [`Scope::complete_load`] under this ticket.
    /// Only valid when [`Capabilities::async_module_load`] is set.
    Later(u64),
    /// The module is native: the callback has already declared it with
    /// [`Scope::native_module`] under the name it was asked to load, and the engine
    /// takes it from there. Its exports are values the host built, not source text.
    ///
    /// Decision: the answer carries no payload because the values belong to a scope
    /// (`V::Value<'s>`), which would make [`Load`] generic over the VM and break its
    /// derives. The alternative that was dropped, `Load::Native(Vec<export names>)`
    /// plus a second callback to fill them in, needs a module handle type in the
    /// contract for no gain: `load` already holds the scope.
    Native,
}

/// One invocation of a native function: the context handle and the arguments.
///
/// Decision: this is a struct rather than two closure parameters because a
/// higher-ranked closure bound must name its lifetime in a parameter type, and
/// associated types alone (`V::Scope<'s>`) do not count. It holds two raw pointers
/// into the adapter's stack frame, so passing it costs nothing; the accessors hand
/// out references that cannot outlive the `Call`.
pub struct Call<'s, V: Vm> {
    /// The context handle of the call.
    scope: *const V::Scope<'s>,
    /// The receiver (`this`).
    this: *const V::Value<'s>,
    /// `new.target`: the constructor `new` was applied to, `undefined` for a plain call.
    new_target: *const V::Value<'s>,
    /// First argument.
    args: *const V::Value<'s>,
    /// Number of arguments.
    len: usize,
}

impl<'s, V: Vm> Call<'s, V> {
    /// Builds the view an adapter passes to a native closure.
    ///
    /// # Arguments
    /// * `scope` - the context handle of the call.
    /// * `this` - the receiver; for a class constructor, the new instance.
    /// * `new_target` - `new.target`; `undefined` when not called with `new`.
    /// * `args` - the arguments, in order.
    ///
    /// # Returns
    /// The call view.
    ///
    /// # Safety
    /// `scope`, `this`, `new_target` and `args` must stay alive and unmoved for as
    /// long as the returned value is used; adapters create it on the stack around
    /// one closure call.
    #[inline]
    pub unsafe fn new(scope: &V::Scope<'s>, this: &V::Value<'s>, new_target: &V::Value<'s>, args: &[V::Value<'s>]) -> Self {
        Self { scope, this, new_target, args: args.as_ptr(), len: args.len() }
    }

    /// # Returns
    /// The receiver: `this` of the call (`undefined` for a plain function call in
    /// strict code); inside a [`Scope::class`] constructor, the new instance.
    #[inline]
    pub fn this(&self) -> &V::Value<'s> {
        // SAFETY: valid for the life of `self` by the contract of `new`.
        unsafe { &*self.this }
    }

    /// # Returns
    /// `new.target`: the constructor `new` was applied to (a JS subclass when the
    /// call comes from its `super()`), or `undefined` for a plain call.
    #[inline]
    pub fn new_target(&self) -> &V::Value<'s> {
        // SAFETY: valid for the life of `self` by the contract of `new`.
        unsafe { &*self.new_target }
    }

    /// # Returns
    /// The context handle.
    #[inline]
    pub fn scope(&self) -> &V::Scope<'s> {
        // SAFETY: valid for the life of `self` by the contract of `new`.
        unsafe { &*self.scope }
    }

    /// # Returns
    /// The arguments the caller passed.
    #[inline]
    pub fn args(&self) -> &[V::Value<'s>] {
        // SAFETY: valid for the life of `self` by the contract of `new`.
        unsafe { std::slice::from_raw_parts(self.args, self.len) }
    }
}

/// One accessor of a [`ClassSpec`]: `(name, getter, setter)`; a missing half is `None`.
pub type Accessor<'s, V> = (String, Option<<V as Vm>::Value<'s>>, Option<<V as Vm>::Value<'s>>);

/// Description of a native class, built from Rust and turned into a JS constructor
/// by [`Scope::class`].
///
/// Decision: members are JS values the caller built (usually with
/// [`Scope::function`], whose closures read the receiver through [`Call::this`]),
/// not closures of their own. Methods of different closure types then fit in one
/// `Vec` without type erasure, so the contract stays free of `dyn`, and the
/// adapter needs no second native-call path. Only the constructor is a closure of
/// its own ([`Scope::class`]'s `constructor`), because it alone runs under `new`.
pub struct ClassSpec<'s, V: Vm> {
    /// The constructor's `name`.
    pub name: String,
    /// The base: a constructor whose `prototype` becomes the parent of this class's
    /// `prototype`, and which builds every instance. A class from [`Scope::class`]
    /// or a builtin such as `Uint8Array`; `None` for a base class.
    pub extends: Option<V::Value<'s>>,
    /// `(name, function)` pairs installed on the prototype: writable, configurable,
    /// not enumerable, like methods of a JS `class`.
    pub methods: Vec<(String, V::Value<'s>)>,
    /// `(name, getter, setter)` triples installed on the prototype as accessors:
    /// configurable, not enumerable. A missing half is `None`.
    pub accessors: Vec<Accessor<'s, V>>,
    /// `(name, value)` pairs installed on the constructor itself: writable,
    /// configurable, not enumerable, like `static` members of a JS `class`.
    pub statics: Vec<(String, V::Value<'s>)>,
}

impl<'s, V: Vm> ClassSpec<'s, V> {
    /// Starts a description with no base and no members.
    ///
    /// # Arguments
    /// * `name` - the class name.
    ///
    /// # Returns
    /// The description.
    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), extends: None, methods: Vec::new(), accessors: Vec::new(), statics: Vec::new() }
    }

    /// Sets the base constructor.
    ///
    /// # Arguments
    /// * `base` - a native class or a builtin constructor.
    ///
    /// # Returns
    /// The description.
    pub fn extends(mut self, base: V::Value<'s>) -> Self {
        self.extends = Some(base);
        self
    }

    /// Adds a prototype method.
    ///
    /// # Arguments
    /// * `name` - property name.
    /// * `function` - the method.
    ///
    /// # Returns
    /// The description.
    pub fn method(mut self, name: &str, function: V::Value<'s>) -> Self {
        self.methods.push((name.to_owned(), function));
        self
    }

    /// Adds a prototype accessor.
    ///
    /// # Arguments
    /// * `name` - property name.
    /// * `get` - the getter, if any.
    /// * `set` - the setter, if any.
    ///
    /// # Returns
    /// The description.
    pub fn accessor(mut self, name: &str, get: Option<V::Value<'s>>, set: Option<V::Value<'s>>) -> Self {
        self.accessors.push((name.to_owned(), get, set));
        self
    }

    /// Adds a static member.
    ///
    /// # Arguments
    /// * `name` - property name.
    /// * `value` - the value (a function for a static method).
    ///
    /// # Returns
    /// The description.
    pub fn static_member(mut self, name: &str, value: V::Value<'s>) -> Self {
        self.statics.push((name.to_owned(), value));
        self
    }
}

/// The calls an engine makes up into its owner.
///
/// Every method receives a context handle and never the Runtime, which rules out
/// re-entering the Runtime from inside the engine.
pub trait Callbacks<V: Vm>: 'static {
    /// Turns an import specifier into a canonical module name.
    ///
    /// # Arguments
    /// * `scope` - the context the import happens in.
    /// * `base` - canonical name of the importing module.
    /// * `name` - the specifier as written.
    ///
    /// # Returns
    /// The canonical name, which is what [`Callbacks::load`] receives.
    ///
    /// # Errors
    /// A message explaining why the specifier cannot be resolved.
    fn resolve<'s>(&self, scope: &V::Scope<'s>, base: &str, name: &str) -> Result<String, String>;

    /// Provides the source of a resolved module.
    ///
    /// # Arguments
    /// * `scope` - the context the import happens in.
    /// * `name` - canonical name from [`Callbacks::resolve`].
    ///
    /// # Returns
    /// The source, or a ticket for it.
    ///
    /// # Errors
    /// A message explaining why the module cannot be loaded.
    fn load<'s>(&self, scope: &V::Scope<'s>, name: &str) -> Result<Load, String>;

    /// Reports a promise rejected without a handler, or later given one.
    ///
    /// # Arguments
    /// * `scope` - the context the promise lives in.
    /// * `promise` - identity of the promise ([`Scope::identity`]).
    /// * `reason` - the rejection reason.
    /// * `handled` - `false` when the promise was rejected with no handler; `true`
    ///   when a handler was attached to a promise reported earlier.
    fn rejection<'s>(&self, scope: &V::Scope<'s>, promise: usize, reason: V::Value<'s>, handled: bool);

    /// Interrupt check, called by the engine at its interrupt points while JS runs.
    ///
    /// # Returns
    /// `true` to terminate the running JS. The default never interrupts.
    fn interrupt(&self) -> bool {
        false
    }
}

/// A JavaScript engine, as a set of types. Implemented by a zero-sized marker.
pub trait Vm: Sized + 'static {
    /// The engine instance: heap, GC, job queue, limits, interrupt.
    type Instance: Instance<Self>;
    /// One realm inside an instance.
    type Context: Context<Self>;
    /// A context handle, valid while the context is entered.
    type Scope<'s>: Scope<'s, Self>;
    /// A scoped handle to a value; cannot outlive its scope.
    type Value<'s>: Clone;
    /// A persistent handle to a value; keeps it alive until dropped. Must be
    /// dropped before its [`Instance`].
    type Persistent: 'static;

    /// # Returns
    /// The adapter's identity.
    fn descriptor() -> Descriptor;

    /// # Returns
    /// What the engine can do beyond the mandatory contract.
    fn capabilities() -> Capabilities;
}

/// The engine instance.
pub trait Instance<V: Vm>: Sized {
    /// Creates an instance.
    ///
    /// # Arguments
    /// * `limits` - resource limits to enforce.
    /// * `callbacks` - the owner's callbacks, kept for the instance's lifetime.
    ///
    /// # Returns
    /// The instance.
    ///
    /// # Errors
    /// A message when the engine cannot be created.
    fn new<C: Callbacks<V>>(limits: &Limits, callbacks: C) -> Result<Self, String>;

    /// Creates a realm with the standard globals.
    ///
    /// # Returns
    /// The context.
    ///
    /// # Errors
    /// A message when the engine cannot allocate it.
    fn context(&self) -> Result<V::Context, String>;

    /// Runs a full garbage collection, finalizing unreachable host objects.
    fn collect_garbage(&self);

    /// # Returns
    /// Current heap statistics.
    fn heap(&self) -> Heap;

    /// # Returns
    /// A handle that terminates running JS from any thread.
    fn interrupt(&self) -> Interrupt;

    /// Opens a debugger session on a context. Requires [`Capabilities::debug_hooks`].
    ///
    /// Decision: the debugger is jetbunny's (`lunarstorm-debug`): the adapter owns
    /// the engine half, the returned handle is the protocol half and goes to a DAP
    /// transport (`lunarstorm_debug::debug::DapAdapter`). Code compiled after this
    /// call carries statement markers; code compiled before it is not debuggable.
    ///
    /// # Arguments
    /// * `context` - the realm to debug.
    /// * `wake` - wakes the owner's loop from any thread, so it calls
    ///   [`Instance::debug_service`]; must not block.
    /// * `options` - whether to pause before the first application entry.
    ///
    /// # Returns
    /// The session controller, usable from any thread.
    ///
    /// # Errors
    /// The engine has no debugger, or one is already attached.
    fn debug(
        &self,
        context: &V::Context,
        wake: Arc<dyn Fn() + Send + Sync>,
        options: lunarstorm_debug::debug::DebugSessionOptions,
    ) -> Result<lunarstorm_debug::debug::VmDebugHandle, String> {
        let _ = (context, wake, options);
        Err(String::from("this engine has no debugger"))
    }

    /// Called before every JS entry while a debugger is attached: holds the entry
    /// while a break-on-start pause is pending.
    ///
    /// # Errors
    /// The debugger session failed.
    fn debug_entry(&self) -> Result<(), String> {
        Ok(())
    }

    /// Called at every loop step while a debugger is attached: serves pause,
    /// resume and inspection requests.
    ///
    /// # Returns
    /// `true` when the client asked the program to stop.
    ///
    /// # Errors
    /// The debugger session failed.
    fn debug_service(&self) -> Result<bool, String> {
        Ok(false)
    }

    /// Called after the job queue was drained: qualifies unhandled rejections for
    /// the debugger.
    ///
    /// # Errors
    /// The debugger session failed.
    fn debug_checkpoint(&self) -> Result<(), String> {
        Ok(())
    }
}

/// One realm.
pub trait Context<V: Vm> {
    /// Enters the context and runs a closure with a handle to it.
    ///
    /// # Arguments
    /// * `f` - the code to run; scoped handles it creates die with the scope.
    ///
    /// # Returns
    /// Whatever the closure returns.
    fn enter<R>(&self, f: impl for<'s> FnOnce(V::Scope<'s>) -> R) -> R;
}

/// A context handle: creates, inspects and calls values.
///
/// Methods take values by reference and never run JS unless their documentation
/// says so ([`Scope::call`], [`Scope::eval`], [`Scope::module`], [`Scope::run_job`]).
pub trait Scope<'s, V: Vm>: Clone {
    // ---- constructors ---------------------------------------------------------

    /// # Returns
    /// `undefined`.
    fn undefined(&self) -> V::Value<'s>;

    /// # Returns
    /// `null`.
    fn null(&self) -> V::Value<'s>;

    /// # Arguments
    /// * `value` - the boolean.
    ///
    /// # Returns
    /// A JS boolean.
    fn boolean(&self, value: bool) -> V::Value<'s>;

    /// # Arguments
    /// * `value` - the integer.
    ///
    /// # Returns
    /// A JS number holding a 32-bit integer.
    fn int(&self, value: i32) -> V::Value<'s>;

    /// # Arguments
    /// * `value` - the number.
    ///
    /// # Returns
    /// A JS number.
    fn number(&self, value: f64) -> V::Value<'s>;

    /// # Arguments
    /// * `text` - UTF-8 text.
    ///
    /// # Returns
    /// A JS string.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn string(&self, text: &str) -> Outcome<V::Value<'s>>;

    /// Creates a `Uint8Array` that takes ownership of a buffer without copying it.
    ///
    /// # Arguments
    /// * `bytes` - the buffer; freed when the array is collected.
    ///
    /// # Returns
    /// The array.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn bytes(&self, bytes: Vec<u8>) -> Outcome<V::Value<'s>>;

    /// Creates a `Uint8Array` holding a copy of a slice.
    ///
    /// # Arguments
    /// * `bytes` - the bytes to copy.
    ///
    /// # Returns
    /// The array.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn bytes_copy(&self, bytes: &[u8]) -> Outcome<V::Value<'s>>;

    /// # Arguments
    /// * `items` - the elements, in order.
    ///
    /// # Returns
    /// A JS array.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn array(&self, items: &[V::Value<'s>]) -> Outcome<V::Value<'s>>;

    /// # Returns
    /// An empty plain object.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn object(&self) -> Outcome<V::Value<'s>>;

    /// Creates an error object without throwing it.
    ///
    /// # Arguments
    /// * `kind` - which constructor.
    /// * `message` - the error message.
    ///
    /// # Returns
    /// The error object.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn error(&self, kind: ErrorKind, message: &str) -> Outcome<V::Value<'s>>;

    /// Creates a JS function backed by a native closure.
    ///
    /// This is the native function signature of the contract: the closure receives
    /// a [`Call`] (the context handle plus the arguments as a slice; missing
    /// arguments are simply absent) and returns a value or [`Thrown`].
    ///
    /// # Arguments
    /// * `name` - the function's `name` property.
    /// * `f` - the implementation; dropped when the function is collected.
    ///
    /// # Returns
    /// The function.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn function<F>(&self, name: &str, f: F) -> Outcome<V::Value<'s>>
    where
        F: for<'a> Fn(&Call<'a, V>) -> Outcome<V::Value<'a>> + 'static;

    /// Creates a host object: a JS object owning a Rust value.
    ///
    /// The value's `Drop` is the finalizer; it runs when the object is collected or
    /// the instance is destroyed.
    ///
    /// # Arguments
    /// * `data` - the Rust value to own.
    ///
    /// # Returns
    /// The object.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn host_object<T: 'static>(&self, data: T) -> Outcome<V::Value<'s>>;

    /// Reads the Rust value inside a host object or an instance of a native class
    /// ([`Scope::class`]).
    ///
    /// # Arguments
    /// * `value` - a value that may be a host object or native instance.
    /// * `f` - receives the Rust value.
    ///
    /// # Returns
    /// The closure's result, or `None` when `value` is not a host object holding a `T`.
    fn host_data<T: 'static, R>(&self, value: &V::Value<'s>, f: impl FnOnce(&T) -> R) -> Option<R>;

    // Decision: `promise`, `settle` and `class` are required, with no default that
    // throws. The QuickJS adapter is the only implementor in the workspace, so
    // nothing needs a fallback, and a required member makes a future adapter fail to
    // compile instead of failing at run time.

    /// Creates a pending promise and the handle that settles it.
    ///
    /// # Returns
    /// `(promise, resolver)`: the promise to hand to JS, and a persistent handle
    /// that [`Scope::settle`] takes. The resolver may outlive the scope; dropping it
    /// unsettled leaves the promise pending forever.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails.
    fn promise(&self) -> Outcome<(V::Value<'s>, V::Persistent)>;

    /// Settles a promise made by [`Scope::promise`]. Reactions are queued as jobs
    /// ([`Scope::run_job`]); none runs here. Settling an already settled promise
    /// does nothing, as in JS.
    ///
    /// Decision: one resolver handle covers both outcomes, chosen by `result`, so
    /// the caller keeps a single persistent per pending operation.
    ///
    /// # Arguments
    /// * `resolver` - the handle [`Scope::promise`] returned.
    /// * `result` - `Ok(value)` fulfils (resolving with a thenable adopts it, which
    ///   reads its `then` property); `Err(reason)` rejects.
    ///
    /// # Errors
    /// [`Thrown`] (a `TypeError`) when `resolver` is not a resolver handle, or when
    /// reading a thenable's `then` throws.
    fn settle(&self, resolver: &V::Persistent, result: Result<V::Value<'s>, V::Value<'s>>) -> Outcome<()>;

    /// Creates a native class: a JS constructor described by `spec` whose instances
    /// carry Rust data.
    ///
    /// On `new C(...args)` (or `super(...args)` from a JS subclass) the adapter
    /// makes the instance with the prototype of `new.target`, runs `constructor`
    /// with [`Call::this`] set to that instance and [`Call::new_target`] to
    /// `new.target`, and attaches the returned `T`; [`Scope::host_data`] then finds
    /// it on the instance (in methods: `host_data(call.this(), ..)`). Calling the
    /// class without `new` throws a `TypeError`.
    ///
    /// With [`ClassSpec::extends`], the instance is built by the base constructor
    /// with the same arguments and `new.target`, so an instance of a class
    /// extending `Uint8Array` is a real `Uint8Array`.
    ///
    /// Decision: the data is the constructor's return value rather than something
    /// set on `this` later, so every instance has its data and no "not yet
    /// initialized" state is visible to methods once construction returns. The
    /// data is attached after `constructor` returns, so `constructor` itself cannot
    /// read it back. The base receives the same arguments as the class.
    ///
    /// # Arguments
    /// * `spec` - name, base and members.
    /// * `constructor` - runs once per instance and returns its data; dropped when
    ///   the class is collected.
    ///
    /// # Returns
    /// The constructor.
    ///
    /// # Errors
    /// [`Thrown`] when allocation fails, or when `spec.extends` is not a constructor
    /// (a `TypeError`).
    fn class<T: 'static, F>(&self, spec: ClassSpec<'s, V>, constructor: F) -> Outcome<V::Value<'s>>
    where
        F: for<'a> Fn(&Call<'a, V>) -> Outcome<T> + 'static;

    // ---- inspection -----------------------------------------------------------

    /// # Arguments
    /// * `value` - any value.
    ///
    /// # Returns
    /// Its coarse type.
    fn kind(&self, value: &V::Value<'s>) -> Kind;

    /// # Arguments
    /// * `value` - any value.
    ///
    /// # Returns
    /// The boolean, or `None` when the value is not a boolean.
    fn as_bool(&self, value: &V::Value<'s>) -> Option<bool>;

    /// # Arguments
    /// * `value` - any value.
    ///
    /// # Returns
    /// The number, or `None` when the value is not a number.
    fn as_number(&self, value: &V::Value<'s>) -> Option<f64>;

    /// Lends a JS string's UTF-8 text to a closure without copying it out.
    ///
    /// # Arguments
    /// * `value` - a string value.
    /// * `f` - receives the text; must not call back into the engine.
    ///
    /// # Returns
    /// The closure's result.
    ///
    /// # Errors
    /// [`Thrown`] (a `TypeError`) when the value is not a string.
    fn with_str<R>(&self, value: &V::Value<'s>, f: impl FnOnce(&str) -> R) -> Outcome<R>;

    /// Lends the bytes of a string (as UTF-8) or a `Uint8Array` (in place) to a
    /// closure. `undefined` and `null` lend an empty slice.
    ///
    /// # Arguments
    /// * `value` - a string, `Uint8Array` or nullish value.
    /// * `f` - receives the bytes; must not call back into the engine, which could
    ///   move or detach the buffer.
    ///
    /// # Returns
    /// The closure's result.
    ///
    /// # Errors
    /// [`Thrown`] (a `TypeError`) for any other value.
    fn with_bytes<R>(&self, value: &V::Value<'s>, f: impl FnOnce(&[u8]) -> R) -> Outcome<R>;

    /// # Arguments
    /// * `value` - an object value.
    ///
    /// # Returns
    /// A number identifying the object for as long as it is alive; never dereferenced.
    fn identity(&self, value: &V::Value<'s>) -> usize;

    // ---- objects --------------------------------------------------------------

    /// # Returns
    /// The realm's global object.
    fn global(&self) -> V::Value<'s>;

    /// Reads a property. May run a getter.
    ///
    /// # Arguments
    /// * `object` - the object.
    /// * `key` - property name.
    ///
    /// # Returns
    /// The property's value; `undefined` when absent.
    ///
    /// # Errors
    /// [`Thrown`] when `object` is not an object or a getter throws.
    fn get(&self, object: &V::Value<'s>, key: &str) -> Outcome<V::Value<'s>>;

    /// Writes a property. May run a setter.
    ///
    /// # Arguments
    /// * `object` - the object.
    /// * `key` - property name.
    /// * `value` - the value to store.
    ///
    /// # Errors
    /// [`Thrown`] when `object` is not an object or a setter throws.
    fn set(&self, object: &V::Value<'s>, key: &str, value: V::Value<'s>) -> Outcome<()>;

    /// Calls a function with `this` undefined. Runs JS.
    ///
    /// # Arguments
    /// * `function` - the callee.
    /// * `args` - the arguments.
    ///
    /// # Returns
    /// The function's return value.
    ///
    /// # Errors
    /// [`Thrown`] when the callee throws or is not callable.
    fn call(&self, function: &V::Value<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>>;

    // ---- code -----------------------------------------------------------------

    /// Evaluates a classic script. Runs JS.
    ///
    /// # Arguments
    /// * `source` - the script text.
    ///
    /// # Returns
    /// The completion value.
    ///
    /// # Errors
    /// [`Thrown`] on a syntax error or an uncaught exception.
    fn eval(&self, source: &str) -> Outcome<V::Value<'s>>;

    /// Compiles and evaluates an ES module. Runs JS; imports reach
    /// [`Callbacks::resolve`] and [`Callbacks::load`].
    ///
    /// # Arguments
    /// * `name` - canonical name of the module.
    /// * `source` - its source and `import.meta`.
    ///
    /// # Returns
    /// `(namespace, promise)`: the module namespace object, and the promise that
    /// settles when evaluation (including top-level `await`) completes.
    ///
    /// # Errors
    /// [`Thrown`] on a syntax error or a failed import.
    fn module(&self, name: &str, source: ModuleSource) -> Outcome<(V::Value<'s>, V::Value<'s>)>;

    /// Declares a native module: an ES module whose exports are values the host built
    /// (usually functions made with [`Scope::function`]) instead of source text.
    ///
    /// Call it from [`Callbacks::load`] for the module being loaded and answer
    /// [`Load::Native`]. The module is evaluated like any other, once per instance, so
    /// every importer sees the same export values.
    ///
    /// # Arguments
    /// * `name` - the canonical name [`Callbacks::load`] was asked for.
    /// * `exports` - `(export name, value)` pairs; the values are kept alive by the
    ///   engine for the life of the module.
    ///
    /// # Errors
    /// [`Thrown`] when the engine cannot declare the module, or (the default) when
    /// the adapter has no native modules.
    fn native_module(&self, name: &str, exports: &[(&str, V::Value<'s>)]) -> Outcome<()> {
        let _ = (name, exports);
        Err(self.throw_error(ErrorKind::Error, "this engine has no native modules"))
    }

    /// Delivers the source promised by [`Load::Later`].
    ///
    /// # Arguments
    /// * `ticket` - the ticket the load callback returned.
    /// * `result` - the source, or why it could not be loaded.
    ///
    /// # Errors
    /// [`Thrown`] when the engine lacks [`Capabilities::async_module_load`] or the
    /// ticket is unknown.
    fn complete_load(&self, ticket: u64, result: Result<ModuleSource, String>) -> Outcome<()>;

    /// Compiles a module without linking or evaluating it, and writes its bytecode:
    /// source text stripped, line and column positions kept. Its imports are not
    /// resolved or loaded; they resolve when the bytecode is loaded as a module.
    ///
    /// # Arguments
    /// * `name` - the canonical name the module will be loaded under; the engine
    ///   keeps it in the bytecode and names stack frames after it.
    /// * `source` - its source text.
    ///
    /// # Returns
    /// The bytecode; [`Image::compile`] makes it loadable as [`ModuleSource::image`].
    ///
    /// # Errors
    /// [`Thrown`] on a syntax error, or when the engine lacks
    /// [`Capabilities::bytecode_cache`].
    fn compile(&self, name: &str, source: ModuleSource) -> Outcome<Vec<u8>>;

    /// Runs one job from the instance's queue (a promise reaction). Runs JS.
    ///
    /// An exception thrown by a job rejects its promise and is reported through
    /// [`Callbacks::rejection`]; it is not returned here.
    ///
    /// # Returns
    /// `false` when the queue was empty.
    fn run_job(&self) -> bool;

    /// Reads and clears the engine's out-of-memory mark: set whenever an
    /// allocation failed since the last call, even if the script caught the error.
    ///
    /// Decision: a caught out-of-memory error does not make the heap usable again,
    /// so the Runtime treats a set mark as fatal after every JS entry. Engines
    /// without the mark answer `false`.
    ///
    /// # Returns
    /// Whether an allocation failed since the last call.
    fn take_out_of_memory(&self) -> bool {
        false
    }

    // ---- exceptions -----------------------------------------------------------

    /// Throws a value.
    ///
    /// # Arguments
    /// * `value` - the value to throw.
    ///
    /// # Returns
    /// The marker to return from the current native function.
    fn throw(&self, value: V::Value<'s>) -> Thrown;

    /// Takes the pending exception, clearing it.
    ///
    /// # Returns
    /// The thrown value; `undefined` when nothing is pending.
    fn exception(&self) -> V::Value<'s>;

    /// Creates an error and throws it.
    ///
    /// # Arguments
    /// * `kind` - which constructor.
    /// * `message` - the error message.
    ///
    /// # Returns
    /// The marker to return from the current native function.
    fn throw_error(&self, kind: ErrorKind, message: &str) -> Thrown {
        match self.error(kind, message) {
            Ok(error) => self.throw(error),
            Err(thrown) => thrown,
        }
    }

    // ---- handles --------------------------------------------------------------

    /// Makes a persistent handle from a scoped one.
    ///
    /// # Arguments
    /// * `value` - the value to keep alive.
    ///
    /// # Returns
    /// A handle that outlives the scope.
    fn persist(&self, value: &V::Value<'s>) -> V::Persistent;

    /// Makes a scoped handle from a persistent one.
    ///
    /// # Arguments
    /// * `handle` - a handle created in the same instance.
    ///
    /// # Returns
    /// The value, usable in this scope.
    fn local(&self, handle: &V::Persistent) -> V::Value<'s>;
}
