// Copyright (c) 2026 Coldmind AB
// SPDX-License-Identifier: MIT
//! The Rust compiler (crates/qjs-*) as the context's only JavaScript compiler.
//!
//! Registered on every context with `jbq_set_compiler`, so scripts, modules,
//! indirect eval, `new Function` and direct/debugger eval all compile here and
//! install only images that passed the Rust image verifier.
//!
//! lunarstorm patch: the compiler itself moved to `rquickjs_core::compiler`,
//! because with the C parser removed every rquickjs context needs it, not only
//! the ones this crate wraps. jetbunny's text is there unchanged apart from
//! marked patches; this file keeps jetbunny's `install` entry point and the
//! debugger backend's side of the calls the compiler makes
//! ([`CompileObserver`] for [`QuickJsDebug`]). The moved text is archived at
//! `.archive/2026-11-24/runtime/debugger/src/engines/quickjs_ng/compiler.rs`.
use super::debug::QuickJsDebug;
use super::ffi;
use crate::debug::ScriptKind;
use rquickjs_core::compiler::{CompileObserver, SourceKind, Value};
#[allow(unused_imports)]
use std::ffi::{CString, c_char, c_int, c_void};

/// Install the Rust compiler on a freshly created context.
///
/// # Arguments
/// * `context` - the bridge context.
/// * `opaque` - the `QuickJsDebug` backend, or null for none.
///
/// # Returns
/// `true` when installed.
///
/// # Safety
/// `context` is a live context owned by the calling thread; a non-null
/// `opaque` is a `QuickJsDebug` that outlives the installed compiler.
pub(super) unsafe fn install(context: *mut ffi::Context, opaque: *mut c_void) -> bool {
    // SAFETY: same bridge context type behind both declarations; caller owns it.
    unsafe {
        rquickjs_core::compiler::install::<QuickJsDebug>(context.cast(), opaque.cast::<QuickJsDebug>())
    }
}

/// jetbunny's compiler callback, for the boundary tests below that drive it
/// directly.
///
/// # Safety
/// As `rquickjs_core::compiler::compile`.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn compile(
    opaque: *mut c_void,
    context: *mut ffi::Context,
    lease: u64,
    source: *const u16,
    units: usize,
    filename: *const c_char,
    line: c_int,
    flags: c_int,
) -> ffi::Value {
    // SAFETY: forwarded unchanged; both declarations name the same C types.
    let value = unsafe {
        rquickjs_core::compiler::compile::<QuickJsDebug>(
            opaque,
            context.cast(),
            lease,
            source,
            units,
            filename,
            line,
            flags,
        )
    };
    ffi::Value { words: value.words }
}

