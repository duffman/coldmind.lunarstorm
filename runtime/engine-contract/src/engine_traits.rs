//! The traits an engine adapter implements: [`Vm`] (the associated types),
//! [`Instance`] (heap, jobs, limits), [`Context`] (one realm), [`Scope`] (every
//! value operation) and [`Callbacks`] (what the engine calls up into).

use crate::*;
use std::sync::Arc;

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
    /// transport (`lunarstorm_debugger::debug::DapAdapter`). Code compiled after this
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
        options: lunarstorm_debugger::debug::DebugSessionOptions,
    ) -> Result<lunarstorm_debugger::debug::VmDebugHandle, String> {
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

    /// Calls a function with a receiver, as `function.call(this, ...args)`. Runs JS.
    ///
    /// Decision: required, like [`Scope::call`]. The natives that implement Node's
    /// builtins call listeners with their emitter as `this` and builtin methods on
    /// their objects (`Map.prototype.get`); the alternative, `Reflect.apply` through
    /// [`Scope::call`], allocates an argument array per call on hot paths.
    ///
    /// # Arguments
    /// * `function` - the callee.
    /// * `this` - the receiver.
    /// * `args` - the arguments.
    ///
    /// # Returns
    /// The function's return value.
    ///
    /// # Errors
    /// [`Thrown`] when the callee throws or is not callable.
    fn call_with(&self, function: &V::Value<'s>, this: &V::Value<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>>;

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
