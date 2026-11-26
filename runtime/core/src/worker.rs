//! `lunarstorm:worker`: a part of the program on its own thread, in its own VM.
//!
//! ```text
//!  parent Runtime (thread A)                    child Runtime (thread B)
//!  Workers ── to_child Channel ───────────────> Ipc receiver  (worker `onMessage`)
//!          <─ from_child Channel ───────────── Link.outbound  (worker `postMessage`)
//!          <─ Exit (mutex) + Nudge ──────────── thread end
//! ```
//!
//! A worker is a [`Runtime`] built on a new thread from a host the parent's host
//! provides ([`Host::worker_host`]), running the entry module a path names. The two
//! directions are [`Channel`]s, so the bounds are those of `lunarstorm:ipc`: 4096
//! unread messages, 64 MiB unread bytes, 8 MiB per message, per direction and per
//! worker. At most [`MAX_ACTIVE_WORKERS`] workers run at once in one Runtime.
//!
//! Decisions:
//! - The module is native ([`Workers::module`]): the handle class, its `exited`
//!   promise ([`Scope::promise`]) and the message encoding were `js/worker.js`.
//!   The low-level natives ([`Workers::exports`]) sit on an object every export
//!   receives as its bound first argument, so no Rust closure holds a JS handle.
//! - Message format: a message crosses threads as bytes. The first byte is a tag:
//!   0 = the rest is UTF-8 JSON text, 1 = the rest is a `Uint8Array`'s bytes. So
//!   structured data means JSON values and a top-level `Uint8Array`; functions,
//!   symbols and `undefined` are refused with a TypeError, and a nested
//!   `Uint8Array` arrives as a JSON object. Dropped alternative: a full structured
//!   clone, which the VM contract does not offer.
//! - Bounds are per direction per worker, where jetbunny shared one inbound credit
//!   among all children of a parent. The shared credit needs a sender id on every
//!   queued message and buys only a tighter total; the per-worker bound already
//!   caps the parent's memory at 64 MiB times [`MAX_ACTIVE_WORKERS`].
//! - Entry paths resolve against the directory of the program's entry module, under
//!   [`Access::Read`]. jetbunny's `jetbunny.workers` allowlist is a package feature
//!   and this Runtime has no packages.
//! - `exited` holds one loop reference (`Shared::refs`) from `spawn` until the exit
//!   is delivered, so the parent outlives its workers. A worker's own receiver holds
//!   a reference through [`Ipc`], so a worker with a listener lives until `close()`
//!   or `terminate()`.
//! - Messages a worker sent are delivered to the parent's listener before the exit
//!   is reported, and a listener registered after the exit still gets what is queued.
//! - `terminate()` sets a flag the worker's loop checks every step, asks the engine
//!   to stop running JS ([`Interrupt`]) and wakes the loop; the host's uncaught-error
//!   policy is not consulted for the interrupt's own failure.
//! - A worker's host should make `Host::exit` end its thread only; the Runtime
//!   cannot unwind out of a native call, so `process.exit` in a worker ends whatever
//!   `exit` ends.

use crate::channel::{Channel, Nudge};
use crate::host::{Access, Host};
use crate::io::Shared;
use crate::ipc::Ipc;
use crate::runtime::{Runtime, Source};
use lunarstorm_vm::{Call, ErrorKind, Interrupt, Kind, Limits, Outcome, Scope, Vm};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

/// The specifier scripts import.
pub const MODULE: &str = "lunarstorm:worker";
/// Most workers running at once in one Runtime.
pub const MAX_ACTIVE_WORKERS: usize = 64;

/// Locks a mutex, ignoring poisoning: a panicking worker thread must not make its
/// parent panic too.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// How a worker ended.
enum Exit {
    /// Its program finished (or it called `close()`).
    Fulfilled,
    /// The parent called `terminate()`, or the parent itself ended.
    Terminated,
    /// It never ran: the entry could not be loaded or the thread panicked.
    Failed(String),
}

/// What the parent can do to a running worker from its own thread.
pub struct Control {
    /// Set by `terminate`; shared with the worker's `Shared::terminated`, which its
    /// loop checks every step.
    terminated: Arc<AtomicBool>,
    /// Stops the JS the worker is running; set once its VM exists.
    interrupt: Mutex<Option<Interrupt>>,
    /// Wakes the worker's loop; set once its Runtime exists.
    nudge: Mutex<Option<Nudge>>,
}

