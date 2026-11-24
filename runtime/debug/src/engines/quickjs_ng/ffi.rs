//! Private C ABI; native QuickJS layouts never cross this module.
//!
//! Safety: all calls run on one owner thread with a live `Context`. Inputs are
//! borrowed and must belong to that context. Returned `Value`s are owned and
//! released once with `jbq_free`, before context destruction. Exception sentinel
//! results require `jbq_take_exception`. No callback/finalizer may unwind.
//! Opaque pointers and argv/key indices must remain valid for the documented call.
#![allow(dead_code)]
use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct Value {
    pub(super) words: [u64; 2],
}
/// The engine's first (lowest) tag of a reference-counted value (`JS_TAG_FIRST`).
const TAG_FIRST: i32 = -9;
/// The engine's tag of the exception sentinel (`JS_TAG_EXCEPTION`).
const TAG_EXCEPTION: i32 = 6;
impl Value {
    /// The value's engine tag (`JS_VALUE_GET_TAG`).
    ///
    /// Decision: the bridge packs a `JSValue` into these words byte for byte. On 64-bit
    /// targets that is the 16-byte struct whose second word is the tag; on 32-bit targets
    /// the engine NaN-boxes and the tag is the high half of the first word.
    ///
    /// # Returns
    /// The tag.
    #[inline]
    fn tag(self) -> i32 {
        #[cfg(target_pointer_width = "64")]
        {
            self.words[1] as i32
        }
        #[cfg(not(target_pointer_width = "64"))]
        {
            (self.words[0] >> 32) as i32
        }
    }
    /// Whether this is the exception sentinel, as `jbq_is_exception` answers.
    ///
    /// Fix: every value the engine returned was tested with a call into the shim.
    /// Decision: the tag is compared here, with no call.
    ///
    /// # Returns
    /// `true` for the sentinel.
    #[inline]
    pub(super) fn is_exception(self) -> bool {
        self.tag() == TAG_EXCEPTION
    }
    /// Whether the value is reference-counted (`JS_VALUE_HAS_REF_COUNT`): a string, symbol,
    /// BigInt or object, as opposed to a number, boolean, null or undefined.
    ///
    /// # Returns
    /// `true` when duplicating or freeing the value touches a reference count.
    #[inline]
    pub(super) fn has_ref(self) -> bool {
        (self.tag() as u32) >= (TAG_FIRST as u32)
    }
}
#[repr(C)]
pub(super) struct Context {
    _private: [u8; 0],
}
#[repr(C)]
pub(super) struct Keys {
    _private: [u8; 0],
}
/// Borrowed source/lease for one synchronous call; returns an owned compiled
/// object or the exception sentinel. Must not unwind.
pub(super) type Compiler = unsafe extern "C" fn(
    *mut c_void,
    *mut Context,
    u64,
    *const u16,
    usize,
    *const c_char,
    c_int,
    c_int,
) -> Value;
/// Mirrors JSJetbunnyEnvView.
#[repr(C)]
#[derive(Default)]
pub(super) struct EnvView {
    pub(super) args: u32,
    pub(super) locals: u32,
    pub(super) captures: u32,
    pub(super) var_refs: u32,
    pub(super) strict_mode: u32,
    pub(super) flags: u32,
    pub(super) scope_head: i32,
}
/// Mirrors JSJetbunnyEnvBinding.
#[repr(C)]
#[derive(Default)]
pub(super) struct EnvBinding {
    pub(super) name: u32,
    pub(super) scope_level: u32,
    pub(super) scope_next: i32,
    pub(super) flags: u32,
    pub(super) var_ref_idx: u32,
    pub(super) initialized: u32,
    pub(super) live: u32,
}
pub(super) type Interrupt = unsafe extern "C" fn(*mut c_void) -> c_int;
/// One queued job: owned by the host from the sink until `jbq_run_job` or `jbq_drop_job`.
#[repr(C)]
pub(super) struct Job {
    _private: [u8; 0],
}
/// Receives every job the runtime creates; `< 0` refuses it. Must not unwind.
pub(super) type JobSink = unsafe extern "C" fn(*mut c_void, *mut Job) -> c_int;
pub(super) type DropCallback = unsafe extern "C" fn(*mut c_void);
/// argv/receiver are borrowed for the call; result owned. thrown: 0 success,
/// 1 throw returned value, 2 synthesize host fault (retained out-of-band by Rust).
pub(super) type HostCallback = unsafe extern "C" fn(
    *mut c_void,
    *mut Context,
    Value,
    *const c_void,
    usize,
    *mut c_int,
) -> Value;

