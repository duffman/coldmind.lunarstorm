//! The engine-neutral scope: `JsScope` (every value operation), value kinds, property
//! keys, scoped errors, deferreds and promise settlement.

use super::*;
use std::{num::NonZeroUsize, rc::Rc};

/// Non-coercing classification of a validated native value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    /// The `undefined` primitive.
    Undefined,
    /// The `null` primitive.
    Null,
    /// A Boolean primitive.
    Boolean,
    /// An IEEE 754 JavaScript number.
    Number,
    /// A lossless UTF-16 string.
    String,
    /// A native symbol primitive.
    Symbol,
    /// A native arbitrary-precision integer.
    BigInt,
    /// A non-callable native object.
    Object,
    /// A callable native value.
    Function,
}
/// Input-only property key; the text borrow does not constrain result lifetimes.
#[derive(Clone, Copy, Debug)]
pub enum PropertyKey<'s> {
    /// A lossless string property name borrowed only for the operation.
    String(&'s JsString),
    /// A validated native symbol; any other value kind is rejected.
    Symbol(ScopedValue<'s>),
}
/// Either an original thrown JavaScript value or an already detached fault.
#[derive(Debug)]
pub enum ScopedError<'s> {
    /// The original thrown value, protected only for this scope’s lifetime.
    Exception(ScopedValue<'s>),
    /// A detached host, contract, or engine fault requiring no live handle.
    Fault(FailureReport),
}
impl From<FailureReport> for ScopedError<'_> {
    fn from(value: FailureReport) -> Self {
        Self::Fault(value)
    }
}
/// Scoped operation result whose thrown JavaScript value cannot outlive the scope.
pub type JsResult<'s, T> = Result<T, ScopedError<'s>>;
/// Native Promise plus its owned, owner-local settlement capability.
#[derive(Debug)]
pub struct Deferred<'s> {
    /// Promise value protected by the creating scope.
    pub promise: ScopedValue<'s>,
    /// Persistent one-use resolver capability; never send this to a worker.
    pub resolver: RootedDeferred,
}
/// One event from the engine's promise rejection tracker, in arrival order.
#[derive(Debug)]
pub struct Rejection<'s> {
    /// The promise whose rejection state changed.
    pub promise: ScopedValue<'s>,
    /// The rejection reason.
    pub reason: ScopedValue<'s>,
    /// `false` when the promise was rejected with no handler; `true` when a handler was
    /// attached to an already rejected promise afterwards.
    pub handled: bool,
}
/// Engine-internal slots that script cannot read without side effects.
#[derive(Debug)]
pub enum Internals<'s> {
    /// Not a promise or proxy.
    None,
    /// A promise and its state; `result` is the value or reason once settled.
    Promise {
        /// 0 pending, 1 fulfilled, 2 rejected.
        state: u8,
        /// The value or reason once settled; `None` while pending.
        result: Option<ScopedValue<'s>>,
    },
    /// A proxy; target and handler are `null` once revoked.
    Proxy {
        /// The proxied object.
        target: ScopedValue<'s>,
        /// The handler object whose traps run.
        handler: ScopedValue<'s>,
    },
}
/// The fault an engine without a platform hook returns, instead of a silent no-op.
pub fn unsupported_hook(name: &str) -> FailureReport {
    FailureReport::fault(
        FailureCode::Unsupported,
        &format!("engine has no {name} hook"),
    )
}
/// Native Promise resolution or rejection value supplied from an active scope.
#[derive(Clone, Copy, Debug)]
pub enum Settlement<'s> {
    /// Resolve using native Promise resolution, including thenable assimilation.
    Resolve(ScopedValue<'s>),
    /// Reject with the original JavaScript value.
    Reject(ScopedValue<'s>),
}

/// Dyn-compatible engine-neutral interface for one active native root frame.
///
/// Adapters validate owner, engine/context/scope identity, and handle generation
/// before native access. Results and original exceptions borrow the receiver.
/// Registry/queue borrows must be released before reentrant JavaScript or host
/// callbacks; shared receivers alone do not prove aliasing safety. Methods may
/// execute JavaScript unless explicitly described as non-coercing bookkeeping.
pub trait JsScope {
    /// Returns this active frame’s context identity without native execution.
    fn context(&self) -> ContextId;
    /// Returns actual context import/export limits without executing JavaScript.
    fn data_limits(&self) -> ExportLimits;
    /// Returns the native global object protected until scope exit.
    fn global<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>>;
    /// Classifies a validated value without coercion or application execution.
    ///
    /// Foreign, stale, or closed-context handles return a detached fault.
    fn kind(&self, value: ScopedValue<'_>) -> Result<ValueKind, FailureReport>;
    /// Borrows a validated persistent root into this frame without a new root slot.
    ///
    /// The root remains owned by its caller; foreign/stale/closed roots fail before
    /// native access, and the local token cannot outlive this scope.
    fn local<'s>(&'s self, value: &RootedValue) -> JsResult<'s, ScopedValue<'s>>;
    /// Explicitly creates persistent ownership preserving native value identity.
    ///
    /// Validates the scoped token and root limit before exposure. Returns detached
    /// identity/liveness/capacity faults; drop queues release without engine reentry.
    fn retain(&self, value: ScopedValue<'_>) -> Result<RootedValue, FailureReport>;
    /// Consumes bounded copied data and materializes a scope-protected native value.
    ///
    /// Validate depth/nodes/backing capacities iteratively before unsafe traversal.
    /// Records use null-prototype own data properties, arrays are dense, and bytes
    /// use pristine native `Uint8Array` facilities. Failure cleans partial temporaries.
    fn import_data<'s>(&'s self, data: JsData) -> JsResult<'s, ScopedValue<'s>>;
    /// Copies supported native data within the supplied traversal/storage bounds.
    ///
    /// Rejects cycles, functions, symbols, BigInts, sparse arrays, symbol-keyed
    /// records, and unsupported object semantics. Getters/proxies may execute and
    /// throw; failures return no partial export. Shared acyclic children may be copied.
    fn export_data<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, JsData>;
    /// A primitive Number, without coercion.
    ///
    /// Fix: host functions read each numeric argument through [`Self::export_data`], the
    /// generic copier with its budget and footprint walk, to get one `f64`. Decision: a
    /// method of its own, so an engine reads the number directly. The default goes through
    /// `export_data` and gives the same result.
    ///
    /// # Arguments
    /// * `value` - The value.
    ///
    /// # Returns
    /// The number, or `None` when the value is not a primitive Number.
    ///
    /// # Errors
    /// Identity, liveness or engine faults.
    fn number<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, Option<f64>> {
        if self.kind(value)? != ValueKind::Number {
            return Ok(None);
        }
        // `JsData` implements `Drop`, so the copy is matched by reference.
        Ok(match &self.export_data(value, self.data_limits())? {
            JsData::Number(number) => Some(*number),
            _ => None,
        })
    }
    /// Copies a primitive string out as UTF-8, when that is exact and within `limits`.
    ///
    /// Taken from rquickjs (`FromJs for String`): one copy straight to UTF-8 instead of
    /// [`Self::export_data`]'s UTF-16 copy and a transcode. Decision: `None` is not an error
    /// but "use [`Self::export_data`]": for a value that is not a string, a string with an
    /// unpaired surrogate (no `String` holds one) and a string that may exceed `limits`. The
    /// slow path then gives the exact result and error, so this method never accepts a
    /// string `export_data` rejects. The default goes through `export_data` itself.
    ///
    /// # Arguments
    /// * `value` - The value.
    /// * `limits` - The copy limits, as for [`Self::export_data`].
    ///
    /// # Returns
    /// The string's UTF-8, or `None` as described.
    ///
    /// # Errors
    /// Identity, liveness or engine faults.
    fn export_utf8<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, Option<String>> {
        if self.kind(value)? != ValueKind::String {
            return Ok(None);
        }
        // `JsData` implements `Drop`, so the copy is matched by reference.
        Ok(match self.export_data(value, limits) {
            Ok(data) => match &data {
                JsData::String(text) => String::from_utf16(text.as_units()).ok(),
                _ => None,
            },
            Err(_) => None,
        })
    }
    /// Creates a primitive string from UTF-8, without a UTF-16 copy on adapters that
    /// support it. The default goes through [`Self::import_data`].
    ///
    /// # Arguments
    /// * `text` - The string.
    ///
    /// # Returns
    /// The scope-protected string.
    ///
    /// # Errors
    /// Capacity or engine faults, as for [`Self::import_data`].
    fn import_utf8<'s>(&'s self, text: &str) -> JsResult<'s, ScopedValue<'s>> {
        self.import_data(JsData::String(JsString::from_utf8(text)))
    }
    /// Reads a property using normal native JavaScript semantics.
    ///
    /// Accessors/proxies may execute or throw. The key is borrowed only as input;
    /// the returned value or exception remains protected by this scope.
    fn get<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
    ) -> JsResult<'s, ScopedValue<'s>>;
    /// Assigns a property with native strict-assignment failure semantics.
    ///
    /// Accessors/proxies may execute or throw; symbol keys and all value handles
    /// are validated before native access. The key is input-only.
    fn set<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
        value: ScopedValue<'s>,
    ) -> JsResult<'s, ()>;
    /// Calls a native callable with scope-protected receiver and arguments.
    ///
    /// `None` supplies `undefined`; strict/sloppy receiver behavior remains native.
    /// Non-callables throw rather than producing a successful placeholder. Host
    /// reentry is synchronous and no implicit microtask checkpoint runs.
    fn call<'s>(
        &'s self,
        function: ScopedValue<'s>,
        receiver: Option<ScopedValue<'s>>,
        arguments: &[ScopedValue<'s>],
    ) -> JsResult<'s, ScopedValue<'s>>;
    /// Runs native construction with protected arguments and optional new target.
    ///
    /// `None` uses the constructor as `new_target`. Invalid construction produces
    /// a native exception; the returned object/exception borrows this frame.
    fn construct<'s>(
        &'s self,
        constructor: ScopedValue<'s>,
        arguments: &[ScopedValue<'s>],
        new_target: Option<ScopedValue<'s>>,
    ) -> JsResult<'s, ScopedValue<'s>>;
    /// Creates a callable retaining the owner-local host registration.
    ///
    /// The function is not installed globally. Invocation supports synchronous
    /// reentry with per-depth argument storage and contained native-boundary panics.
    /// The adapter must release registration state safely on its owner thread.
    fn host_function<'s>(
        &'s self,
        name: &JsString,
        arity: u16,
        function: Rc<dyn HostFunction>,
    ) -> JsResult<'s, ScopedValue<'s>>;
    /// Returns the same native value for this definition in this context.
    /// Other contexts initialize independently. Reentry into an in-progress
    /// definition fails, and failed initialization may be retried.
    fn materialize<'s>(
        &'s self,
        _definition: &Rc<HostValueDefinition>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        Err(unsupported_hook("host value materialization").into())
    }
    /// Creates a native intrinsic Promise and persistent one-use resolver.
    ///
    /// No mutable global Promise constructor is invoked. Root/capacity failures
    /// produce no successful deferred; the resolver remains owner-local.
    fn deferred<'s>(&'s self) -> JsResult<'s, Deferred<'s>>;
    /// Consumes the resolver and performs native Promise resolve/reject semantics.
    ///
    /// Validate context and capability before access. Resolution may assimilate
    /// thenables; reactions await a later explicit checkpoint. Ordinary Promise
    /// rejection is not itself an engine failure.
    fn settle<'s>(
        &'s self,
        deferred: RootedDeferred,
        settlement: Settlement<'s>,
    ) -> JsResult<'s, ()>;
    /// Drains native jobs and reports truthful remaining queue state.
    ///
    /// The nonzero budget is a hint; indivisible native drains may run to empty.
    /// Retain at most 64 bounded script failures and count excess failures; return
    /// host/contract/engine faults through `Err`. This is not Promise-await behavior.
    fn drain_microtasks(
        &self,
        budget_hint: NonZeroUsize,
    ) -> Result<CheckpointReport, FailureReport>;
    /// Queues `function()` as a job in FIFO order with promise jobs, under the async
    /// context current now. A throw is reported by the checkpoint that runs it.
    fn enqueue_microtask<'s>(&'s self, function: ScopedValue<'s>) -> JsResult<'s, ()> {
        let _ = function;
        Err(unsupported_hook("microtask").into())
    }
    /// The async context value promise reactions capture when registered and restore
    /// while their job runs; `undefined` until set.
    fn async_context<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        Err(unsupported_hook("async context").into())
    }
    /// Replaces the current async context value.
    fn set_async_context<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, ()> {
        let _ = value;
        Err(unsupported_hook("async context").into())
    }
    /// Takes the oldest promise rejection-tracker event; `None` when none is queued.
    /// Queue overflow/allocation loss is reported explicitly. QuickJS retains at
    /// most 1024 raw events; debugger tracking uses its own independent bounded table.
    fn take_rejection<'s>(&'s self) -> JsResult<'s, Option<Rejection<'s>>> {
        Err(unsupported_hook("rejection tracker").into())
    }
    /// Detaches an `ArrayBuffer`, as transfer does; any other value throws a TypeError.
    fn detach_buffer<'s>(&'s self, buffer: ScopedValue<'s>) -> JsResult<'s, ()> {
        let _ = buffer;
        Err(unsupported_hook("buffer detach").into())
    }
    /// Reads promise state or proxy target without running script or marking a rejection
    /// handled.
    fn internals<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, Internals<'s>> {
        let _ = value;
        Err(unsupported_hook("internal slots").into())
    }
    /// Snapshots a scoped error into bounded owned diagnostics before scope exit.
    ///
    /// Never invokes `toString`, getters, proxies, or application code. Missing
    /// metadata remains absent; the result has no native handle and survives close.
    fn detach(&self, error: ScopedError<'_>) -> FailureReport;
    /// Returns the optional source interface, stable across this context’s scopes.
    fn source(&self) -> Option<&dyn SourceApi>;
    /// Returns the optional native compiler, independently of artifact execution.
    fn compiler(&self) -> Option<&dyn CompilerApi>;
    /// Returns optional trusted-artifact execution, independently of compilation.
    fn artifacts(&self) -> Option<&dyn ArtifactApi>;
    /// Returns optional native module operations, stable across this context’s scopes.
    fn modules(&self) -> Option<&dyn ModuleApi>;
}