impl Control {
    /// # Returns
    /// A control with nothing terminated and no worker attached yet.
    fn new() -> Arc<Self> {
        Arc::new(Self { terminated: Arc::default(), interrupt: Mutex::new(None), nudge: Mutex::new(None) })
    }

    /// Wakes the worker's loop.
    fn wake(&self) {
        if let Some(nudge) = lock(&self.nudge).as_ref() {
            nudge.nudge();
        }
    }

    /// Ends the worker: flag, interrupt, wake.
    fn terminate(&self) {
        self.terminated.store(true, Ordering::SeqCst);
        if let Some(interrupt) = lock(&self.interrupt).as_ref() {
            interrupt.request();
        }
        self.wake();
    }
}

/// A worker Runtime's link to its parent.
pub struct Link {
    /// Messages the worker sends to the parent.
    pub outbound: Arc<Channel>,
    /// Wakes the parent's loop.
    pub parent: Nudge,
}

/// One worker as its parent sees it.
struct Child<V: Vm> {
    /// Messages for the worker.
    to_child: Arc<Channel>,
    /// Messages from the worker.
    from_child: Arc<Channel>,
    /// Terminate, interrupt, wake.
    control: Arc<Control>,
    /// Set by the worker thread as its last act.
    exit: Arc<Mutex<Option<Exit>>>,
    /// The worker's thread, joined when the record goes.
    thread: Option<JoinHandle<()>>,
    /// The parent's listener for the worker's messages.
    listener: Option<V::Persistent>,
    /// Called once with the exit.
    on_exit: Option<V::Persistent>,
    /// Whether the exit was delivered (and the loop reference given back).
    reported: bool,
}

/// What the loop has to hand to JS next.
pub enum WorkerEvent<'s, V: Vm> {
    /// A message for a worker's listener.
    Message {
        /// The listener.
        listener: V::Value<'s>,
        /// The message bytes (tag byte first, see `js/worker.js`).
        bytes: Vec<u8>,
    },
    /// A worker ended.
    Exit {
        /// The `exited` callback.
        callback: V::Value<'s>,
        /// `fulfilled`, `terminated` or `failed`.
        state: &'static str,
        /// The failure text for `failed`.
        message: Option<String>,
    },
}

/// The per-Runtime worker state: its children and, when it is itself a worker, its
/// link to the parent.
pub struct Workers<V: Vm> {
    /// Children by id.
    children: RefCell<BTreeMap<u32, Child<V>>>,
    /// Last id handed out.
    next: Cell<u32>,
    /// Set when this Runtime is a worker.
    link: RefCell<Option<Link>>,
    /// Wakes this Runtime's own loop.
    nudge: Nudge,
}

impl<V: Vm> Workers<V> {
    /// # Arguments
    /// * `nudge` - wakes the Runtime's loop.
    ///
    /// # Returns
    /// The state, with no children and no parent.
    pub fn new(nudge: Nudge) -> Rc<Self> {
        Rc::new(Self { children: RefCell::new(BTreeMap::new()), next: Cell::new(0), link: RefCell::new(None), nudge })
    }

    /// Makes this Runtime a worker.
    ///
    /// # Arguments
    /// * `link` - the way back to the parent.
    pub fn attach(&self, link: Link) {
        *self.link.borrow_mut() = Some(link);
    }