// SAFETY: declarations match native/quickjs_ng/bridge.h, including repr(C)
// containers and platform C integer types. Each caller upholds the module contract.
unsafe extern "C" {
    /// Creates the bridge context on `runtime`, which the caller owns and frees afterwards.
    ///
    /// # Arguments
    /// * `runtime` - the `JSRuntime` of the context's rquickjs `Runtime`.
    /// * `memory` - heap limit; `stack` - native stack limit.
    /// * `interrupt`, `opaque` - interrupt poll and its argument.
    ///
    /// # Returns
    /// The context, or null on allocation or class registration failure.
    pub(super) fn jbq_context_new(
        runtime: *mut c_void,
        memory: usize,
        stack: usize,
        interrupt: Option<Interrupt>,
        opaque: *mut c_void,
    ) -> *mut Context;
    /// Releases the bridge context's state and its context reference; not the runtime.
    ///
    /// # Arguments
    /// * `context` - the bridge context, freed here.
    pub(super) fn jbq_context_free(context: *mut Context);
    /// lunarstorm patch: the bridge around a context its owner (rquickjs) created; the
    /// runtime's handlers stay the owner's.
    ///
    /// # Arguments
    /// * `js_context` - a live `JSContext`; the bridge takes one reference.
    ///
    /// # Returns
    /// The bridge context, or null on allocation or class registration failure.
    pub(super) fn jbq_context_wrap(js_context: *mut c_void) -> *mut Context;
    /// lunarstorm patch: the debugger's share of the interrupt poll of a wrapped context.
    ///
    /// # Arguments
    /// * `context` - the bridge context.
    ///
    /// # Returns
    /// 0 to continue; nonzero to stop the running JavaScript.
    pub(super) fn jbq_debug_poll(context: *mut Context) -> c_int;
    /// lunarstorm patch: forwards one rejection-tracker event of a wrapped context to the
    /// debugger's rejection hook.
    ///
    /// # Arguments
    /// * `context` - the bridge context.
    /// * `promise`, `reason` - the engine's `JSValue`s, borrowed.
    /// * `handled` - 1 when a handler was attached to a promise reported earlier.
    pub(super) fn jbq_debug_rejection(context: *mut Context, promise: *const c_void, reason: *const c_void, handled: c_int);
    /// The engine `JSContext` under the bridge, with one new reference.
    ///
    /// # Arguments
    /// * `context` - the bridge context.
    ///
    /// # Returns
    /// The `JSContext`; the caller releases the reference with `JS_FreeContext`.
    pub(super) fn jbq_js_context(context: *mut Context) -> *mut c_void;
    pub(super) fn jbq_set_entry(context: *mut Context, entry: *mut c_void);
    pub(super) fn jbq_get_entry(context: *mut Context) -> *mut c_void;
    pub(super) fn jbq_gc(context: *mut Context);
    /// Reads the runtime heap's size and ceiling (see `jbq_heap_usage` in bridge.c).
    ///
    /// # Arguments
    /// * `context` - Live bridge context, owner thread only.
    /// * `used` - Receives the allocated bytes including allocator overhead.
    /// * `limit` - Receives the memory limit; 0 when there is none.
    pub(super) fn jbq_heap_usage(context: *mut Context, used: *mut i64, limit: *mut i64);
    /// Nonallocating take/reset of sticky OOM metadata, including null fallback.
    pub(super) fn jbq_take_oom(context: *mut Context) -> c_int;
    /// The address of the sticky out-of-memory flag `jbq_take_oom` takes: one byte, nonzero
    /// while set, valid until the context is freed.
    ///
    /// # Arguments
    /// * `context` - Live bridge context.
    ///
    /// # Returns
    /// The flag's address.
    pub(super) fn jbq_oom_flag(context: *mut Context) -> *const u8;
    /// Private native policy/fault-shaping hook; does not execute JavaScript.
    pub(super) fn jbq_set_memory_limit(context: *mut Context, bytes: usize);
    pub(super) fn jbq_version() -> *const c_char;
    pub(super) fn jbq_dup(context: *mut Context, value: Value) -> Value;
    pub(super) fn jbq_free(context: *mut Context, value: Value);
    pub(super) fn jbq_global(context: *mut Context) -> Value;
    pub(super) fn jbq_undefined() -> Value;
    pub(super) fn jbq_null() -> Value;
    pub(super) fn jbq_bool(context: *mut Context, value: c_int) -> Value;
    pub(super) fn jbq_number(context: *mut Context, value: f64) -> Value;
    pub(super) fn jbq_string(context: *mut Context, value: *const u16, length: usize) -> Value;
    pub(super) fn jbq_bytes(context: *mut Context, value: *const u8, length: usize) -> Value;
    pub(super) fn jbq_array(context: *mut Context) -> Value;
    pub(super) fn jbq_record(context: *mut Context) -> Value;
    pub(super) fn jbq_kind(context: *mut Context, value: Value) -> c_int;
    pub(super) fn jbq_is_exception(value: Value) -> c_int;
    /// Inspects the original thrown value without coercion or application code.
    pub(super) fn jbq_is_uncatchable(value: Value) -> c_int;
    pub(super) fn jbq_take_exception(context: *mut Context) -> Value;
    pub(super) fn jbq_to_bool(context: *mut Context, value: Value) -> c_int;
    pub(super) fn jbq_to_number(context: *mut Context, value: Value, out: *mut f64) -> c_int;
    pub(super) fn jbq_string_data(
        context: *mut Context,
        value: Value,
        length: *mut usize,
    ) -> *const u16;
    pub(super) fn jbq_string_free(context: *mut Context, data: *const u16);
    /// Borrows a primitive string's UTF-8 (WTF-8 for unpaired surrogates); null with a
    /// pending exception for a non-string. Release with [`jbq_utf8_free`].
    ///
    /// # Arguments
    /// * `context` - The live context.
    /// * `value` - A primitive string.
    /// * `length` - Receives the byte length.
    pub(super) fn jbq_string_utf8(
        context: *mut Context,
        value: Value,
        length: *mut usize,
    ) -> *const c_char;
    /// Releases the bytes of [`jbq_string_utf8`].
    ///
    /// # Arguments
    /// * `context` - The context the bytes came from.
    /// * `data` - The bytes.
    pub(super) fn jbq_utf8_free(context: *mut Context, data: *const c_char);
    /// A new string from UTF-8 bytes.
    ///
    /// # Arguments
    /// * `context` - The live context.
    /// * `value` - The bytes.
    /// * `length` - Their length.
    pub(super) fn jbq_string_from_utf8(
        context: *mut Context,
        value: *const c_char,
        length: usize,
    ) -> Value;
    /// An owned atom for a UTF-16 property name; 0 with a pending exception on failure.
    ///
    /// # Arguments
    /// * `context` - The live context.
    /// * `value` - The name's code units.
    /// * `length` - Their count.
    pub(super) fn jbq_atom(context: *mut Context, value: *const u16, length: usize) -> u32;
    /// Releases an atom of [`jbq_atom`].
    ///
    /// # Arguments
    /// * `context` - The context the atom belongs to.
    /// * `atom` - The atom.
    pub(super) fn jbq_atom_free(context: *mut Context, atom: u32);
    /// `object[atom]`, as [`jbq_get`].
    ///
    /// # Arguments
    /// * `context` - The live context.
    /// * `object` - The borrowed object.
    /// * `atom` - A borrowed atom of [`jbq_atom`].
    pub(super) fn jbq_get_atom(context: *mut Context, object: Value, atom: u32) -> Value;
    /// `object[atom] = value`, as [`jbq_set`].
    ///
    /// # Arguments
    /// * `context` - The live context.
    /// * `object` - The borrowed object.
    /// * `atom` - A borrowed atom of [`jbq_atom`].
    /// * `value` - The borrowed value.
    pub(super) fn jbq_set_atom(context: *mut Context, object: Value, atom: u32, value: Value)
    -> c_int;
    pub(super) fn jbq_get(context: *mut Context, object: Value, key: Value) -> Value;
    pub(super) fn jbq_set(context: *mut Context, object: Value, key: Value, value: Value) -> c_int;
    pub(super) fn jbq_get_index(context: *mut Context, object: Value, index: u32) -> Value;
    pub(super) fn jbq_define_index(
        context: *mut Context,
        object: Value,
        index: u32,
        value: Value,
    ) -> c_int;
    pub(super) fn jbq_has_own_index(context: *mut Context, object: Value, index: u32) -> c_int;
    pub(super) fn jbq_length(context: *mut Context, object: Value, length: *mut i64) -> c_int;
    pub(super) fn jbq_define_key(
        context: *mut Context,
        object: Value,
        key: Value,
        value: Value,
    ) -> c_int;
    pub(super) fn jbq_call(
        context: *mut Context,
        function: Value,
        receiver: Value,
        args: *const Value,
        argc: usize,
    ) -> Value;
    pub(super) fn jbq_construct(
        context: *mut Context,
        function: Value,
        target: Value,
        args: *const Value,
        argc: usize,
    ) -> Value;
    /// Consumes opaque/drop exactly once, including allocation failures.
    pub(super) fn jbq_host_function(
        context: *mut Context,
        name: Value,
        arity: u16,
        callback: HostCallback,
        opaque: *mut c_void,
        drop: DropCallback,
    ) -> Value;
    /// Borrowed result; index must be within callback argc; duplicate to own.
    pub(super) fn jbq_argv_get(argv: *const c_void, index: usize) -> Value;
    pub(super) fn jbq_deferred(context: *mut Context, pair: *mut Value) -> Value;
    pub(super) fn jbq_settle(
        context: *mut Context,
        pair: Value,
        reject: c_int,
        value: Value,
    ) -> Value;
    pub(super) fn jbq_set_job_sink(
        context: *mut Context,
        sink: Option<JobSink>,
        opaque: *mut c_void,
    );
    pub(super) fn jbq_run_job(context: *mut Context, job: *mut Job) -> c_int;
    pub(super) fn jbq_drop_job(context: *mut Context, job: *mut Job);
    pub(super) fn jbq_enqueue_job(context: *mut Context, function: Value) -> c_int;
    pub(super) fn jbq_async_context(context: *mut Context) -> Value;
    pub(super) fn jbq_set_async_context(context: *mut Context, value: Value);
    pub(super) fn jbq_take_rejection(
        context: *mut Context,
        promise: *mut Value,
        reason: *mut Value,
    ) -> c_int;
    pub(super) fn jbq_detach_buffer(context: *mut Context, value: Value) -> c_int;
    pub(super) fn jbq_internals(
        context: *mut Context,
        value: Value,
        first: *mut Value,
        second: *mut Value,
    ) -> c_int;
    pub(super) fn jbq_eval(
        context: *mut Context,
        source: *const u16,
        length: usize,
        origin: *const c_char,
    ) -> Value;
    pub(super) fn jbq_compile(
        context: *mut Context,
        source: *const u16,
        length: usize,
        origin: *const c_char,
    ) -> Value;
    pub(super) fn jbq_write(context: *mut Context, value: Value, length: *mut usize) -> *mut u8;
    /// Trusted own compiler output only; QuickJS does not validate hostile bytecode.
    pub(super) fn jbq_read(context: *mut Context, bytes: *const u8, length: usize) -> Value;
    pub(super) fn jbq_eval_compiled(context: *mut Context, compiled: Value) -> Value;
    pub(super) fn jbq_set_compiler(
        context: *mut Context,
        compiler: Option<Compiler>,
        opaque: *mut c_void,
    ) -> c_int;
    pub(super) fn jbq_eval_environment(
        context: *mut Context,
        lease: u64,
        out: *mut EnvView,
    ) -> c_int;
    pub(super) fn jbq_eval_binding(
        context: *mut Context,
        lease: u64,
        kind: u32,
        index: u32,
        out: *mut EnvBinding,
    ) -> c_int;
    pub(super) fn jbq_eval_binding_name(
        context: *mut Context,
        lease: u64,
        kind: u32,
        index: u32,
    ) -> Value;
    /// Unsafe: bytes must be a Rust-verified image of this pinned target.
    pub(super) fn jbq_install_verified(
        context: *mut Context,
        bytes: *const u8,
        length: usize,
        prepare: c_int,
    ) -> Value;
    pub(super) fn jbq_throw_compile(
        context: *mut Context,
        kind: c_int,
        message: *const c_char,
    ) -> Value;
    pub(super) fn jbq_buffer_free(context: *mut Context, buffer: *mut c_void);
    pub(super) fn jbq_own_keys(
        context: *mut Context,
        object: Value,
        length: *mut usize,
    ) -> *mut Keys;
    pub(super) fn jbq_key(context: *mut Context, keys: *const Keys, index: usize) -> Value;
    pub(super) fn jbq_key_enumerable(keys: *const Keys, index: usize) -> c_int;
    pub(super) fn jbq_keys_free(context: *mut Context, keys: *mut Keys);
    pub(super) fn jbq_same_object(a: Value, b: Value) -> c_int;
    pub(super) fn jbq_is_array(value: Value) -> c_int;
    pub(super) fn jbq_is_plain_record(context: *mut Context, value: Value) -> c_int;
    /// Returned pointer borrowed until next JS entry; copy immediately. 1 byte
    /// storage, 0 unsupported, -1 exception. Null with length zero is valid.
    pub(super) fn jbq_bytes_data(
        context: *mut Context,
        value: Value,
        bytes: *mut *const u8,
        length: *mut usize,
    ) -> c_int;
    pub(super) fn jbq_error(context: *mut Context, message: Value) -> Value;
    pub(super) fn jbq_error_field(context: *mut Context, value: Value, stack: c_int) -> Value;
}