/// The debugger backend as the compiler's observer: the calls jetbunny's
/// compiler made on `QuickJsDebug` directly.
impl CompileObserver for QuickJsDebug {
    type Registration = crate::debug::ScriptRegistration;
    /// Counts compiles for the lookup tests (jetbunny's `#[cfg(test)]` counter).
    fn compiling(&self) {
        #[cfg(test)]
        self.lookup_observation
            .compilations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn is_development(&self) -> bool {
        QuickJsDebug::is_development(self)
    }
    fn register_source(
        &self,
        source: &[u16],
        name: &str,
        kind: SourceKind,
    ) -> Result<Option<Self::Registration>, String> {
        let kind = match kind {
            SourceKind::Script => ScriptKind::Script,
            SourceKind::Module => ScriptKind::Module,
            SourceKind::Eval => ScriptKind::Eval,
            SourceKind::Function => ScriptKind::Function,
        };
        QuickJsDebug::register_source(self, source, name, kind).map_err(|error| error.to_string())
    }
    fn loaded(&self, root: Value, registration: Option<Self::Registration>) -> Result<(), String> {
        QuickJsDebug::loaded(self, ffi::Value { words: root.words }, registration)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod controlled_boundary_tests {
    use super::*;
    use crate::{engine::*, engines::quickjs_ng::QuickJsNgEngine};
    use std::cell::Cell;

    struct Fixture {
        compiling_finished: Cell<bool>,
        evaluating: Cell<bool>,
        cause: i32,
        status: Cell<i32>,
    }
    unsafe extern "C" fn after_compile(
        opaque: *mut c_void,
        context: *mut ffi::Context,
        lease: u64,
        source: *const u16,
        units: usize,
        filename: *const c_char,
        line: c_int,
        flags: c_int,
    ) -> ffi::Value {
        // SAFETY: forward the live synchronous frame lease and source to the
        // actual production Rust compiler; no alternate parser/image is used.
        let image = unsafe {
            compile(
                std::ptr::null_mut(),
                context,
                lease,
                source,
                units,
                filename,
                line,
                flags,
            )
        };
        // SAFETY: the returned image belongs to this live context; tag inspection
        // executes no JavaScript. A compiler failure must fail this regression.
        let succeeded = unsafe { ffi::jbq_is_exception(image) } == 0;
        // SAFETY: the owner retains Fixture for this synchronous callback. Cells
        // permit hook reentry without a mutable borrow across native calls.
        unsafe { &*opaque.cast::<Fixture>() }
            .compiling_finished
            .set(succeeded);
        image
    }
    unsafe extern "C" fn hook(opaque: *mut c_void, context: *mut ffi::Context, event: i32) -> i32 {
        // SAFETY: test owns this shared fixture until all synchronous hooks end.
        let fixture = unsafe { &*opaque.cast::<Fixture>() };
        if event == -1 {
            return if fixture.evaluating.get() && fixture.compiling_finished.get() {
                fixture.cause
            } else {
                0
            };
        }
        let source: Vec<u16> = "globalThis.firstInstruction=1".encode_utf16().collect();
        let name = CString::new("compiler-return.js").unwrap();
        let mut value = std::mem::MaybeUninit::uninit();
        fixture.evaluating.set(true);
        // SAFETY: selected paused frame, initialized text and writable out live
        // throughout compilation and its subsequent native control poll.
        let status = unsafe {
            ffi::jbq_debug_eval_verified(
                context,
                0,
                source.as_ptr(),
                source.len(),
                name.as_ptr(),
                value.as_mut_ptr(),
            )
        };
        fixture.evaluating.set(false);
        fixture.status.set(status);
        // SAFETY: native initialized/transferred its private evaluator value.
        unsafe { ffi::jbq_free(context, value.assume_init()) };
        0
    }
    #[test]
    fn compiler_return_poll_cancels_or_expires_before_first_evaluator_instruction() {
        for cause in [2, 3] {
            let engine = QuickJsNgEngine::new().unwrap();
            let id = engine.create_context(ContextLimits::default()).unwrap();
            let ptr = engine.context(id).unwrap().raw.ptr().unwrap();
            let source: Vec<u16> = "globalThis.firstInstruction=0;debugger;firstInstruction"
                .encode_utf16()
                .collect();
            let name = CString::new("application.js").unwrap();
            let fixture = Fixture {
                compiling_finished: Cell::new(false),
                evaluating: Cell::new(false),
                cause,
                status: Cell::new(-1),
            };
            let opaque = (&fixture as *const Fixture).cast_mut().cast();
            // SAFETY: every native call is owner-local; code/value ownership is
            // released before context close; fixture outlives installed hooks.
            unsafe {
                let code =
                    ffi::jbq_compile_debug(ptr, source.as_ptr(), source.len(), name.as_ptr());
                assert_eq!(ffi::jbq_is_exception(code), 0);
                assert_eq!(ffi::jbq_set_compiler(ptr, Some(after_compile), opaque), 0);
                ffi::jbq_debug_install(ptr, Some(hook), opaque);
                ffi::jbq_debug_flags(ptr, 1);
                let value = ffi::jbq_eval_compiled(ptr, code);
                assert_eq!(ffi::jbq_is_exception(value), 0);
                let mut number = -1.;
                assert_eq!(ffi::jbq_to_number(ptr, value, &mut number), 0);
                assert_eq!(number, 0.);
                assert!(fixture.compiling_finished.get());
                assert_eq!(fixture.status.get(), cause);
                ffi::jbq_free(ptr, value);
                ffi::jbq_free(ptr, code);
                ffi::jbq_debug_install(ptr, None, std::ptr::null_mut());
                ffi::jbq_set_compiler(ptr, None, std::ptr::null_mut());
            }
            engine.close_context(id).unwrap();
        }
    }
}