    /// Takes the next thing JS has to hear about: a message for a listener, or an
    /// exit. A worker's messages come before its exit; finished children whose
    /// messages are all taken are dropped (and their thread joined).
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `refs` - the loop's reference count; an exit gives its reference back.
    ///
    /// # Returns
    /// The event, or `None` when there is nothing to deliver.
    pub fn next_event<'s>(&self, scope: &V::Scope<'s>, refs: &Cell<u32>) -> Option<WorkerEvent<'s, V>> {
        let mut found = None;
        let mut gone = Vec::new();
        let mut children = self.children.borrow_mut();
        for (&id, child) in children.iter_mut() {
            if let Some(listener) = &child.listener {
                if let Some(bytes) = child.from_child.take() {
                    found = Some(WorkerEvent::Message { listener: scope.local(listener), bytes });
                    break;
                }
            }
            if !child.reported {
                if let (Some(callback), Some(exit)) = (&child.on_exit, lock(&child.exit).take()) {
                    child.reported = true;
                    refs.set(refs.get().saturating_sub(1));
                    let (state, message) = match exit {
                        Exit::Fulfilled => ("fulfilled", None),
                        Exit::Terminated => ("terminated", None),
                        Exit::Failed(message) => ("failed", Some(message)),
                    };
                    found = Some(WorkerEvent::Exit { callback: scope.local(callback), state, message });
                    break;
                }
            }
            if child.reported && !child.from_child.has_messages() {
                gone.push(id);
            }
        }
        let finished: Vec<Child<V>> = gone.into_iter().filter_map(|id| children.remove(&id)).collect();
        drop(children);
        for mut child in finished {
            if let Some(thread) = child.thread.take() {
                let _ = thread.join();
            }
        }
        found
    }

    /// Ends every worker and waits for their threads. Must run before the VM
    /// instance is destroyed: the listeners are persistent handles into its heap.
    ///
    /// # Arguments
    /// * `refs` - the loop's reference count.
    pub fn shutdown(&self, refs: &Cell<u32>) {
        let children = std::mem::take(&mut *self.children.borrow_mut());
        for (_, mut child) in children {
            child.control.terminate();
            if let Some(thread) = child.thread.take() {
                let _ = thread.join();
            }
            if !child.reported {
                refs.set(refs.get().saturating_sub(1));
            }
        }
    }

    /// Starts a worker.
    ///
    /// # Arguments
    /// * `shared` - loop state and host.
    /// * `path` - the entry module as the script wrote it.
    /// * `memory` - the worker's heap ceiling in bytes.
    ///
    /// # Returns
    /// The worker's id.
    ///
    /// # Errors
    /// The message to throw: the limit is reached, the host has no workers or
    /// refuses the path, the file is missing, or the worker's VM cannot start.
    fn spawn<H: Host>(&self, shared: &Rc<Shared<H>>, path: &str, memory: usize) -> Result<u32, String> {
        if self.children.borrow().values().filter(|child| !child.reported).count() >= MAX_ACTIVE_WORKERS {
            return Err(String::from("worker limit reached"));
        }
        let entry = entry_path(&shared.entry_dir.borrow(), path);
        let text = entry.to_string_lossy().into_owned();
        if !shared.host.allow(Access::Read(&text)) {
            return Err(format!("EACCES: permission denied by host policy, spawn '{text}'"));
        }
        if !entry.is_file() {
            return Err(format!("worker entry not found: {text}"));
        }
        let factory = shared.host.worker_host().ok_or_else(|| String::from("workers are not available in this host"))?;
        let (to_child, from_child) = (Arc::new(Channel::default()), Arc::new(Channel::default()));
        let (control, exit) = (Control::new(), Arc::new(Mutex::new(None)));
        let thread = launch::<V, H>(Launch {
            factory,
            entry,
            memory,
            inbound: to_child.clone(),
            outbound: from_child.clone(),
            parent: self.nudge.clone(),
            control: control.clone(),
            exit: exit.clone(),
        })?;
        let id = self.next.get() + 1;
        self.next.set(id);
        self.children.borrow_mut().insert(
            id,
            Child { to_child, from_child, control, exit, thread: Some(thread), listener: None, on_exit: None, reported: false },
        );
        shared.refs.set(shared.refs.get() + 1);
        Ok(id)
    }

    /// Builds the exports of [`MODULE`]: `spawn`, `postMessage`, `onMessage` and
    /// `close`, over the natives of [`Workers::exports`].
    ///
    /// # Arguments
    /// * `scope` - the context handle the module is being loaded in.
    /// * `shared` - loop state and host.
    /// * `ipc` - the Runtime's `lunarstorm:ipc` state.
    ///
    /// # Returns
    /// `(name, function)` per export.
    ///
    /// # Errors
    /// [`lunarstorm_vm::Thrown`] when a value cannot be created.
    pub fn module<'s, H: Host>(
        self: &Rc<Self>,
        scope: &V::Scope<'s>,
        shared: &Rc<Shared<H>>,
        ipc: &Rc<Ipc<V>>,
    ) -> Outcome<Vec<(&'static str, V::Value<'s>)>> {
        let natives = scope.object()?;
        for (name, function) in self.exports(scope, shared, ipc)? {
            scope.set(&natives, name, function)?;
        }
        // `(listener, bytes)`: the listener with the decoded message.
        let deliver = scope.function("deliver", |call: &Call<'_, V>| {
            let scope = call.scope();
            let data = decode::<V>(scope, &arg::<V>(scope, call.args(), 1))?;
            scope.call(&arg::<V>(scope, call.args(), 0), &[data])
        })?;
        scope.set(&natives, "_deliver", deliver)?;
        // `(resolve, reject, state, message)`: settles a handle's `exited`.
        let exit = scope.function("exit", |call: &Call<'_, V>| {
            let (scope, args) = (call.scope(), call.args());
            let state = arg::<V>(scope, args, 2);
            if scope.kind(&state) == Kind::String && scope.with_str(&state, |state| state == "failed")? {
                let message = arg::<V>(scope, args, 3);
                let message = if scope.kind(&message) == Kind::String { scope.with_str(&message, str::to_owned)? } else { String::new() };
                let error = scope.error(ErrorKind::Error, &message)?;
                scope.set(&error, "code", scope.string("WorkerError")?)?;
                scope.call(&arg::<V>(scope, args, 1), &[error])?;
            } else {
                let result = scope.object()?;
                scope.set(&result, "state", state)?;
                scope.call(&arg::<V>(scope, args, 0), &[result])?;
            }
            Ok(scope.undefined())
        })?;
        scope.set(&natives, "_exit", exit)?;

        let post_message = scope.function("postMessage", |call: &Call<'_, V>| {
            let (scope, this) = (call.scope(), call.this());
            let natives = scope.get(this, "_native")?;
            let bytes = encode::<V>(scope, &arg::<V>(scope, call.args(), 0))?;
            scope.call(&scope.get(&natives, "post")?, &[scope.get(this, "id")?, bytes])
        })?;
        let on_message = scope.function("onMessage", |call: &Call<'_, V>| {
            let (scope, this) = (call.scope(), call.this());
            let natives = scope.get(this, "_native")?;
            let wrapped = wrap::<V>(scope, &natives, &arg::<V>(scope, call.args(), 0))?;
            scope.call(&scope.get(&natives, "listen")?, &[scope.get(this, "id")?, wrapped])?;
            Ok(scope.undefined())
        })?;
        let terminate = scope.function("terminate", |call: &Call<'_, V>| {
            let (scope, this) = (call.scope(), call.this());
            let natives = scope.get(this, "_native")?;
            scope.call(&scope.get(&natives, "terminate")?, &[scope.get(this, "id")?])?;
            Ok(scope.undefined())
        })?;
        let spec = lunarstorm_vm::ClassSpec::new("Worker").method("postMessage", post_message).method("onMessage", on_message).method("terminate", terminate);
        // `new Worker(id, natives)`: `id`, the hidden natives and `exited`.
        let worker = scope.class::<(), _>(spec, |call: &Call<'_, V>| {
            let (scope, this, args) = (call.scope(), call.this(), call.args());
            let (id, natives) = (arg::<V>(scope, args, 0), arg::<V>(scope, args, 1));
            scope.set(this, "id", id.clone())?;
            hide::<V>(scope, this, "_native", natives.clone())?;
            let (promise, resolver) = scope.promise()?;
            let pair = scope.local(&resolver);
            drop(resolver);
            let handler = bind::<V>(scope, &scope.get(&natives, "_exit")?, &[scope.get(&pair, "0")?, scope.get(&pair, "1")?])?;
            scope.call(&scope.get(&natives, "onExit")?, &[id, handler])?;
            scope.set(this, "exited", promise)?;
            Ok(())
        })?;
        scope.set(&natives, "_Worker", worker)?;

        let spawn = scope.function("spawn", |call: &Call<'_, V>| {
            let (scope, args) = (call.scope(), call.args());
            let (natives, path, options) = (arg::<V>(scope, args, 0), arg::<V>(scope, args, 1), arg::<V>(scope, args, 2));
            if scope.kind(&path) != Kind::String {
                return Err(scope.throw_error(ErrorKind::Type, "spawn expects a path"));
            }
            let memory = if matches!(scope.kind(&options), Kind::Object | Kind::Function) { scope.get(&options, "maxMemory")? } else { scope.undefined() };
            let memory = if scope.kind(&memory) == Kind::Undefined { 268_435_456.0 } else { scope.as_number(&memory).unwrap_or(f64::NAN) };
            if !(memory.is_finite() && memory.fract() == 0.0 && memory > 0.0) {
                return Err(scope.throw_error(ErrorKind::Range, "maxMemory must be a positive byte count"));
            }
            let id = scope.call(&scope.get(&natives, "spawn")?, &[path, scope.number(memory)])?;
            construct::<V>(scope, &scope.get(&natives, "_Worker")?, &[id, natives])
        })?;
        let post = scope.function("postMessage", |call: &Call<'_, V>| {
            let (scope, args) = (call.scope(), call.args());
            let natives = arg::<V>(scope, args, 0);
            let bytes = encode::<V>(scope, &arg::<V>(scope, args, 1))?;
            if !is_worker::<V>(scope, &natives)? {
                return Ok(scope.boolean(false));
            }
            scope.call(&scope.get(&natives, "post")?, &[scope.int(0), bytes])
        })?;
        let listen = scope.function("onMessage", |call: &Call<'_, V>| {
            let (scope, args) = (call.scope(), call.args());
            let natives = arg::<V>(scope, args, 0);
            let wrapped = wrap::<V>(scope, &natives, &arg::<V>(scope, args, 1))?;
            if !is_worker::<V>(scope, &natives)? {
                return Ok(scope.boolean(false));
            }
            scope.call(&scope.get(&natives, "listenParent")?, &[wrapped])?;
            Ok(scope.boolean(true))
        })?;
        let close = scope.function("close", |call: &Call<'_, V>| {
            let scope = call.scope();
            let natives = arg::<V>(scope, call.args(), 0);
            if is_worker::<V>(scope, &natives)? {
                scope.call(&scope.get(&natives, "close")?, &[])?;
            }
            Ok(scope.undefined())
        })?;
        Ok(vec![
            ("spawn", bind::<V>(scope, &spawn, std::slice::from_ref(&natives))?),
            ("postMessage", bind::<V>(scope, &post, std::slice::from_ref(&natives))?),
            ("onMessage", bind::<V>(scope, &listen, std::slice::from_ref(&natives))?),
            ("close", bind::<V>(scope, &close, std::slice::from_ref(&natives))?),
        ])
    }

    /// Builds the low-level natives the module's exports call.
    ///
    /// # Arguments
    /// * `scope` - the context handle the module is being loaded in.
    /// * `shared` - loop state and host.
    /// * `ipc` - the Runtime's `lunarstorm:ipc` state; a worker's `onMessage` is its
    ///   receiver.
    ///
    /// # Returns
    /// `(name, function)` for `isWorker`, `spawn`, `post`, `listen`, `listenParent`,
    /// `onExit`, `terminate` and `close`.
    ///
    /// # Errors
    /// [`lunarstorm_vm::Thrown`] when a function cannot be created.
    pub fn exports<'s, H: Host>(
        self: &Rc<Self>,
        scope: &V::Scope<'s>,
        shared: &Rc<Shared<H>>,
        ipc: &Rc<Ipc<V>>,
    ) -> Outcome<Vec<(&'static str, V::Value<'s>)>> {
        let this = self.clone();
        let is_worker = scope.function("isWorker", move |call| Ok(call.scope().boolean(this.link.borrow().is_some())))?;

        let (this, host) = (self.clone(), shared.clone());
        let spawn = scope.function("spawn", move |call| {
            let scope = call.scope();
            let (Some(path), Some(memory)) = (call.args().first(), call.args().get(1)) else {
                return Err(scope.throw_error(ErrorKind::Type, "spawn expects a path and a memory limit"));
            };
            let path = scope.with_str(path, str::to_owned)?;
            let memory = scope.as_number(memory).filter(|m| *m >= 1.0).unwrap_or(0.0) as usize;
            match this.spawn(&host, &path, memory) {
                Ok(id) => Ok(scope.number(f64::from(id))),
                Err(message) => Err(scope.throw_error(ErrorKind::Error, &message)),
            }
        })?;

        let this = self.clone();
        let post = scope.function("post", move |call| {
            let scope = call.scope();
            let id = call.args().first().and_then(|id| scope.as_number(id)).unwrap_or(-1.0);
            let Some(message) = call.args().get(1).filter(|m| scope.kind(m) == Kind::Bytes) else {
                return Err(scope.throw_error(ErrorKind::Type, "post expects a Uint8Array"));
            };
            let bytes = scope.with_bytes(message, <[u8]>::to_vec)?;
            let accepted = if id == 0.0 {
                match this.link.borrow().as_ref() {
                    Some(link) => {
                        let accepted = link.outbound.post(bytes);
                        if accepted {
                            link.parent.nudge();
                        }
                        accepted
                    }
                    None => false,
                }
            } else {
                match this.children.borrow().get(&(id as u32)) {
                    Some(child) => {
                        let accepted = child.to_child.post(bytes);
                        if accepted {
                            child.control.wake();
                        }
                        accepted
                    }
                    None => false,
                }
            };
            Ok(scope.boolean(accepted))
        })?;

        let this = self.clone();
        let listen = scope.function("listen", move |call| {
            let scope = call.scope();
            let id = call.args().first().and_then(|id| scope.as_number(id)).unwrap_or(-1.0) as u32;
            let listener = match call.args().get(1) {
                Some(function) if scope.kind(function) == Kind::Function => Some(scope.persist(function)),
                _ => None,
            };
            if let Some(child) = this.children.borrow_mut().get_mut(&id) {
                child.listener = listener;
            }
            // Queued messages are delivered in a later turn, never from inside `listen`.
            this.nudge.nudge();
            Ok(scope.undefined())
        })?;

        let this = self.clone();
        let on_exit = scope.function("onExit", move |call| {
            let scope = call.scope();
            let id = call.args().first().and_then(|id| scope.as_number(id)).unwrap_or(-1.0) as u32;
            if let Some(function) = call.args().get(1).filter(|f| scope.kind(f) == Kind::Function) {
                if let Some(child) = this.children.borrow_mut().get_mut(&id) {
                    child.on_exit = Some(scope.persist(function));
                }
            }
            this.nudge.nudge();
            Ok(scope.undefined())
        })?;

        let this = self.clone();
        let terminate = scope.function("terminate", move |call| {
            let scope = call.scope();
            let id = call.args().first().and_then(|id| scope.as_number(id)).unwrap_or(-1.0) as u32;
            if let Some(child) = this.children.borrow().get(&id) {
                child.control.terminate();
            }
            Ok(scope.undefined())
        })?;

        let host = shared.clone();
        let close = scope.function("close", move |call| {
            host.stop.set(true);
            Ok(call.scope().undefined())
        })?;

        let listen_parent = ipc.on_message(scope, shared)?;
        Ok(vec![
            ("isWorker", is_worker),
            ("spawn", spawn),
            ("post", post),
            ("listen", listen),
            ("listenParent", listen_parent),
            ("onExit", on_exit),
            ("terminate", terminate),
            ("close", close),
        ])
    }
}