unsafe extern "C" {
    pub(super) fn jbq_module_prepare(
        c: *mut Context,
        s: *const u16,
        n: usize,
        origin: *const c_char,
    ) -> Value;
    pub(super) fn jbq_module_count(c: *mut Context, v: Value) -> c_int;
    pub(super) fn jbq_module_import(c: *mut Context, v: Value, i: c_int) -> Value;
    pub(super) fn jbq_module_bind(c: *mut Context, v: Value, i: c_int, target: Value) -> c_int;
    pub(super) fn jbq_module_link(c: *mut Context, v: Value) -> c_int;
    pub(super) fn jbq_module_namespace(c: *mut Context, v: Value) -> Value;
    pub(super) fn jbq_promise_state(c: *mut Context, v: Value) -> c_int;
    pub(super) fn jbq_promise_result(c: *mut Context, v: Value) -> Value;
    pub(super) fn jbq_module_write(c: *mut Context, v: Value, n: *mut usize) -> *mut u8;
    pub(super) fn jbq_module_cache_write(c: *mut Context, v: Value, n: *mut usize) -> *mut u8;
    pub(super) fn jbq_module_read(
        c: *mut Context,
        bytes: *const u8,
        n: usize,
        identity: Value,
    ) -> Value;
    pub(super) fn jbq_module_builtin(
        c: *mut Context,
        identity: *const c_char,
        exports: Value,
    ) -> Value;
}

