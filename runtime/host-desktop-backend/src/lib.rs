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

use lunarstorm_event_loop_runtime::vm::{Call, Interrupt, Limits, Outcome, Param, Scope, Sig, Thrown, Ty, Vm};
use lunarstorm_event_loop_runtime::{Access, Bindings, Host, Log, Poster, Runtime, Source, Stream, Wait};
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

/// The glue module's source.
const GLUE: &str = include_str!("../js/desktop.js");

/// Specifier of the glue module.
const GLUE_MODULE: &str = "lunarstorm:desktop";

/// Source of `@desktop/bridge`.
const BRIDGE_MODULE: &str = "import { bridge } from 'lunarstorm:desktop';\nexport const setProperty = bridge.setProperty;\nexport const serve = bridge.serve;\nexport const restored = bridge.restored;\nexport default bridge;\n";

/// Source of `@desktop/api`.
const API_MODULE: &str = "import { native } from 'lunarstorm:desktop';\nexport { native };\nexport default native;\n";

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
    /// Filesystem roots the backend may read and write; nothing outside them.
    pub roots: Vec<PathBuf>,
    /// The environment the backend sees as `process.env`.
    pub environment: Vec<(String, String)>,
    /// `process.argv[0]`: the application's id.
    pub name: String,
}

/// The glue's hooks of the instance running on this thread.
struct Hooks {
    /// `dispatch(json)`.
    dispatch: <Engine as Vm>::Persistent,
    /// `settle(id, ok, json)`.
    settle: <Engine as Vm>::Persistent,
}

thread_local! {
    /// The hooks of the backend thread's current instance.
    ///
    /// Decision: a thread-local, because the tasks that call the hooks are posted
    /// from other threads and so cannot own engine handles, yet always run on the
    /// one backend thread. It is cleared before the instance is destroyed.
    static HOOKS: RefCell<Option<Hooks>> = const { RefCell::new(None) };
}

/// The host services of one backend instance.
struct DesktopHost<S> {
    /// The desktop process.
    shell: Arc<S>,
    /// The instance's epoch.
    epoch: u64,
    /// `process.argv[0]`.
    name: String,
    /// Filesystem roots the backend may touch.
    roots: Vec<PathBuf>,
    /// `process.env`.
    environment: Vec<(String, String)>,
    /// Whether the entry module is being evaluated; an error then fails the load.
    loading: Cell<bool>,
    /// The first error raised while loading.
    failure: RefCell<Option<String>>,
}

/// Names an interrupted script for what it is.
///
/// # Arguments
/// * `description` - the engine's description of the error.
///
/// # Returns
/// The description; an interrupt reads as an exceeded budget.
fn explain(description: &str) -> String {
    if description.contains("interrupted") {
        format!("execution budget exceeded ({})", description.trim_end())
    } else {
        description.to_owned()
    }
}

impl<S: Shell> Host for DesktopHost<S> {
    /// No extensions.
    type Extensions<V: lunarstorm_event_loop_runtime::vm::Vm> = ();

    /// # Returns
    /// The empty set.
    fn extensions<V: lunarstorm_event_loop_runtime::vm::Vm>(&self) -> Self::Extensions<V> {}

    /// The watchdog reads the pulse to enforce the execution budget.
    const PULSE: bool = true;

    /// # Returns
    /// `process.argv`: the application's id.
    fn args(&self) -> Vec<String> {
        vec![self.name.clone()]
    }

    /// # Returns
    /// The environment the host configuration permits.
    fn environment(&self) -> Vec<(String, String)> {
        self.environment.clone()
    }

    /// # Returns
    /// The process's working directory, or `/` when it has none.
    fn working_directory(&self) -> String {
        std::env::current_dir().map_or_else(|_| String::from("/"), |path| path.to_string_lossy().into_owned())
    }