/// Reads an optional argument.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `args` - the call's arguments.
/// * `index` - position.
///
/// # Returns
/// The argument; `undefined` when it was left out.
fn arg<'s, V: Vm>(scope: &V::Scope<'s>, args: &[V::Value<'s>], index: usize) -> V::Value<'s> {
    args.get(index).cloned().unwrap_or_else(|| scope.undefined())
}

/// A builtin reached from the global object by path (`JSON.stringify`).
///
/// # Arguments
/// * `scope` - the context handle.
/// * `path` - the property names from the global object.
///
/// # Returns
/// The value.
///
/// # Errors
/// What a getter throws.
fn builtin<'s, V: Vm>(scope: &V::Scope<'s>, path: &[&str]) -> Outcome<V::Value<'s>> {
    let mut value = scope.global();
    for key in path {
        value = scope.get(&value, key)?;
    }
    Ok(value)
}

/// `function.bind(undefined, ...args)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `function` - the target.
/// * `args` - the bound leading arguments.
///
/// # Returns
/// The bound function.
///
/// # Errors
/// What `bind` throws.
fn bind<'s, V: Vm>(scope: &V::Scope<'s>, function: &V::Value<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
    let mut all = vec![scope.undefined()];
    all.extend_from_slice(args);
    scope.call_with(&builtin::<V>(scope, &["Function", "prototype", "bind"])?, function, &all)
}

