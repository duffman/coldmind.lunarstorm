//! VM adapter for QuickJS (quickjs-ng), through the `rquickjs` binding vendored in
//! `runtime/vendor/rquickjs-binding`.
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

use lunarstorm_engine_contract::{
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
use lunarstorm_debugger::debug::{DebugLimits, DebugSessionOptions, VmDebugHandle};
use lunarstorm_debugger::engines::quickjs_ng::Debugger;
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

mod quickjs_instance_and_context;
mod quickjs_module_loading;
mod quickjs_native_functions_and_classes;
mod quickjs_scope_operations;

pub use quickjs_instance_and_context::{QuickContext, QuickInstance};
use quickjs_instance_and_context::*;
use quickjs_module_loading::*;
use quickjs_native_functions_and_classes::*;

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