    /// Sends `console` output to the shell's log.
    ///
    /// # Arguments
    /// * `stream` - stdout is `info`, stderr is `error`.
    /// * `bytes` - what was printed.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        let level = if stream == Stream::Out { "info" } else { "error" };
        self.shell.log(level, String::from_utf8_lossy(bytes).trim_end_matches('\n'));
    }

    /// `process.exit(code)` in the backend ends the application.
    ///
    /// # Arguments
    /// * `code` - the exit status.
    fn exit(&self, code: i32) -> ! {
        std::process::exit(code)
    }

    /// The fs/net policy of a desktop backend: paths under the permitted roots,
    /// and no sockets (the host configuration has no network permission).
    ///
    /// # Arguments
    /// * `access` - what the backend is about to do.
    ///
    /// # Returns
    /// Whether it may.
    fn allow(&self, access: Access<'_>) -> bool {
        match access {
            Access::Read(path) | Access::Write(path) => self.roots.iter().any(|root| Path::new(path).starts_with(root)),
            Access::Listen(_) | Access::Connect(..) => false,
        }
    }

    /// Fills the buffer from the system entropy source.
    ///
    /// # Arguments
    /// * `into` - the buffer to fill completely.
    fn entropy(&self, into: &mut [u8]) {
        for chunk in into.chunks_mut(ENTROPY_CHUNK) {
            // SAFETY: `chunk` is a live, writable slice of at most 256 bytes, the
            // most `getentropy` accepts.
            let filled = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) };
            assert!(filled == 0, "the system entropy source failed");
        }
    }

    /// Forwards a Runtime record to the shell.
    ///
    /// # Arguments
    /// * `record` - the record.
    fn log(&self, record: Log<'_>) {
        match record {
            Log::Warning(text) => self.shell.log("warn", text),
            Log::Error(text) => self.shell.error(self.epoch, text),
            Log::Uncaught { description, .. } => self.shell.error(self.epoch, &explain(description)),
        }
    }

    /// Reports a script error and lets the backend go on. While the entry module
    /// is loading, the first error is kept as the reason the load failed.
    ///
    /// # Arguments
    /// * `_in_promise` - whether it is a rejected promise.
    /// * `description` - the error as `console` would show it.
    fn uncaught(&self, _in_promise: bool, description: &str) {
        if self.loading.get() {
            self.failure.borrow_mut().get_or_insert_with(|| explain(description));
        } else {
            self.shell.error(self.epoch, &explain(description));
        }
    }
}

/// Reads a string argument of a native function.
///
/// # Arguments
/// * `call` - the call.
/// * `index` - the argument's position.
///
/// # Returns
/// The text; empty when the argument is missing.
///
/// # Errors
/// The engine's exception when the argument is not convertible to a string.
fn text(call: &Call<'_, Engine>, index: usize) -> Outcome<String> {
    match call.args().get(index) {
        Some(value) => call.scope().with_str(value, str::to_owned),
        None => Ok(String::new()),
    }
}

/// Shorthand for a [`Sig`] of the backend natives: no rest parameter.
///
/// # Arguments
/// * `params` - the parameters.
/// * `returns` - the result.
/// * `about` - one line.
///
/// # Returns
/// The signature.
const fn sig(params: &'static [Param], returns: Ty, about: &'static str) -> Sig {
    Sig { params, rest: None, returns, about }
}

/// `desktopSend(json)`.
const SEND: Sig = sig(&[Param { name: "json", ty: Ty::String, about: "the envelope as JSON" }], Ty::Undefined, "Sends an envelope to the desktop process.");
/// `desktopNative(id, namespace, method, args)`.
const NATIVE: Sig = sig(
    &[
        Param { name: "id", ty: Ty::Number, about: "the call id the result settles" },
        Param { name: "namespace", ty: Ty::String, about: "the native API's namespace" },
        Param { name: "method", ty: Ty::String, about: "the method" },
        Param { name: "args", ty: Ty::String, about: "the arguments as a JSON array" },
    ],
    Ty::Undefined,
    "Starts a native call; the result arrives through the settle hook.",
);
/// `desktopError(text)`.
const ERROR: Sig = sig(&[Param { name: "text", ty: Ty::String, about: "the formatted error" }], Ty::Undefined, "Reports a handler error.");
/// `desktopRestore()`.
const RESTORE: Sig = sig(&[], Ty::String, "The control values carried over from the previous instance, as JSON.");
/// `desktopHooks(dispatch, settle)`. Only the hooks' callability is checked at
/// the call; their parameters are declared for `lunarstorm types`.
///
/// Fix: the hooks were declared without parameters, which typed them as `() =>
/// void`, a type the glue's real hooks (they take arguments) are not assignable
/// to. They now declare the arguments the host passes.
const HOOKS_SIG: Sig = sig(
    &[
        Param {
            name: "dispatch",
            ty: Ty::Function(&sig(&[Param { name: "json", ty: Ty::String, about: "" }], Ty::Undefined, "")),
            about: "delivers one envelope (JSON) from the desktop process",
        },
        Param {
            name: "settle",
            ty: Ty::Function(&sig(
                &[
                    Param { name: "id", ty: Ty::Number, about: "" },
                    Param { name: "ok", ty: Ty::Bool, about: "" },
                    Param { name: "json", ty: Ty::String, about: "" },
                ],
                Ty::Undefined,
                "",
            )),
            about: "settles a native call: (id, ok, json)",
        },
    ],
    Ty::Undefined,
    "Hands the host the glue's two hooks.",
);

