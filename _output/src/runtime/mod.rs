//! AppRuntime: the interface between the native host and the runtime that runs the app's
//! logic on the other side of the IPC envelope.
//!
//! The host validates every WebView message against the form's table and then hands it
//! to the runtime; the runtime answers with updates and responses. Which runtime the host
//! embeds is chosen at build time by `build.rs` from `ZEBRA_APP_RUNTIME`:
//!
//! | value           | runtime                                                        |
//! |-----------------|----------------------------------------------------------------|
//! | `quickjs`       | default: the QuickJS JS thread ([`crate::js::JsThread`])       |
//! | `mock`          | [`mock::MockRuntime`]: logs what arrives over IPC, no app logic |
//! | path to a `.rs` | that file, compiled as module `selected::external`             |
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
//! - Native calls go through [`RuntimeContext::natives`], which checks permissions.

pub mod mock;

use std::sync::Arc;

use crate::js::{JsThread, NativeRouter};

/// Host -> runtime message (the JS thread's message type is the interface's).
pub use crate::js::ToJs as HostMessage;
/// Runtime -> host message.
pub use crate::js::FromJs as RuntimeMessage;

/// What the host gives a runtime when it creates it.
#[derive(Clone)]
pub struct RuntimeContext {
    /// Receives every message for the host; callable from any thread.
    pub out: Arc<dyn Fn(RuntimeMessage) + Send + Sync>,
    /// Native service router (permissions are checked there).
    pub natives: Arc<dyn NativeRouter>,
}

/// A runtime the host embeds. Dropping it stops it.
pub trait AppRuntime {
    /// Short name for logs, e.g. `quickjs` or `mock`.
    ///
    /// # Returns
    /// The name.
    fn name(&self) -> &'static str;

    /// Queues a message without blocking the UI thread.
    ///
    /// # Arguments
    /// * `message` - the message.
    ///
    /// # Errors
    /// The runtime cannot take it (queue full, stopped); the host reports it as a
    /// runtime error.
    fn send(&self, message: HostMessage) -> Result<(), String>;

    /// Queues a message that must not be lost (`Load`, `Stop`), waiting for room.
    ///
    /// # Arguments
    /// * `message` - the message.
    fn send_blocking(&self, message: HostMessage);
}

impl AppRuntime for JsThread {
    /// # Returns
    /// `quickjs`.
    fn name(&self) -> &'static str {
        "quickjs"
    }

    /// Forwards to [`JsThread::send`].
    ///
    /// # Arguments
    /// * `message` - the message.
    ///
    /// # Errors
    /// The JS queue is full or the thread is gone.
    fn send(&self, message: HostMessage) -> Result<(), String> {
        JsThread::send(self, message)
    }

    /// Forwards to [`JsThread::send_blocking`].
    ///
    /// # Arguments
    /// * `message` - the message.
    fn send_blocking(&self, message: HostMessage) {
        JsThread::send_blocking(self, message)
    }
}

/// The runtime `build.rs` selected; generated code defines `NAME` and `create`.
pub mod selected {
    #[allow(unused_imports)]
    use super::{mock::MockRuntime, AppRuntime, JsThread, RuntimeContext};
    include!(concat!(env!("OUT_DIR"), "/selected_runtime.rs"));
}

/// Creates the runtime selected at build time.
///
/// # Arguments
/// * `ctx` - output sink and native router.
///
/// # Returns
/// The runtime.
pub fn create(ctx: RuntimeContext) -> Box<dyn AppRuntime> {
    selected::create(ctx)
}