unsafe extern "C" {
    pub(super) fn jbq_import_take(c: *mut Context, base: *mut Value, specifier: *mut Value) -> u64;
    pub(super) fn jbq_import_finish(
        c: *mut Context,
        id: u64,
        module: Value,
        evaluation: Value,
        failure: *const c_char,
    ) -> c_int;
}

/// Owner-thread hook; callbacks contain panics and never transfer native state.
pub(super) type DebugHook = unsafe extern "C" fn(*mut c_void, *mut Context, c_int) -> c_int;
pub(super) type DebugMarker = unsafe extern "C" fn(*mut c_void, usize, u32, c_int, c_int, c_int);
pub(super) type DebugVariable = unsafe extern "C" fn(*mut c_void, Value, Value, c_int, c_int);
#[repr(C)]
#[derive(Default)]
pub(super) struct DebugFrame {
    pub(super) function: usize,
    pub(super) activation: u64,
    pub(super) marker: u32,
    pub(super) line: c_int,
    pub(super) column: c_int,
    pub(super) scope_known: c_int,
}
pub(super) type DebugRejectionHook =
    unsafe extern "C" fn(*mut c_void, *mut Context, Value, Value, c_int, u64);
pub(super) type DebugAsyncHook = unsafe extern "C" fn(*mut c_void, *mut Context, c_int, u64) -> u64;
// SAFETY: signatures match the owner-thread debugger ABI in bridge.h.
unsafe extern "C" {
    pub(super) fn jbq_debug_install(c: *mut Context, hook: Option<DebugHook>, opaque: *mut c_void);
    pub(super) fn jbq_debug_rejection_install(
        c: *mut Context,
        hook: Option<DebugRejectionHook>,
        opaque: *mut c_void,
    );
    pub(super) fn jbq_debug_describe_exception(
        c: *mut Context,
        value: Value,
        info: *mut DebugValueInfo,
    ) -> c_int;
    pub(super) fn jbq_debug_promise_identity(c: *mut Context, value: Value) -> usize;
    pub(super) fn jbq_debug_async_install(
        c: *mut Context,
        hook: Option<DebugAsyncHook>,
        opaque: *mut c_void,
    );
    pub(super) fn jbq_debug_async_capture(c: *mut Context, kind: c_int) -> u64;
    pub(super) fn jbq_debug_async_stack(c: *mut Context) -> u64;
    pub(super) fn jbq_debug_async_set(c: *mut Context, id: u64) -> u64;
    pub(super) fn jbq_debug_frame_metadata(
        c: *mut Context,
        level: c_int,
        frame: *mut DebugFrame,
    ) -> c_int;
    pub(super) fn jbq_debug_flags(c: *mut Context, flags: c_int);
    pub(super) fn jbq_debug_exception_policy(c: *mut Context, policy: c_int);
    pub(super) fn jbq_debug_exception(
        c: *mut Context,
        value: *mut Value,
        caught: *mut c_int,
    ) -> c_int;
    pub(super) fn jbq_debug_step_info(
        c: *mut Context,
        expected: u64,
        activation: *mut u64,
        kind: *mut c_int,
        parent: *mut u64,
    ) -> c_int;
    pub(super) fn jbq_debug_frame_count(c: *mut Context) -> c_int;
    pub(super) fn jbq_debug_uninitialized(value: Value) -> c_int;
    pub(super) fn jbq_debug_markers(
        c: *mut Context,
        root: Value,
        callback: DebugMarker,
        opaque: *mut c_void,
    ) -> c_int;
    pub(super) fn jbq_debug_breakpoint(
        c: *mut Context,
        function: usize,
        marker: u32,
        enabled: c_int,
    ) -> c_int;
    pub(super) fn jbq_debug_frame_at(
        c: *mut Context,
        level: c_int,
        out: *mut DebugFrame,
        name: *mut Value,
        origin: *mut Value,
    ) -> c_int;
    pub(super) fn jbq_debug_variables(
        c: *mut Context,
        level: c_int,
        callback: DebugVariable,
        opaque: *mut c_void,
    ) -> c_int;
    pub(super) fn jbq_debug_stop(c: *mut Context) -> Value;
    pub(super) fn jbq_take_debug_stop(c: *mut Context) -> c_int;
    pub(super) fn jbq_debug_eval_verified(
        c: *mut Context,
        level: c_int,
        source: *const u16,
        units: usize,
        filename: *const c_char,
        out: *mut Value,
    ) -> c_int;
    pub(super) fn jbq_debug_unwinding(c: *mut Context) -> c_int;
}