/// The bindings of one backend instance: the natives behind the glue, the glue
/// and the two modules a backend imports.
///
/// | native                               | does                                   |
/// |--------------------------------------|----------------------------------------|
/// | `desktopSend(json)`                  | an envelope to the desktop process     |
/// | `desktopNative(id, ns, method, args)`| starts a native call                   |
/// | `desktopError(text)`                 | reports a handler error                |
/// | `desktopRestore()`                   | the carried-over control values (JSON) |
/// | `desktopHooks(dispatch, settle)`     | hands the host the glue's hooks        |
///
/// # Arguments
/// * `shell` - the desktop process.
/// * `epoch` - the instance's epoch.
/// * `restore` - the carried-over control values as JSON.
///
/// # Returns
/// The bindings to register before `start`.
fn bindings<S: Shell>(shell: &Arc<S>, epoch: u64, restore: String) -> Bindings<Engine> {
    let (send, native, error) = (shell.clone(), shell.clone(), shell.clone());
    Bindings::new()
        .function("desktopSend", SEND, move |call: &Call<'_, Engine>| {
            send.send(epoch, &text(call, 0)?);
            Ok(call.scope().undefined())
        })
        .function("desktopNative", NATIVE, move |call: &Call<'_, Engine>| {
            let scope = call.scope();
            let id = call.args().first().and_then(|id| scope.as_number(id)).unwrap_or(0.0) as u64;
            native.native(epoch, id, &text(call, 1)?, &text(call, 2)?, &text(call, 3)?);
            Ok(scope.undefined())
        })
        .function("desktopError", ERROR, move |call: &Call<'_, Engine>| {
            error.error(epoch, &text(call, 0)?);
            Ok(call.scope().undefined())
        })
        .function("desktopRestore", RESTORE, move |call: &Call<'_, Engine>| call.scope().string(&restore))
        .function("desktopHooks", HOOKS_SIG, |call: &Call<'_, Engine>| {
            let scope = call.scope();
            if let [dispatch, settle, ..] = call.args() {
                let hooks = Hooks { dispatch: scope.persist(dispatch), settle: scope.persist(settle) };
                HOOKS.with(|slot| *slot.borrow_mut() = Some(hooks));
            }
            Ok(scope.undefined())
        })
        .module(GLUE_MODULE, GLUE)
        .module("@desktop/bridge", BRIDGE_MODULE)
        .module("@desktop/api", API_MODULE)
}

/// A desktop process that ignores everything, for [`declarations`]: its Runtime
/// never starts, so none of these is called.
struct Silent;

impl Shell for Silent {
    /// Ignored.
    fn loaded(&self, _: u64, _: Option<&str>) {}
    /// Ignored.
    fn send(&self, _: u64, _: &str) {}
    /// Ignored.
    fn native(&self, _: u64, _: u64, _: &str, _: &str, _: &str) {}
    /// Ignored.
    fn error(&self, _: u64, _: &str) {}
    /// Ignored.
    fn log(&self, _: &str, _: &str) {}
    /// Ignored.
    fn retired(&self, _: u64) {}
}

