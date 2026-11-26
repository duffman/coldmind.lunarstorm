//! The desktop app host (spec section 5): a desktop application's backend on the
//! Lunarstorm Runtime.
//!
//! ```text
//!  native desktop process (windows, menus, WebViews, native services)
//!        | Backend::load / dispatch / Settler::settle      ^ Shell
//!        v                                                 |
//!  +-----------------------------------------------------------+
//!  | this crate: backend thread, host services, bindings        |
//!  +-----------------------------------------------------------+
//!        | Runtime interface            ^ Host services
//!        v                              |
//!  lunarstorm-event-loop-runtime, bound to the QuickJS adapter
//! ```
//!
//! Decisions:
//! - **One backend thread, one Runtime per epoch.** A reload is a new Runtime,
//!   which is a new VM instance (spec 5, "Isolation"): heap, jobs, timers and
//!   pending native calls of the old backend go with it.
//! - **Events are posted tasks.** `dispatch` and `settle` post a task straight to
//!   the Runtime's loop from the calling thread; there is no second queue and no
//!   forwarding thread. The loop is held (`Runtime::hold`), so it sleeps in the
//!   kernel between events.
//! - **Reload and stop use a control channel**, because they replace the Runtime
//!   the tasks run in. The sender wakes the loop with an empty task.
//! - **The execution budget is a watchdog on the Runtime's pulse**, so it covers
//!   every JS entry (a dispatched event, a timer, a settled native call), not
//!   only the ones this crate starts.
//! - **A script error never ends the process** (`Host::uncaught`): it is reported
//!   to the shell and the backend goes on with the next event.
//! - **The shell contract carries text, not engine or desktop types.** Envelopes
//!   and native-call arguments cross as JSON text, exactly as they travel over
//!   IPC, so this library uses neither the desktop process's types nor a JSON
//!   crate. The `lunarstorm-desktop` binary (a tao event loop) is one shell.
//! - The VM is bound here, in one alias ([`Engine`]).
//! - **HostApi is installed with the areas a [`Program`] lists** ([`Fs`] so far).
//!   The manifest is built per backend from `Program.areas`, so an area the app
//!   is not permitted does not resolve. The backend's `call` dispatcher
//!   (`desktop_bridge_natives.rs`) answers WebView `call` envelopes from those
//!   areas, so an app needs no backend code for the WebView's calls.

use lunarstorm_event_loop_runtime::vm::{Call, Interrupt, Limits, Outcome, Param, Scope, Sig, Thrown, Ty, Vm};
use lunar_extension_hostapi::HostApi;
use lunarstorm_event_loop_runtime::{Access, Bindings, Entry, Host, Log, Manifest, Poster, Runtime, Source, Stream, Wait};
use lunarstorm_engine_quickjs_adapter::QuickJs;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The VM the desktop host is bound to.
pub type Engine = QuickJs;

/// Most bytes one `getentropy` call fills.
const ENTROPY_CHUNK: usize = 256;

/// What the native desktop process provides to the backend. Every method may be
/// called on the backend thread; none may block it for long.
pub trait Shell: Send + Sync + 'static {
    /// The backend of `epoch` finished loading, or failed to.
    ///
    /// # Arguments
    /// * `epoch` - the epoch given to [`Backend::load`].
    /// * `error` - why the backend did not load, if it did not.
    fn loaded(&self, epoch: u64, error: Option<&str>);

    /// An `update` or `response` envelope from the backend.
    ///
    /// # Arguments
    /// * `epoch` - the epoch of the backend that sent it.
    /// * `envelope` - the envelope as JSON, without `version` and `epoch`.
    fn send(&self, epoch: u64, envelope: &str);

    /// The backend called `native.<namespace>.<method>(...)`. The result goes to
    /// [`Settler::settle`] with the same `epoch` and `call`.
    ///
    /// # Arguments
    /// * `epoch` - the epoch of the calling backend.
    /// * `call` - identifies the call within its epoch.
    /// * `namespace` - the service.
    /// * `method` - the method.
    /// * `arguments` - the arguments as a JSON array.
    fn native(&self, epoch: u64, call: u64, namespace: &str, method: &str, arguments: &str);

    /// A script error: an exception out of a handler or a callback, an unhandled
    /// rejection, or an exceeded execution budget.
    ///
    /// # Arguments
    /// * `epoch` - the epoch it happened in.
    /// * `message` - the description, with the stack when there is one.
    fn error(&self, epoch: u64, message: &str);

    /// Output of the backend's `console`, and Runtime warnings.
    ///
    /// # Arguments
    /// * `level` - `info` (stdout), `error` (stderr) or `warn` (the Runtime).
    /// * `message` - the text, without the final newline.
    fn log(&self, level: &str, message: &str);

    /// The backend of `epoch` is gone; every native resource it created can be
    /// released (spec 6.2).
    ///
    /// # Arguments
    /// * `epoch` - the retired epoch.
    fn retired(&self, epoch: u64);
}