/// Own-property callback; borrowed primitive key and descriptor value. Never unwinds.
pub(super) type DebugProperty =
    unsafe extern "C" fn(*mut c_void, *mut Context, Value, Value, c_int);
// SAFETY: these signatures match the bounded non-executing bridge.h API.
unsafe extern "C" {
    pub(super) fn jbq_debug_object_class(c: *mut Context, value: Value) -> c_int;
    pub(super) fn jbq_debug_properties(
        c: *mut Context,
        value: Value,
        start: usize,
        count: usize,
        callback: DebugProperty,
        opaque: *mut c_void,
        total: *mut usize,
    ) -> c_int;
}

// SAFETY: artifact helpers match bridge.h and use the same owner/value lifetimes.
unsafe extern "C" {
    pub(super) fn jbq_compile_debug(
        c: *mut Context,
        source: *const u16,
        units: usize,
        origin: *const c_char,
    ) -> Value;
    pub(super) fn jbq_write_debug(c: *mut Context, compiled: Value, len: *mut usize) -> *mut u8;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct DebugCursor {
    pub position: usize,
    pub skip_remaining: usize,
    pub collection_link: usize,
    pub phase: c_int,
    pub index: c_int,
    pub depth: c_int,
    pub last_level: c_int,
    pub skip_args: c_int,
}
#[repr(C)]
pub(super) struct DebugValueInfo {
    pub type_name: [u8; 32],
    pub text: [u16; 512],
    pub length: usize,
    pub expandable: c_int,
}
// SAFETY: raw info layout matches the fixed bridge.h structure.
unsafe extern "C" {
    pub(super) fn jbq_debug_value_info_get(
        c: *mut Context,
        value: Value,
        out: *mut DebugValueInfo,
    ) -> c_int;
}
// SAFETY: cursor layout and borrowed visitor match bridge.h exactly.
unsafe extern "C" {
    pub(super) fn jbq_debug_exception_description(
        c: *mut Context,
        out: *mut DebugValueInfo,
    ) -> c_int;
    pub(super) fn jbq_debug_cursor_retain(cursor: *mut DebugCursor);
    pub(super) fn jbq_debug_cursor_release(c: *mut Context, cursor: *mut DebugCursor);
    pub(super) fn jbq_debug_inspect(
        c: *mut Context,
        object: Value,
        frame: c_int,
        scope: u32,
        cursor: *mut DebugCursor,
        count: usize,
        scan: usize,
        callback: DebugPageEntry,
        opaque: *mut c_void,
        scanned: *mut usize,
        complete: *mut c_int,
    ) -> c_int;
    pub(super) fn jbq_debug_copy_string(
        value: Value,
        out: *mut u16,
        cap: usize,
        length: *mut usize,
    ) -> c_int;
}

/// Stoppable borrowed native page entry visitor; zero leaves the entry for the next page.
pub(super) type DebugPageEntry =
    unsafe extern "C" fn(*mut c_void, *mut Context, Value, Value, c_int, u32, u32, u32) -> c_int;

// SAFETY: targets are owner-local and own their roots/atoms until freed.
unsafe extern "C" {
    pub(super) fn jbq_debug_target_new(
        c: *mut Context,
        object: Value,
        frame: c_int,
        scope: u32,
        atom: u32,
        kind: u32,
        index: u32,
        status: *mut c_int,
    ) -> *mut c_void;
    pub(super) fn jbq_debug_target_free(c: *mut Context, target: *mut c_void);
    pub(super) fn jbq_debug_target_validate(c: *mut Context, target: *mut c_void) -> c_int;
    pub(super) fn jbq_debug_target_store(
        c: *mut Context,
        target: *mut c_void,
        value: Value,
    ) -> c_int;
}

#[repr(C)]
pub(super) struct LookupKey {
    pub text: *const u16,
    pub units: usize,
    pub index: u32,
    pub is_index: u32,
}
// SAFETY: private ABI matches bridge.h; keys borrow parsed UTF-16 for this call.
unsafe extern "C" {
    pub(super) fn jbq_debug_lookup(
        c: *mut Context,
        level: c_int,
        root_this: c_int,
        root: *const u16,
        units: usize,
        keys: *const LookupKey,
        count: usize,
        out: *mut Value,
    ) -> c_int;
}