/// Lists everything a backend on this host can reach, for `lunarstorm types`.
///
/// Decision: built the way [`start`] builds a backend's Runtime (same host type,
/// same [`bindings`]), over a [`Silent`] shell and without starting it, so the
/// declarations cannot drift from what a backend gets.
///
/// # Returns
/// The Runtime's merged declarations (`Runtime::declarations`).
///
/// # Errors
/// The event loop cannot be created, or the Runtime's own error.
pub fn declarations() -> Result<Vec<lunarstorm_event_loop_runtime::vm::ModuleDecl>, String> {
    let shell = Arc::new(Silent);
    let host = DesktopHost {
        shell: shell.clone(),
        epoch: 0,
        name: String::new(),
        roots: Vec::new(),
        environment: Vec::new(),
        loading: Cell::new(false),
        failure: RefCell::new(None),
    };
    let mut runtime = Runtime::<Engine, _>::new(host).map_err(|error| format!("cannot create the event loop: {error}"))?;
    runtime.register(bindings(&shell, 0, String::new()));
    runtime.declarations()
}

/// Describes the pending exception of a failed hook call.
///
/// # Arguments
/// * `scope` - the context handle; its pending exception is taken.
///
/// # Returns
/// The stack or message of an error, or the thrown text.
fn thrown<'s>(scope: &<Engine as Vm>::Scope<'s>) -> String {
    let value = scope.exception();
    let field = |key: &str| scope.get(&value, key).ok().and_then(|field| scope.with_str(&field, str::to_owned).ok());
    let message = field("message").unwrap_or_default();
    let described = match field("stack") {
        Some(stack) if !stack.is_empty() => format!("{message}\n{stack}"),
        _ if !message.is_empty() => message,
        _ => scope.with_str(&value, str::to_owned).unwrap_or_else(|Thrown| String::from("<unprintable>")),
    };
    explain(&described)
}

/// Calls one of the glue's hooks on the backend thread.
///
/// # Arguments
/// * `scope` - the context handle of the posted task.
/// * `shell` - receives the error if the hook throws.
/// * `epoch` - the instance's epoch.
/// * `call` - picks the hook and builds its arguments, then calls it.
fn hook<'s, S: Shell>(
    scope: &<Engine as Vm>::Scope<'s>,
    shell: &S,
    epoch: u64,
    call: impl FnOnce(&Hooks) -> Outcome<()>,
) {
    // The borrow is shared: the glue only replaces the hooks while a module loads.
    let outcome = HOOKS.with(|slot| slot.borrow().as_ref().map(call));
    if let Some(Err(Thrown)) = outcome {
        shell.error(epoch, &thrown(scope));
    }
}

/// The live instance's epoch and the handle that posts to its loop.
type Slot = Arc<Mutex<Option<(u64, Poster<Engine>)>>>;

/// What the control channel carries.
enum Control {
    /// Replace the running backend.
    Load(Program),
    /// End the thread.
    Stop,
}

/// Interrupts a backend whose JS entry outlives the budget.
struct Watchdog {
    /// Tells the thread to end.
    stop: Arc<AtomicBool>,
    /// The thread.
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    /// Starts watching a Runtime.
    ///
    /// The thread samples the pulse twice per budget. The same odd value for a
    /// whole budget means one JS entry has been running that long.
    ///
    /// # Arguments
    /// * `pulse` - the Runtime's pulse.
    /// * `interrupt` - the instance's termination handle.
    /// * `budget` - the longest one entry may run.
    ///
    /// # Returns
    /// The watchdog; dropping it ends the thread.
    fn start(pulse: Arc<AtomicU64>, interrupt: Interrupt, budget: Duration) -> Watchdog {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new()
            .name("lunarstorm-budget".into())
            .spawn(move || {
                let mut seen = pulse.load(Ordering::Relaxed);
                let mut since = Instant::now();
                while !stopped.load(Ordering::Relaxed) {
                    std::thread::park_timeout(budget / 2);
                    let now = pulse.load(Ordering::Relaxed);
                    if now != seen || now.is_multiple_of(2) {
                        seen = now;
                        since = Instant::now();
                    } else if since.elapsed() >= budget {
                        interrupt.request();
                        since = Instant::now();
                    }
                }
            })
            .expect("spawn the budget watchdog");
        Watchdog { stop, thread: Some(thread) }
    }
}