/// `new constructor(...args)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `constructor` - the class.
/// * `args` - its arguments.
///
/// # Returns
/// The instance.
///
/// # Errors
/// What the constructor throws.
fn construct<'s, V: Vm>(scope: &V::Scope<'s>, constructor: &V::Value<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
    let list = scope.array(args)?;
    scope.call(&builtin::<V>(scope, &["Reflect", "construct"])?, &[constructor.clone(), list])
}

/// Defines a property `Object.keys` does not list.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `object` - where.
/// * `key` - the name.
/// * `value` - the value.
///
/// # Errors
/// What `Object.defineProperty` throws.
fn hide<'s, V: Vm>(scope: &V::Scope<'s>, object: &V::Value<'s>, key: &str, value: V::Value<'s>) -> Outcome<()> {
    let descriptor = scope.object()?;
    scope.set(&descriptor, "value", value)?;
    scope.call(&builtin::<V>(scope, &["Object", "defineProperty"])?, &[object.clone(), scope.string(key)?, descriptor])?;
    Ok(())
}

/// # Arguments
/// * `scope` - the context handle.
/// * `natives` - the module's natives.
///
/// # Returns
/// Whether this Runtime is a worker.
///
/// # Errors
/// What the native throws.
fn is_worker<'s, V: Vm>(scope: &V::Scope<'s>, natives: &V::Value<'s>) -> Outcome<bool> {
    let answer = scope.call(&scope.get(natives, "isWorker")?, &[])?;
    Ok(scope.as_bool(&answer).unwrap_or(false))
}