/// One backend to run: the artifact and what it may do.
#[derive(Debug, Clone)]
pub struct Program {
    /// Identifies this backend instance; stamped on everything it sends.
    pub epoch: u64,
    /// The backend artifact: one ES module importing only `@desktop/bridge` and
    /// `@desktop/api` (spec 4c).
    pub code: Arc<str>,
    /// Control values carried over from the previous instance, as JSON:
    /// `{Form: {Control: {prop: value}}}`.
    pub restore: String,
    /// Heap limit in bytes; `None` for the engine's default.
    pub memory: Option<usize>,
    /// Longest one JS entry may run before it is interrupted.
    pub budget: Duration,
    /// Filesystem roots the backend's own `node:fs` may read and write; nothing
    /// outside them.
    pub roots: Vec<PathBuf>,
    /// Filesystem roots of the HostApi area `fs` (what the WebView reaches).
    ///
    /// Defect fixed here: the area and the backend shared `roots`, so granting the
    /// WebView a folder (`permissions.fs`) also granted it to the backend's
    /// `node:fs` (test `the_backend_gets_no_fs_roots_from_permissions_fs`).
    /// Decision: two scopes; one shared scope was dropped because it widens every
    /// existing backend without a manifest change.
    pub area_roots: Vec<PathBuf>,
    /// The environment the backend sees as `process.env`.
    pub environment: Vec<(String, String)>,
    /// `process.argv[0]`: the application's id.
    pub name: String,
    /// The HostApi areas the app is permitted, e.g. `["fs"]`. An unknown name
    /// fails the load.
    pub areas: Vec<String>,
}

mod fs;

pub use fs::{Fs, MAX_TEXT};

/// The HostApi areas this host offers.
///
/// Decision D10: one list feeds both [`spec`] and the Runtime's extensions, so
/// the spec the platform checks calls against cannot drift from what is
/// installed. Two hand-kept lists were rejected.
pub type Areas = (Fs,);

/// The desktop host's whole API, every area it offers, as [`HostApi::spec`]
/// describes it. The platform checks WebView calls against it before they reach
/// the Runtime; the backend's dispatcher checks them again.
///
/// # Returns
/// The spec.
pub fn spec() -> serde_json::Value {
    HostApi::<Engine, Areas>::new((Fs,)).spec()
}

/// The Runtime manifest of a backend: HostApi with the permitted areas.
///
/// Decision: HostApi is always in the manifest, with no areas when none is
/// permitted, so `lunarstorm:hostapi` always resolves.
///
/// # Arguments
/// * `areas` - the permitted areas.
///
/// # Returns
/// The manifest.
pub(crate) fn manifest(areas: &[String]) -> Manifest {
    let hostapi = Entry { plugins: areas.to_vec(), config: None };
    Manifest { extensions: [(String::from(lunar_extension_hostapi::NAME), hostapi)].into() }
}

mod backend_instance_lifecycle;
mod desktop_bridge_natives;
mod desktop_host_services;

pub use backend_instance_lifecycle::{Backend, Settler};
pub use desktop_bridge_natives::declarations;
use desktop_bridge_natives::{bindings, clear, dispatch, hook, settle, Work};
use desktop_host_services::*;