impl Drop for Watchdog {
    /// Ends the thread and waits for it.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

/// One running backend.
struct Instance<S: Shell> {
    /// Its Runtime.
    runtime: Runtime<Engine, DesktopHost<S>>,
    /// Its epoch.
    epoch: u64,
    /// Its budget watchdog; dropped (and joined) with the instance.
    _watchdog: Watchdog,
}

/// Starts a backend: a new Runtime, its bindings and the entry module.
///
/// # Arguments
/// * `program` - what to run.
/// * `shell` - the desktop process.
/// * `slot` - where the new instance's poster is published.
///
/// # Returns
/// The instance.
///
/// # Errors
/// Why the backend did not load. Nothing of it is left running.
fn start<S: Shell>(program: Program, shell: &Arc<S>, slot: &Slot) -> Result<Instance<S>, String> {
    let Program { epoch, code, restore, memory, budget, roots, environment, name } = program;
    let host = DesktopHost {
        shell: shell.clone(),
        epoch,
        name,
        roots,
        environment,
        loading: Cell::new(true),
        failure: RefCell::new(None),
    };
    let mut runtime = Runtime::<Engine, _>::new(host).map_err(|error| format!("cannot create the event loop: {error}"))?;
    runtime.limits(Limits { memory, ..Limits::default() });
    runtime.register(bindings(shell, epoch, restore));
    runtime.start()?;
    // Events arrive as posted tasks, so the loop sleeps instead of going idle.
    runtime.hold(true);
    let interrupt = runtime.interrupt().ok_or("the runtime has no instance")?;
    let watchdog = Watchdog::start(runtime.pulse(), interrupt, budget);

    let loaded = runtime.load(Source::Text { name: String::from("backend.js"), code: code.to_string() });
    runtime.host().loading.set(false);
    let failure = runtime.host().failure.borrow_mut().take();
    if let Some(error) = failure.or(loaded.err()) {
        HOOKS.with(|hooks| hooks.borrow_mut().take());
        runtime.stop();
        return Err(error);
    }
    *slot.lock().expect("slot lock") = Some((epoch, runtime.poster()));
    Ok(Instance { runtime, epoch, _watchdog: watchdog })
}

/// Ends the running backend, if any, and tells the shell its epoch is retired.
///
/// # Arguments
/// * `live` - the running backend.
/// * `shell` - the desktop process.
/// * `slot` - cleared first, so nothing more is posted to the dying loop.
fn retire<S: Shell>(live: &mut Option<Instance<S>>, shell: &Arc<S>, slot: &Slot) {
    slot.lock().expect("slot lock").take();
    if let Some(mut instance) = live.take() {
        // The hooks are engine handles and must go before the instance does.
        HOOKS.with(|hooks| hooks.borrow_mut().take());
        instance.runtime.stop();
        let epoch = instance.epoch;
        drop(instance);
        shell.retired(epoch);
    }
}

/// The backend thread: runs the live Runtime's loop and acts on control messages.
///
/// # Arguments
/// * `control` - reloads and the stop request.
/// * `shell` - the desktop process.
/// * `slot` - where the live instance's poster is published.
fn run<S: Shell>(control: Receiver<Control>, shell: Arc<S>, slot: Slot) {
    let mut live: Option<Instance<S>> = None;
    loop {
        let message = match live.as_mut() {
            // Nothing runs: only a control message can change that.
            None => match control.recv() {
                Ok(message) => message,
                Err(_) => break,
            },
            Some(instance) => match control.try_recv() {
                Ok(message) => message,
                Err(TryRecvError::Empty) => {
                    // Sleeps in the kernel until an event is posted, a timer is
                    // due or I/O is ready; a control message wakes it too.
                    instance.runtime.poll(Wait::Block);
                    continue;
                }
                Err(TryRecvError::Disconnected) => break,
            },
        };
        match message {
            Control::Stop => break,
            Control::Load(program) => {
                retire(&mut live, &shell, &slot);
                let epoch = program.epoch;
                match start(program, &shell, &slot) {
                    Ok(instance) => {
                        live = Some(instance);
                        shell.loaded(epoch, None);
                    }
                    Err(error) => shell.loaded(epoch, Some(&error)),
                }
            }
        }
    }
    retire(&mut live, &shell, &slot);
}

/// Delivers native-call results to the backend from any thread.
pub struct Settler<S> {
    /// The desktop process, for errors.
    shell: Arc<S>,
    /// The live instance's poster.
    slot: Slot,
}

impl<S> Clone for Settler<S> {
    /// # Returns
    /// Another handle to the same backend thread.
    fn clone(&self) -> Self {
        Settler { shell: self.shell.clone(), slot: self.slot.clone() }
    }
}

impl<S: Shell> Settler<S> {
    /// Finishes a native call. A result for a retired epoch is dropped.
    ///
    /// # Arguments
    /// * `epoch` - the epoch [`Shell::native`] was called with.
    /// * `call` - the call id it was called with.
    /// * `result` - the value as JSON (empty for `undefined`), or the error
    ///   message.
    pub fn settle(&self, epoch: u64, call: u64, result: Result<String, String>) {
        let guard = self.slot.lock().expect("slot lock");
        let Some((live, poster)) = guard.as_ref() else { return };
        if *live != epoch {
            return;
        }
        let shell = self.shell.clone();
        poster.post(move |scope| {
            hook(scope, &*shell, epoch, |hooks| {
                let (ok, json) = match &result {
                    Ok(json) => (true, scope.string(json)?),
                    // The glue parses the error as JSON too, so it is quoted.
                    Err(message) => (false, scope.string(&quote(message))?),
                };
                let arguments = [scope.number(call as f64), scope.boolean(ok), json];
                scope.call(&scope.local(&hooks.settle), &arguments).map(drop)
            });
        });
    }
}

/// Writes text as a JSON string literal.
///
/// # Arguments
/// * `text` - the text.
///
/// # Returns
/// The literal, quotes included.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", control as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The backend of a desktop application: a thread that runs one Runtime per
/// epoch. Dropping it stops the backend.
pub struct Backend<S: Shell> {
    /// Reloads and the stop request.
    control: Sender<Control>,
    /// The live instance's epoch and poster.
    slot: Slot,
    /// The desktop process.
    shell: Arc<S>,
    /// The backend thread.
    thread: Option<JoinHandle<()>>,
}

impl<S: Shell> Backend<S> {
    /// Starts the backend thread. It idles until the first [`Backend::load`].
    ///
    /// # Arguments
    /// * `shell` - the desktop process.
    ///
    /// # Returns
    /// The handle.
    pub fn spawn(shell: Arc<S>) -> Backend<S> {
        let (control, receiver) = channel();
        let slot: Slot = Arc::default();
        let (thread_shell, thread_slot) = (shell.clone(), slot.clone());
        let thread = std::thread::Builder::new()
            .name("lunarstorm-backend".into())
            .spawn(move || run(receiver, thread_shell, thread_slot))
            .expect("spawn the backend thread");
        Backend { control, slot, shell, thread: Some(thread) }
    }