/// Turns a message into the bytes that cross threads (format in the module docs).
///
/// # Arguments
/// * `scope` - the context handle.
/// * `data` - JSON-compatible data or a `Uint8Array`.
///
/// # Returns
/// The tag byte followed by the payload, as a `Uint8Array`.
///
/// # Errors
/// A `TypeError` for data JSON cannot represent; what `JSON.stringify` throws.
fn encode<'s, V: Vm>(scope: &V::Scope<'s>, data: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    if scope.kind(data) == Kind::Bytes {
        let bytes = scope.with_bytes(data, |bytes| [&[1u8][..], bytes].concat())?;
        return scope.bytes(bytes);
    }
    let json = scope.call(&builtin::<V>(scope, &["JSON", "stringify"])?, std::slice::from_ref(data))?;
    if scope.kind(&json) != Kind::String {
        return Err(scope.throw_error(ErrorKind::Type, "postMessage expects structured data"));
    }
    let bytes = scope.with_str(&json, |text| [&[0u8][..], text.as_bytes()].concat())?;
    scope.bytes(bytes)
}

/// Turns the bytes of a message back into data.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `bytes` - the tag byte followed by the payload.
///
/// # Returns
/// The data: a `Uint8Array` for tag 1, else the parsed JSON.
///
/// # Errors
/// What `JSON.parse` throws.
fn decode<'s, V: Vm>(scope: &V::Scope<'s>, bytes: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    let (tag, rest) = scope.with_bytes(bytes, |bytes| (bytes.first().copied().unwrap_or(0), bytes.get(1..).unwrap_or(&[]).to_vec()))?;
    if tag == 1 {
        return scope.bytes(rest);
    }
    let text = scope.string(&String::from_utf8_lossy(&rest))?;
    scope.call(&builtin::<V>(scope, &["JSON", "parse"])?, &[text])
}

