//! AppRuntime: the interface between the native host and the runtime that runs the app's
//! logic on the other side of the IPC envelope.
//!
//! The host validates every WebView message against the form's table and then hands it
//! to the runtime; the runtime answers with updates and responses. Which runtime the host
//! embeds is chosen at build time by `build.rs` from `LUNARSTORM_APP_RUNTIME`:
//!
//! | value           | runtime                                                              |
//! |-----------------|----------------------------------------------------------------------|
//! | `lunarstorm`    | default: [`lunarstorm::LunarstormRuntime`], the Lunarstorm Runtime    |
//! | `mock`          | [`mock::MockRuntime`]: logs what arrives over IPC, no app logic       |
//! | path to a `.rs` | that file, compiled as module `selected::external`                   |
//!
//! An external file must define
//! `pub fn create(ctx: crate::runtime::RuntimeContext) -> Box<dyn crate::runtime::AppRuntime>`.
//!
//! Contract (provisional; the interface is not final):
//! - Host -> runtime: [`HostMessage`]. `Load` starts or replaces the app for an epoch;
//!   `Envelope` carries a validated `event`, `request` or `cancel`; `Stop` ends it (the
//!   host also drops the runtime at shutdown).
//! - Runtime -> host: [`RuntimeMessage`] through [`RuntimeContext::out`]. The runtime must
//!   report `Loaded` for every `Load`: the host mounts the main form only after it. Every
//!   `request` gets a `response` with the same id, or the host cancels it at the deadline.
//!   Messages stamped with a retired epoch are dropped by the host.
//! - Native calls go through [`RuntimeContext::natives`] after the runtime has checked
//!   them against the host configuration's permissions.
//!
//! Decision: the message and native-call types live here, with the interface, rather
//! than in a runtime implementation. They were the former JS thread's (`ToJs`,
//! `FromJs`); that thread is archived and every runtime now shares them.

pub mod lunarstorm;
pub mod mock;
#[cfg(test)]
mod tests;

use crate::envelope::Envelope;
use crate::manifest::HostConfig;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;

/// Host -> runtime message.
#[derive(Debug)]
pub enum HostMessage {
    /// Start (or replace) the app with a backend artifact.
    Load {
        /// Backend artifact source.
        backend: Arc<str>,
        /// Host configuration (permissions, limits).
        config: Arc<HostConfig>,
        /// Carried-over control values, `{Form: {Control: {prop: value}}}`.
        restore: Value,
        /// Epoch of the new instance.
        epoch: u64,
    },
    /// An event, request or cancel for the backend.
    Envelope(Envelope),
    /// Shut the runtime down.
    Stop,
}

/// Runtime -> host message.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeMessage {
    /// An `update` or `response` from the backend, stamped with its epoch.
    Envelope(Envelope),
    /// The instance for `epoch` finished loading (or failed to).
    Loaded {
        /// Epoch of the instance.
        epoch: u64,
        /// The load error, if any.
        error: Option<String>,
    },
    /// An uncaught exception, rejected promise or exceeded budget in the backend.
    Error {
        /// Epoch it happened in.
        epoch: u64,
        /// Description with stack.
        message: String,
    },
    /// A `console` line.
    Log {
        /// `info`, `debug`, `warn` or `error`.
        level: String,
        /// The text.
        message: String,
    },
}

/// Delivers a native call's result to the runtime that made it: `(call id, epoch, result)`.
pub type Settle = Arc<dyn Fn(u64, u64, Result<Value, String>) + Send + Sync>;

/// Finishes one native call; it can be moved to any thread.
#[derive(Clone)]
pub struct Completer {
    /// Where the result goes; the runtime drops results for retired epochs.
    settle: Settle,
    /// Call id.
    id: u64,
    /// Epoch of the caller.
    pub epoch: u64,
}

impl Completer {
    /// Creates the completer of call `id` made by the backend of `epoch`.
    pub fn new(settle: Settle, id: u64, epoch: u64) -> Completer {
        Completer { settle, id, epoch }
    }

    /// Delivers the result. `Ok(Value::Null)` reaches the backend as `undefined`.
    pub fn complete(self, result: Result<Value, String>) {
        (self.settle)(self.id, self.epoch, result);
    }
}

impl fmt::Debug for Completer {
    /// Shows the call, not the closure.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completer").field("id", &self.id).field("epoch", &self.epoch).finish()
    }
}

/// One `native.<ns>.<method>(...)` call from the backend.
#[derive(Debug)]
pub struct NativeCall {
    /// Service namespace (`window`, `dialog`, ...).
    pub ns: String,
    /// Method name.
    pub method: String,
    /// Arguments.
    pub args: Vec<Value>,
    /// Where the result goes.
    pub done: Completer,
}

/// Routes native calls to the thread that may run them (UI thread, worker pool, inline).
pub trait NativeRouter: Send + Sync {
    /// Starts one permitted call.
    fn call(&self, call: NativeCall);

    /// Releases every native resource the backend of `epoch` created (6.2).
    fn release_epoch(&self, epoch: u64);
}

/// What the host gives a runtime when it creates it.
#[derive(Clone)]
pub struct RuntimeContext {
    /// Receives every message for the host; callable from any thread.
    pub out: Arc<dyn Fn(RuntimeMessage) + Send + Sync>,
    /// Native service router.
    pub natives: Arc<dyn NativeRouter>,
}

/// A runtime the host embeds. Dropping it stops it.
pub trait AppRuntime {
    /// Short name for logs, e.g. `lunarstorm` or `mock`.
    fn name(&self) -> &'static str;

    /// Queues a message without blocking the UI thread.
    ///
    /// # Errors
    /// The runtime cannot take it (stopped); the host reports it as a runtime error.
    fn send(&self, message: HostMessage) -> Result<(), String>;

    /// Queues a message that must not be lost (`Load`, `Stop`).
    fn send_blocking(&self, message: HostMessage);
}

/// The runtime `build.rs` selected; generated code defines `NAME` and `create`.
pub mod selected {
    #[allow(unused_imports)]
    use super::{lunarstorm::LunarstormRuntime, mock::MockRuntime, AppRuntime, RuntimeContext};
    include!(concat!(env!("OUT_DIR"), "/selected_runtime.rs"));
}

/// Creates the runtime selected at build time.
pub fn create(ctx: RuntimeContext) -> Box<dyn AppRuntime> {
    selected::create(ctx)
}