    /// Sends a control message and wakes the loop so it is seen.
    ///
    /// # Arguments
    /// * `message` - the message.
    fn control(&self, message: Control) {
        if self.control.send(message).is_ok() {
            if let Some((_, poster)) = self.slot.lock().expect("slot lock").as_ref() {
                poster.post(|_| {});
            }
        }
    }

    /// Starts a backend, replacing the running one. [`Shell::loaded`] reports the
    /// outcome; the previous epoch is retired first.
    ///
    /// # Arguments
    /// * `program` - what to run.
    pub fn load(&self, program: Program) {
        self.control(Control::Load(program));
    }

    /// Delivers an `event`, `request` or `cancel` envelope to the backend. An
    /// envelope for any epoch but the live one is late or early and is dropped
    /// (spec 6.2).
    ///
    /// # Arguments
    /// * `epoch` - the epoch the envelope is for.
    /// * `envelope` - the envelope as JSON.
    pub fn dispatch(&self, epoch: u64, envelope: String) {
        let guard = self.slot.lock().expect("slot lock");
        let Some((live, poster)) = guard.as_ref() else { return };
        if *live != epoch {
            return;
        }
        let shell = self.shell.clone();
        poster.post(move |scope| {
            hook(scope, &*shell, epoch, |hooks| {
                let json = scope.string(&envelope)?;
                scope.call(&scope.local(&hooks.dispatch), &[json]).map(drop)
            });
        });
    }

    /// # Returns
    /// A handle that settles native calls from any thread.
    pub fn settler(&self) -> Settler<S> {
        Settler { shell: self.shell.clone(), slot: self.slot.clone() }
    }
}

impl<S: Shell> Drop for Backend<S> {
    /// Stops the backend and waits for its thread.
    fn drop(&mut self) {
        self.control(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
