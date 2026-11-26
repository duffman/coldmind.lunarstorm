//! What an adapter reports about its engine ([`Descriptor`], [`Capabilities`])
//! and how a host bounds it: [`Limits`], [`Heap`] statistics and the
//! cross-thread [`Interrupt`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
