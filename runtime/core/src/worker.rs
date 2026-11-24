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
//! - The JS half is `js/worker.js` (the handle, its `exited` promise, message
//!   encoding); this file is its natives, in the module `lunarstorm:worker-native`,
//!   which only `lunarstorm:worker` may import.
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
use lunarstorm_vm::{ErrorKind, Interrupt, Kind, Limits, Outcome, Scope, Vm};
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
/// The natives behind [`MODULE`]; importable from it only.
pub const NATIVE_MODULE: &str = "lunarstorm:worker-native";
/// The JS half of the module.
pub const SOURCE: &str = include_str!("js/worker.js");
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

    /// Builds the natives of `lunarstorm:worker-native`.
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
                    runtime.limits(Limits { memory: Some(memory), stack: None });
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