/// Wraps a listener so it receives decoded data.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `natives` - the module's natives (`_deliver`).
/// * `listener` - the script's listener, or null.
///
/// # Returns
/// What to hand to the native side: null, or the bound deliverer.
///
/// # Errors
/// A `TypeError` when the listener is neither a function nor null.
fn wrap<'s, V: Vm>(scope: &V::Scope<'s>, natives: &V::Value<'s>, listener: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    match scope.kind(listener) {
        Kind::Null | Kind::Undefined => Ok(scope.null()),
        Kind::Function => bind::<V>(scope, &scope.get(natives, "_deliver")?, std::slice::from_ref(listener)),
        _ => Err(scope.throw_error(ErrorKind::Type, "onMessage expects a function or null")),
    }
}

/// Resolves a worker's entry path.
///
/// # Arguments
/// * `entry_dir` - the directory of the program's entry module.
/// * `path` - the path the script wrote.
///
/// # Returns
/// `path` itself when absolute, otherwise `path` inside `entry_dir`.
fn entry_path(entry_dir: &str, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        Path::new(entry_dir).join(path)
    }
}

/// Everything a worker's thread needs.
struct Launch<H: Host> {
    /// Builds the worker's host on its own thread.
    factory: Box<dyn FnOnce() -> H + Send>,
    /// The entry module.
    entry: PathBuf,
    /// Heap ceiling in bytes.
    memory: usize,
    /// Messages from the parent.
    inbound: Arc<Channel>,
    /// Messages to the parent.
    outbound: Arc<Channel>,
    /// Wakes the parent's loop.
    parent: Nudge,
    /// How the parent reaches the worker.
    control: Arc<Control>,
    /// Where the worker records how it ended.
    exit: Arc<Mutex<Option<Exit>>>,
}

/// Starts a worker's thread and waits until its VM exists (not until its program
/// runs), so the parent can post to it at once.
///
/// # Arguments
/// * `launch` - what the thread needs.
///
/// # Returns
/// The thread.
///
/// # Errors
/// The message when the thread cannot start or the worker's VM cannot be created.
fn launch<V: Vm, H: Host>(launch: Launch<H>) -> Result<JoinHandle<()>, String> {
    let (ready, started) = mpsc::channel::<Result<(), String>>();
    let thread = thread::Builder::new()
        .name(String::from("lunarstorm-worker"))
        .spawn(move || {
            let Launch { factory, entry, memory, inbound, outbound, parent, control, exit } = launch;
            let work = AssertUnwindSafe(|| -> Exit {
                let built = (|| -> Result<Runtime<V, H>, String> {
                    let mut runtime = Runtime::<V, H>::new(factory()).map_err(|e| format!("cannot create the event loop: {e}"))?;
                    runtime.limits(Limits { memory: Some(memory), stack: None, gc_threshold: None });
                    runtime.as_worker(Link { outbound, parent: parent.clone() }, inbound, control.terminated.clone());
                    runtime.start()?;
                    *lock(&control.interrupt) = runtime.interrupt();
                    *lock(&control.nudge) = Some(runtime.nudge());
                    Ok(runtime)
                })();
                let mut runtime = match built {
                    Ok(runtime) => {
                        let _ = ready.send(Ok(()));
                        runtime
                    }
                    Err(message) => {
                        let _ = ready.send(Err(message.clone()));
                        return Exit::Failed(message);
                    }
                };
                let failure = match runtime.load(Source::File(entry)) {
                    Ok(()) => {
                        runtime.run();
                        None
                    }
                    Err(message) => {
                        runtime.stop();
                        Some(message)
                    }
                };
                // The Runtime goes first: its drop closes the parent's channel to it.
                drop(runtime);
                match failure {
                    _ if control.terminated.load(Ordering::SeqCst) => Exit::Terminated,
                    Some(message) => Exit::Failed(message),
                    None => Exit::Fulfilled,
                }
            });
            let result = catch_unwind(work).unwrap_or_else(|panic| {
                let text = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()));
                Exit::Failed(format!("the worker thread panicked: {}", text.unwrap_or_default()))
            });
            *lock(&exit) = Some(result);
            parent.nudge();
        })
        .map_err(|e| format!("cannot start a worker thread: {e}"))?;
    match started.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(message)) => {
            let _ = thread.join();
            Err(message)
        }
        Err(_) => {
            let _ = thread.join();
            Err(String::from("the worker thread ended before it started"))
        }
    }
}
