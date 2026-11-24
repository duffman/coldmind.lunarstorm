//! The Runtime interface and the event loop behind it.
//!
//! A host drives a [`Runtime`] through seven calls: [`Runtime::register`],
//! [`Runtime::start`], [`Runtime::load`], [`Runtime::call`], [`Runtime::post`],
//! [`Runtime::poll`] and [`Runtime::stop`]. [`Runtime::run`] is `poll` in a loop for
//! hosts that have no loop of their own.
//!
//! The loop is a single-threaded, edge-triggered mio reactor that owns every socket
//! and the VM instance. It never blocks on anything but `poll` and never runs
//! blocking work; that goes to the worker pool ([`crate::pool`]). JS runs here, one
//! callback at a time, each followed by a full drain of `process.nextTick`
//! callbacks and promise jobs.
//!
//! One step of the loop, in Node's phase order:
//! 1. expired timers;
//! 2. `poll` for I/O readiness, sleeping until the next timer (or not at all when
//!    immediates or posted tasks are queued, or forever when only I/O can wake it);
//! 3. I/O events: accepts, reads, write flushes, and worker results via the waker;
//! 4. tasks posted from other threads;
//! 5. `setImmediate` callbacks.
//!
//! The loop is idle when no handle is open, no timer is armed, no pool job is
//! outstanding and no task is queued.
//!
//! Performance decisions:
//! - **Static dispatch across both boundaries.** `Runtime<V, H>` is monomorphized
//!   over one engine and one host; there is no trait object on any call path, so a
//!   call into the engine or the host is a direct call the optimizer can inline.
//! - **One scope entry for the whole run.** [`Runtime::run`] enters the VM once and
//!   loops inside it. A host with its own loop pays one scope entry per `poll`.
//! - **In-place parsing.** Sockets are read into one 64 KiB scratch buffer owned by
//!   the loop. An HTTP request that arrives whole is parsed and handed to JS straight
//!   from that buffer; nothing is copied into per-connection storage.
//! - **Short-read shortcut.** A read that returns fewer bytes than the buffer holds
//!   has drained the socket, so the extra `read` that would return `WouldBlock` is
//!   skipped. The shortcut is disabled once the kernel reports the peer's close, so
//!   end of stream is never missed.
//! - **One JS entry point.** Every event enters JS through a single `dispatch`
//!   function with integer event codes and a stack array of arguments; no per-event
//!   closures, heap allocations or handle lookups in native code.
//! - **Pipelining.** Requests that arrive together are dispatched back to back and
//!   their responses are corked into one write.
//! - **Coalesced wakes.** A burst of posted tasks costs one wake syscall, not one
//!   per task.
//! - **No readiness syscall while work is queued.** A step that already has an
//!   immediate or a posted task to run asks the kernel for readiness at most once
//!   per [`POLL_GAP`], not once per step.

use crate::bindings::Bindings;
use crate::host::{Access, Host, Log};
use lunarstorm_http1::{self as http, Body, Step};
use crate::io::{flag, Deferred, Entry, Fault, Io, Shared, Wake, WAKER};
use crate::modules::{self as loader, Loader, Rejections};
use crate::ops;
use crate::pool::{Pool, ResultMsg};
use bytes::Buf;
use crossbeam_channel::{unbounded, Receiver, Sender};
use lunarstorm_vm::{
    Context, Instance, Interrupt, Kind, Limits, ModuleMeta, ModuleSource, Outcome, Scope, Thrown, Vm,
};
use mio::event::Event;
use mio::{Events, Poll, Waker};
use std::cell::{Cell, OnceCell, RefCell};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The JS glue, evaluated once at `start`. It is a single function expression that
/// receives the `ops` object and returns the hooks the loop calls.
const BOOTSTRAP: &str = include_str!("js/bootstrap.js");
/// Size of the loop's read buffer.
const SCRATCH: usize = 64 * 1024;
/// Readiness events taken from the kernel per `poll` call.
const EVENT_BATCH: usize = 1024;
/// Longest the loop goes without asking the kernel for readiness while immediates
/// or posted tasks keep it busy.
///
/// Decision: a zero-timeout kernel poll costs about 13 microseconds on macOS, far
/// more than running an immediate callback. At 250 microseconds between polls the
/// syscall is about 5% of a busy loop's time, and I/O that becomes ready during a
/// storm of immediates waits at most this long plus one callback.
const POLL_GAP: Duration = Duration::from_micros(250);

/// Event codes passed to the JS `dispatch` function. Mirrored by the `switch` in
/// `js/bootstrap.js`.
mod ev {
    /// `(key, one_shot)`: a timer fired.
    pub const TIMER: i32 = 0;
    /// `()`: run one `setImmediate` callback; JS returns whether more remain.
    pub const IMMEDIATE: i32 = 1;
    /// `(listener, conn, address, port)`: a TCP connection was accepted.
    pub const CONNECTION: i32 = 2;
    /// `(conn, chunk)`: bytes arrived on a TCP socket.
    pub const DATA: i32 = 3;
    /// `(conn)`: the peer finished sending.
    pub const END: i32 = 4;
    /// `(conn, message?, code?)`: a TCP socket closed, with the error if any.
    pub const CLOSE: i32 = 5;
    /// `(conn)`: a TCP socket's write buffer drained.
    pub const DRAIN: i32 = 6;
    /// `(conn)`: an outbound connect completed.
    pub const CONNECT: i32 = 7;
    /// `(listener, conn, method, url, fields, body)`: an HTTP request is complete.
    pub const HTTP: i32 = 8;
    /// `(request, value, message?, code?)`: a filesystem job finished.
    pub const FS: i32 = 9;
}

/// What [`Runtime::load`] loads as the entry module.
// Decision: `Package` stays unboxed. A `Source` is built once per load and moved
// into it, so its size costs nothing; boxing would only add a `Box::new` at every
// caller.
#[allow(clippy::large_enum_variant)]
pub enum Source {
    /// An ES module file.
    File(PathBuf),
    /// Source text.
    Text {
        /// Module name; relative imports resolve against the host's working
        /// directory joined with it (`[eval]` for a command line's `-e`).
        name: String,
        /// The module's source.
        code: String,
    },
    /// A verified `.jbn` package, from [`lunarstorm_jbn::Package::open`]: its entry
    /// module runs, and its modules resolve and load only from the package.
    Package(lunarstorm_jbn::Package),
}

/// The engine identity a package records (`adapter engine version contract`);
/// a package runs only on the same identity, since bytecode is engine-specific.
///
/// # Returns
/// The identity of `V`.
pub fn engine<V: Vm>() -> String {
    let d = V::descriptor();
    format!("{} {} {} contract {}", d.adapter, d.engine, d.engine_version, d.contract)
}

/// A value that crosses the host boundary: an argument to [`Runtime::call`] or the
/// value its promise settled with.
///
/// Decision: the host interface carries no engine handles. Objects an export
/// returns arrive as JSON [`Arg::Text`]; a host that needs live objects registers
/// bindings, which work on the VM's handles directly.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// `null`; also what `undefined` settles as.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A string.
    Text(String),
    /// A `Uint8Array`.
    Bytes(Vec<u8>),
}

/// Identifies one [`Runtime::call`] until its result is collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallId(u32);

/// How [`Runtime::poll`] waits for readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Sleep until an event, a timer or a posted task. For hosts whose loop is the
    /// Runtime's.
    Block,
    /// Take what is ready and return. For hosts that interleave their own loop.
    NoBlock,
}

/// What a loop step found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The step ran; something can still produce an event.
    Running,
    /// Nothing is open, armed or queued; only `call` or `post` can create work.
    Idle,
}

/// A unit of work posted to the loop thread. It receives the context handle.
type Task<V> = Box<dyn for<'s> FnOnce(&<V as Vm>::Scope<'s>) + Send>;

/// Posts tasks to a Runtime's loop from any thread.
///
/// Decision: the Runtime itself is single-threaded and cannot be shared; this
/// handle is the one part of it that is `Send`, and it holds nothing but a channel
/// and the loop's waker.
pub struct Poster<V: Vm> {
    /// Queue drained by the loop.
    tasks: Sender<Task<V>>,
    /// Wakes the loop out of `poll`.
    waker: Arc<Waker>,
    /// Whether a wake is already on its way; coalesces a burst into one syscall.
    pending: Arc<AtomicBool>,
}

impl<V: Vm> Clone for Poster<V> {
    /// # Returns
    /// Another handle to the same loop.
    fn clone(&self) -> Self {
        Self { tasks: self.tasks.clone(), waker: self.waker.clone(), pending: self.pending.clone() }
    }
}

impl<V: Vm> Poster<V> {
    /// Queues a task for the loop thread and wakes the loop.
    ///
    /// # Arguments
    /// * `task` - runs on the loop thread with the context handle; promise jobs it
    ///   queues are drained right after it.
    ///
    /// # Returns
    /// `false` when the Runtime no longer exists and the task was dropped.
    pub fn post(&self, task: impl for<'s> FnOnce(&V::Scope<'s>) + Send + 'static) -> bool {
        if self.tasks.send(Box::new(task)).is_err() {
            return false;
        }
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.waker.wake();
        }
        true
    }
}

/// The glue's hooks, kept alive across scope entries.
struct Hooks<V: Vm> {
    /// JS entry point for every event.
    dispatch: V::Persistent,
    /// Runs the `process.nextTick` queue.
    ticks: V::Persistent,
    /// Formats a thrown value for the uncaught-error record.
    describe: V::Persistent,
    /// Reads `process.exitCode`.
    exit_code: V::Persistent,
    /// Calls an export and reports how its promise settles.
    invoke: V::Persistent,
    /// Namespace of the entry module, once loaded; where `call` finds exports.
    namespace: Option<V::Persistent>,
}

/// One VM instance and everything that lives inside it.
///
/// Decision: field order is drop order. Persistent handles go first, then the
/// context, then the instance that owns the heap they point into.
struct Engine<V: Vm> {
    /// The glue's hooks.
    hooks: Hooks<V>,
    /// The one realm scripts run in.
    context: V::Context,
    /// The heap.
    instance: V::Instance,
}

/// A JS runtime, generic over the engine and the host that owns it.
pub struct Runtime<V: Vm, H: Host> {
    /// The VM, between `start` and `stop`.
    engine: Option<Engine<V>>,
    /// Loop state and host, shared with the native functions.
    shared: Rc<Shared<H>>,
    /// Unhandled promise rejections, filled by the VM callbacks.
    rejections: Rejections<V>,
    /// Files and packages, shared with the loader; the entry module goes through it too.
    sources: Rc<lunarstorm_modules::Modules>,
    /// The `.jbn` package being run, shared with the loader; set once by `load`.
    package: Rc<OnceCell<lunarstorm_jbn::Package>>,
    /// Specifiers of the host modules, kept by `start` for [`Runtime::package`].
    hosted: Vec<&'static str>,
    /// The readiness source.
    reactor: Poll,
    /// Readiness events of the current step.
    events: Events,
    /// Read buffer shared by every socket.
    scratch: Box<[u8]>,
    /// Host bindings waiting for `start`.
    bindings: Bindings<V>,
    /// Tasks posted from other threads.
    tasks: Receiver<Task<V>>,
    /// The sending side, cloned for each [`Runtime::poster`].
    poster: Poster<V>,
    /// Limits for the instance `start` creates.
    limits: Limits,
    /// Id of the next `call`.
    next_call: u32,
    /// Kernel readiness polls made so far.
    polls: u64,
    /// When the loop last asked the kernel for readiness while work was queued;
    /// `None` after a step that polled because nothing was queued.
    polled: Option<Instant>,
}

impl<V: Vm, H: Host> Drop for Runtime<V, H> {
    /// Releases the rejection reasons before the instance whose heap they live in.
    fn drop(&mut self) {
        self.rejections.borrow_mut().clear();
        self.engine = None;
    }
}

impl<V: Vm, H: Host> Runtime<V, H> {
    /// Creates a Runtime owned by a host. No VM exists until [`Runtime::start`].
    ///
    /// # Arguments
    /// * `host` - the host services.
    ///
    /// # Returns
    /// The Runtime.
    ///
    /// # Errors
    /// The OS error when the readiness source or its waker cannot be created.
    pub fn new(host: H) -> io::Result<Self> {
        let reactor = Poll::new()?;
        let waker = Arc::new(Waker::new(reactor.registry(), WAKER)?);
        let registry = reactor.registry().try_clone()?;
        let (sender, tasks) = unbounded();
        let shared = Rc::new(Shared {
            io: RefCell::new(Io::new(registry, Pool::new(waker.clone()))),
            tick: Cell::new(false),
            immediate: Cell::new(false),
            start: host.monotonic(),
            host,
            calls: RefCell::new(Vec::new()),
            held: Cell::new(false),
            pulse: Arc::default(),
        });
        let sources = loader::sources(&shared);
        Ok(Self {
            engine: None,
            shared,
            sources,
            package: Rc::default(),
            hosted: Vec::new(),
            rejections: Rc::default(),
            reactor,
            events: Events::with_capacity(EVENT_BATCH),
            scratch: vec![0u8; SCRATCH].into_boxed_slice(),
            bindings: Bindings::new(),
            tasks,
            poster: Poster { tasks: sender, waker, pending: Arc::new(AtomicBool::new(false)) },
            limits: Limits::default(),
            next_call: 0,
            polls: 0,
            polled: None,
        })
    }

    /// # Returns
    /// The host this Runtime calls up into.
    pub fn host(&self) -> &H {
        &self.shared.host
    }

    /// # Returns
    /// How many times the loop has asked the kernel for readiness so far. For
    /// hosts and tests that watch what the loop costs.
    pub fn polls(&self) -> u64 {
        self.polls
    }

    /// Sets the limits of the VM instance. Takes effect at [`Runtime::start`].
    ///
    /// # Arguments
    /// * `limits` - heap and stack bounds; ignored by engines whose capabilities
    ///   do not report `memory_limits`.
    pub fn limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    /// Adds host bindings. Takes effect at [`Runtime::start`].
    ///
    /// # Arguments
    /// * `bindings` - natives and modules specific to this kind of host.
    pub fn register(&mut self, bindings: Bindings<V>) {
        self.bindings.extend(bindings);
    }

    /// # Returns
    /// A handle that posts tasks to this loop from any thread.
    pub fn poster(&self) -> Poster<V> {
        self.poster.clone()
    }

    /// Queues a task for the next loop step. See [`Poster::post`].
    ///
    /// # Arguments
    /// * `task` - runs on the loop thread with the context handle.
    pub fn post(&self, task: impl for<'s> FnOnce(&V::Scope<'s>) + Send + 'static) {
        self.poster.post(task);
    }

    /// Holds the loop alive, or lets it go idle again.
    ///
    /// A loop with no open handle, timer or job is idle, and [`Runtime::poll`]
    /// returns at once. A host whose events arrive as posted tasks (a desktop
    /// host: user input comes from the UI thread) holds the loop, so
    /// `poll(Wait::Block)` sleeps until a task is posted instead of spinning.
    ///
    /// # Arguments
    /// * `held` - `true` to hold; `false` to let the loop report idle again.
    pub fn hold(&mut self, held: bool) {
        self.shared.held.set(held);
    }

    /// # Returns
    /// The pulse: a counter that is odd while a script is running and changes at
    /// every JS entry and exit. It only moves when the host type sets
    /// [`Host::PULSE`]. A watchdog thread that reads the same odd value twice, a
    /// budget apart, knows one entry has run that long and can
    /// [`Interrupt::request`] it.
    pub fn pulse(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.shared.pulse.clone()
    }

    /// # Returns
    /// The instance's termination handle, usable from any thread; `None` before
    /// `start`.
    pub fn interrupt(&self) -> Option<Interrupt> {
        self.engine.as_ref().map(|engine| engine.instance.interrupt())
    }

    /// Attaches a debugger to the started Runtime. Call it after [`Runtime::start`]
    /// and before [`Runtime::load`]: modules compiled after it are debuggable.
    ///
    /// The returned handle is the protocol side; drive it with
    /// `lunarstorm_debug::debug::DapAdapter` on another thread. Requests wake this
    /// loop, which serves them at its next step.
    ///
    /// # Arguments
    /// * `options` - whether to pause before the entry module runs.
    ///
    /// # Returns
    /// The session controller.
    ///
    /// # Errors
    /// The Runtime is not started, the engine has no debugger, or one is attached.
    pub fn debug(&mut self, options: lunarstorm_vm::lunarstorm_debug::debug::DebugSessionOptions) -> Result<lunarstorm_vm::lunarstorm_debug::debug::VmDebugHandle, String> {
        let engine = self.engine.as_ref().ok_or_else(|| String::from("the runtime is not started"))?;
        let waker = self.poster.waker.clone();
        engine.instance.debug(&engine.context, Arc::new(move || {
            let _ = waker.wake();
        }), options)
    }

    /// Creates the VM instance and its context, installs the Runtime's natives and
    /// the registered host bindings, and evaluates the JS glue.
    ///
    /// Decision: a Runtime starts once. Isolation and reload are a new Runtime,
    /// which is a new instance; nothing is reset in place.
    ///
    /// # Errors
    /// A message when the Runtime is already started, the engine cannot be created
    /// or the glue fails to evaluate.
    pub fn start(&mut self) -> Result<(), String> {
        if self.engine.is_some() {
            return Err(String::from("the runtime is already started"));
        }
        let (installers, modules) = self.bindings.take();
        self.hosted = modules.iter().map(|(name, _)| *name).collect();
        let loader = Loader::<V, H>::new(self.rejections.clone(), modules, self.sources.clone(), self.package.clone());
        let instance = V::Instance::new(&self.limits, loader)?;
        let context = instance.context()?;
        let shared = &self.shared;
        let hooks = context.enter(|scope| {
            let boot = || -> Outcome<Hooks<V>> {
                let ops = ops::install::<V, H>(&scope, shared)?;
                // Host bindings go on `ops` before the glue runs: it reads some of
                // them while it builds `console`.
                for install in installers {
                    install(&scope, &ops)?;
                }
                let bootstrap = scope.eval(BOOTSTRAP)?;
                let hooks = scope.call(&bootstrap, &[ops])?;
                let hook = |name: &str| scope.get(&hooks, name).map(|value| scope.persist(&value));
                Ok(Hooks {
                    dispatch: hook("dispatch")?,
                    ticks: hook("ticks")?,
                    describe: hook("describe")?,
                    exit_code: hook("exitCode")?,
                    invoke: hook("invoke")?,
                    namespace: None,
                })
            };
            boot().map_err(|Thrown| exception_text::<V>(&scope))
        })?;
        self.engine = Some(Engine { hooks, context, instance });
        Ok(())
    }

    /// Loads and evaluates the entry module. Its top-level code runs here, followed
    /// by the promise jobs it queued; anything it scheduled runs in later steps.
    ///
    /// An exception the module throws is uncaught: it goes to the host's log sink
    /// and `Host::exit(1)`, as in Node.
    ///
    /// # Arguments
    /// * `source` - the entry module.
    ///
    /// # Errors
    /// A message when the Runtime is not started, the file cannot be read or the
    /// host's policy refuses it, or a package is for another engine or a second
    /// package is loaded.
    pub fn load(&mut self, source: Source) -> Result<(), String> {
        let (name, code, bytecode) = match source {
            Source::File(path) => {
                let (name, code) = self.read(&path)?;
                // TypeScript, JSON and CommonJS entries become ES modules like any import.
                (name.clone(), self.sources.entry(&name, code, true, &loader::builtin).map_err(|e| e.to_string())?, false)
            }
            Source::Text { name, code } => {
                let base = self.shared.host.working_directory();
                let name = Path::new(&base).join(name).to_string_lossy().into_owned();
                (name.clone(), self.sources.entry(&name, code.into_bytes(), false, &loader::builtin).map_err(|e| e.to_string())?, false)
            }
            Source::Package(package) => {
                let manifest = package.manifest();
                if manifest.engine != engine::<V>() {
                    return Err(format!("{} is built for {}, this runtime is {}", manifest.name, manifest.engine, engine::<V>()));
                }
                let name = manifest.entry.clone();
                let code = package.bytecode(&name).expect("a verified package holds its entry").to_vec();
                self.package.set(package).map_err(|_| String::from("a package is already loaded"))?;
                (name, code, true)
            }
        };
        let source_map = if bytecode { None } else { self.sources.source_map(&name) };
        let namespace = self.enter(|turn, _, _, _| {
            let source = ModuleSource { source: code, meta: Some(ModuleMeta { filename: name.clone(), main: true }), bytecode, source_map };
            // The evaluation promise is dropped on purpose: a rejection (a throw at
            // top level) reaches the rejection callback like any other.
            turn.entry();
            turn.beat();
            let namespace = match turn.scope.module(&name, source) {
                Ok((namespace, _promise)) => turn.scope.persist(&namespace),
                Err(Thrown) => {
                    // Under the default host this exits; a host that survives
                    // uncaught errors gets the failure as `load`'s error.
                    turn.raise();
                    turn.beat();
                    return Err(format!("{name} failed to load"));
                }
            };
            turn.settle();
            turn.beat();
            turn.drain_deferred();
            Ok(namespace)
        })??;
        if let Some(engine) = self.engine.as_mut() {
            engine.hooks.namespace = Some(namespace);
        }
        Ok(())
    }

    /// Compiles an entry file and every module it reaches into a package. Each
    /// module is resolved, transformed and compiled exactly as [`Runtime::load`]
    /// would load it; its import table records what each specifier resolved to.
    ///
    /// Decisions:
    /// - Module names stay the canonical build-time paths, so stack traces and
    ///   `import.meta` of a packaged app name the source files it was built from.
    /// - Only static imports, re-exports and `import()` of a string literal are
    ///   followed; a computed `import()` reaches host modules and builtins only.
    ///
    /// # Arguments
    /// * `entry` - the entry file.
    /// * `name`, `version`, `release` - the package's identity.
    ///
    /// # Returns
    /// The builder, ready for resources and metadata, then signing.
    ///
    /// # Errors
    /// A message when the Runtime is not started, the engine cannot write
    /// bytecode, or a module cannot be read, resolved or compiled.
    pub fn package(&mut self, entry: &Path, name: &str, version: &str, release: u64) -> Result<lunarstorm_jbn::Builder, String> {
        if !V::capabilities().bytecode_cache {
            return Err(format!("{} cannot write bytecode", V::descriptor().adapter));
        }
        let (entry_name, code) = self.read(entry)?;
        let code = self.sources.entry(&entry_name, code, true, &loader::builtin).map_err(|e| e.to_string())?;
        let hosted = self.hosted.clone();
        let native = |specifier: &str| loader::native(hosted.iter().copied(), specifier);
        let mut graph = Vec::new();
        let mut queue = vec![(entry_name.clone(), code)];
        let mut seen = std::collections::HashSet::from([entry_name.clone()]);
        while let Some((module, code)) = queue.pop() {
            let text = String::from_utf8(code).map_err(|_| format!("{module}: not UTF-8"))?;
            let mut table = Vec::new();
            for specifier in lunarstorm_modules::transform::imports(&module, &text).map_err(|e| e.to_string())? {
                let target = loader::resolve(&native, &self.sources, &module, &specifier)
                    .map_err(|e| format!("{module}: cannot resolve {specifier}: {e}"))?;
                if native(&target).as_deref() != Some(target.as_str()) && seen.insert(target.clone()) {
                    let source = self.sources.load(&target, &native).map_err(|e| format!("{target}: {e}"))?;
                    queue.push((target.clone(), source));
                }
                table.push((specifier, target));
            }
            graph.push((module, text, table));
        }
        let compiled = self.enter(|turn, _, _, _| {
            graph
                .into_iter()
                .map(|(module, text, table)| {
                    let source = ModuleSource { source: text.into_bytes(), meta: None, bytecode: false, source_map: None };
                    match turn.scope.compile(&module, source) {
                        Ok(bytecode) => Ok((module, bytecode, table)),
                        Err(Thrown) => Err(format!("{module}: {}", exception_text::<V>(&turn.scope))),
                    }
                })
                .collect::<Result<Vec<_>, String>>()
        })??;
        let mut builder = lunarstorm_jbn::Builder::new(name, version, release, &engine::<V>(), &entry_name);
        for (module, bytecode, table) in compiled {
            builder.module(&module, bytecode, table);
        }
        Ok(builder)
    }

    /// Reads an entry file under the host's policy.
    ///
    /// # Returns
    /// Its canonical name and bytes.
    ///
    /// # Errors
    /// A message naming the file when it cannot be read or the policy refuses it.
    fn read(&self, path: &Path) -> Result<(String, Vec<u8>), String> {
        let unreadable = |reason: &dyn std::fmt::Display| format!("cannot read {}: {reason}", path.display());
        let absolute = std::fs::canonicalize(path).map_err(|e| unreadable(&e))?;
        let name = absolute.to_string_lossy().into_owned();
        if !self.shared.host.allow(Access::Read(&name)) {
            return Err(unreadable(&"permission denied by host policy"));
        }
        let code = std::fs::read(&absolute).map_err(|e| unreadable(&e))?;
        Ok((name, code))
    }

    /// Calls a function the entry module exports. The export may be synchronous or
    /// asynchronous; either way its outcome is collected with [`Runtime::result`]
    /// once the promise settles, which for a synchronous export is immediately.
    ///
    /// Decision: the arguments are marshalled into one small vector per call. This
    /// is the host-rate entry; the event path (`dispatch`) allocates nothing.
    ///
    /// # Arguments
    /// * `export` - name of the exported function.
    /// * `args` - arguments, converted to JS values.
    ///
    /// # Returns
    /// The id to collect the result with.
    ///
    /// # Errors
    /// A message when no module is loaded or the export is not a function.
    pub fn call(&mut self, export: &str, args: &[Arg]) -> Result<CallId, String> {
        let id = self.next_call;
        self.next_call = id.wrapping_add(1);
        self.enter(|turn, _, _, hooks| {
            let scope = turn.scope.clone();
            let namespace = hooks.namespace.as_ref().ok_or_else(|| String::from("no module is loaded"))?;
            let function = match scope.get(&scope.local(namespace), export) {
                Ok(function) if scope.kind(&function) == Kind::Function => function,
                Ok(_) => return Err(format!("export '{export}' is not a function")),
                Err(Thrown) => return Err(exception_text::<V>(&scope)),
            };
            let mut values = Vec::with_capacity(args.len() + 2);
            values.push(function);
            values.push(scope.number(f64::from(id)));
            for arg in args {
                let value = match arg {
                    Arg::Null => Ok(scope.null()),
                    Arg::Bool(flag) => Ok(scope.boolean(*flag)),
                    Arg::Number(number) => Ok(scope.number(*number)),
                    Arg::Text(text) => scope.string(text),
                    Arg::Bytes(bytes) => scope.bytes_copy(bytes),
                };
                values.push(value.map_err(|Thrown| exception_text::<V>(&scope))?);
            }
            turn.entry();
            if scope.call(&scope.local(&hooks.invoke), &values).is_err() {
                turn.raise();
            }
            turn.settle();
            turn.drain_deferred();
            Ok(())
        })??;
        Ok(CallId(id))
    }

    /// Collects the outcome of a [`Runtime::call`].
    ///
    /// # Arguments
    /// * `call` - the id `call` returned.
    ///
    /// # Returns
    /// `None` while the promise is pending. Then, once: `Ok` with the value it
    /// resolved to, or `Err` with the rejection described as `console` would.
    pub fn result(&mut self, call: CallId) -> Option<Result<Arg, String>> {
        let mut calls = self.shared.calls.borrow_mut();
        let at = calls.iter().position(|(id, _)| *id == call.0)?;
        Some(calls.swap_remove(at).1)
    }

    /// Runs one step of the event loop.
    ///
    /// # Arguments
    /// * `wait` - whether the step may sleep for readiness.
    ///
    /// # Returns
    /// [`State::Idle`] when nothing can produce another event (or the Runtime is
    /// not started); [`State::Running`] otherwise.
    pub fn poll(&mut self, wait: Wait) -> State {
        self.enter(|turn, reactor, events, _| turn.step(reactor, events, wait)).unwrap_or(State::Idle)
    }

    /// Runs the loop until it is idle, then stops the Runtime. For hosts whose
    /// only loop is this one.
    ///
    /// # Returns
    /// `process.exitCode`, or 0.
    pub fn run(&mut self) -> i32 {
        // The whole loop runs inside one scope entry.
        let _ = self.enter(|turn, reactor, events, _| while turn.step(reactor, events, Wait::Block) == State::Running {});
        self.stop()
    }

    /// Stops the Runtime: flushes stdout to the host and destroys the VM instance.
    /// Open sockets close when the Runtime is dropped.
    ///
    /// # Returns
    /// `process.exitCode`, or 0; 0 when the Runtime was not started.
    pub fn stop(&mut self) -> i32 {
        let code = self.enter(|turn, _, _, hooks| {
            let exit_code = turn.scope.local(&hooks.exit_code);
            let code = turn.scope.call(&exit_code, &[]).ok().and_then(|code| turn.scope.as_number(&code));
            code.unwrap_or(0.0) as i32
        });
        self.shared.io.borrow_mut().flush_stdout(&self.shared.host);
        self.rejections.borrow_mut().clear();
        self.engine = None;
        code.unwrap_or(0)
    }

    /// Enters the VM scope and hands the loop state to a closure.
    ///
    /// # Arguments
    /// * `f` - receives the per-entry loop state, the readiness source, its event
    ///   buffer and the hooks.
    ///
    /// # Returns
    /// The closure's result.
    ///
    /// # Errors
    /// A message when the Runtime is not started.
    fn enter<R>(
        &mut self,
        f: impl for<'a, 's> FnOnce(&mut Turn<'a, 's, V, H>, &mut Poll, &mut Events, &'a Hooks<V>) -> R,
    ) -> Result<R, String> {
        let Self { engine, shared, rejections, reactor, events, scratch, tasks, poster, polls, polled, .. } = self;
        let engine = engine.as_ref().ok_or_else(|| String::from("the runtime is not started"))?;
        let hooks = &engine.hooks;
        let instance = &engine.instance;
        Ok(engine.context.enter(|scope| {
            let mut turn = Turn {
                instance,
                dispatch: scope.local(&hooks.dispatch),
                ticks: scope.local(&hooks.ticks),
                describe: scope.local(&hooks.describe),
                scope,
                shared: &**shared,
                rejections: &*rejections,
                scratch,
                tasks: &*tasks,
                pending: &poster.pending,
                polls,
                polled,
            };
            f(&mut turn, reactor, events, hooks)
        }))
    }
}

/// Describes the pending exception without the glue, for errors raised before or
/// outside it.
///
/// # Arguments
/// * `scope` - the context handle; its pending exception is taken.
///
/// # Returns
/// `Name: message` for an error object, the text of a thrown string, or a
/// placeholder.
fn exception_text<'s, V: Vm>(scope: &V::Scope<'s>) -> String {
    let thrown = scope.exception();
    if let Ok(text) = scope.with_str(&thrown, str::to_owned) {
        return text;
    }
    let field = |key: &str| {
        let value = scope.get(&thrown, key).ok()?;
        scope.with_str(&value, str::to_owned).ok()
    };
    match (field("name"), field("message")) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (_, Some(message)) => message,
        _ => {
            // A failed property read left its own exception pending; drop it.
            let _ = scope.exception();
            String::from("unknown error")
        }
    }
}

/// Outcome of trying to take one request from an HTTP connection.
enum Take {
    /// A request of this many bytes was dispatched and answered; keep going.
    Next(usize),
    /// The window does not hold a complete request.
    NeedMore,
    /// Stop pumping: the connection is gone, closing, or waiting for an async
    /// response. Carries the bytes consumed by a request dispatched just before.
    Stop(usize),
}

/// The loop's state for one scope entry: the context handle, the hooks as scoped
/// values, and borrows of what the Runtime owns.
struct Turn<'a, 's, V: Vm, H: Host> {
    /// The context handle.
    scope: V::Scope<'s>,
    /// The engine instance, for the debugger hooks.
    instance: &'a V::Instance,
    /// JS entry point for every event.
    dispatch: V::Value<'s>,
    /// Runs the `process.nextTick` queue.
    ticks: V::Value<'s>,
    /// Formats a thrown value for the uncaught-error record.
    describe: V::Value<'s>,
    /// State shared with the native functions.
    shared: &'a Shared<H>,
    /// Unhandled promise rejections, filled by the VM callbacks.
    rejections: &'a Rejections<V>,
    /// Read buffer shared by every socket.
    scratch: &'a mut [u8],
    /// Tasks posted from other threads.
    tasks: &'a Receiver<Task<V>>,
    /// Set by a poster when the task queue became non-empty.
    pending: &'a AtomicBool,
    /// Kernel readiness polls made so far.
    polls: &'a mut u64,
    /// When readiness was last polled while work was queued; see [`Runtime`].
    polled: &'a mut Option<Instant>,
}

impl<'s, V: Vm, H: Host> Turn<'_, 's, V, H> {
    /// Runs one step of the loop.
    ///
    /// # Arguments
    /// * `reactor` - the readiness source.
    /// * `events` - buffer for this step's readiness events.
    /// * `wait` - whether the step may sleep.
    ///
    /// # Returns
    /// [`State::Idle`] when there was nothing to wait for; otherwise
    /// [`State::Running`] after the step.
    fn step(&mut self, reactor: &mut Poll, events: &mut Events, wait: Wait) -> State {
        // Phase 0: debugger requests (pause, resume, breakpoints, inspection). A
        // client that asks the program to stop ends the loop.
        match self.instance.debug_service() {
            Ok(false) => {}
            Ok(true) => return State::Idle,
            Err(e) => self.die(&format!("debugger: {e}")),
        }

        // Phase 1: timers.
        self.run_timers();

        // Phase 2: poll. Sleep exactly as long as the next timer allows.
        let host = &self.shared.host;
        let (timeout, queued) = {
            let mut io = self.shared.io.borrow_mut();
            io.flush_stdout(host);
            io.date_fresh = false;
            if self.shared.immediate.get() || self.pending.load(Ordering::Acquire) {
                (Some(Duration::ZERO), true)
            } else if let Some(deadline) = io.next_deadline() {
                (Some(deadline.saturating_duration_since(host.monotonic())), false)
            } else if io.alive() || self.shared.held.get() {
                (None, false)
            } else {
                return State::Idle;
            }
        };
        let timeout = if wait == Wait::NoBlock { Some(Duration::ZERO) } else { timeout };
        // Fix: with an immediate or a posted task queued, every step used to make a
        // zero-timeout kernel poll, so a chain of `setImmediate` callbacks paid one
        // syscall each. Such a step now polls only when `POLL_GAP` has passed.
        // Dropped alternative: polling every Nth step, which bounds the number of
        // callbacks I/O waits behind but not the time.
        if self.poll_due(queued) {
            *self.polls += 1;
            match reactor.poll(events, timeout) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => self.die(&format!("poll failed: {e}")),
            }

            // Phase 3: I/O. Only after a poll: the buffer otherwise still holds the
            // events of an earlier step.
            for event in events.iter() {
                self.on_event(event);
            }
        }

        // Phase 4: tasks posted from other threads.
        if self.pending.load(Ordering::Acquire) {
            self.drain_tasks();
        }

        // Phase 5: immediates.
        if self.shared.immediate.replace(false) {
            self.run_immediates();
        }
        State::Running
    }

    /// Decides whether this step asks the kernel for readiness.
    ///
    /// Decision: the step that has nothing queued always polls and reads no clock,
    /// so the I/O path costs what it did before. Only a step with queued work reads
    /// the clock, and the gap is measured from the first such step after a poll.
    ///
    /// # Arguments
    /// * `queued` - whether an immediate or a posted task is waiting, which makes
    ///   the poll a readiness check rather than a wait.
    ///
    /// # Returns
    /// `true` to poll now.
    #[inline]
    fn poll_due(&mut self, queued: bool) -> bool {
        if !queued {
            *self.polled = None;
            return true;
        }
        let now = self.shared.host.monotonic();
        match *self.polled {
            Some(last) if now.saturating_duration_since(last) < POLL_GAP => false,
            Some(_) => {
                *self.polled = Some(now);
                true
            }
            // The previous step polled (or none has run yet): readiness is fresh.
            None => {
                *self.polled = Some(now);
                false
            }
        }
    }

    // ------------------------------------------------------------ JS entries

    /// # Arguments
    /// * `value` - an event code or slab key.
    ///
    /// # Returns
    /// It as a JS integer.
    #[inline]
    fn int(&self, value: i32) -> V::Value<'s> {
        self.scope.int(value)
    }

    /// # Arguments
    /// * `text` - text for an event argument.
    ///
    /// # Returns
    /// It as a JS string. Allocation failure is fatal.
    fn text(&self, text: &str) -> V::Value<'s> {
        match self.scope.string(text) {
            Ok(value) => value,
            Err(Thrown) => self.fatal(),
        }
    }

    /// Calls the JS `dispatch` function with one event and settles the engine.
    ///
    /// # Arguments
    /// * `args` - event code followed by the event's arguments.
    fn fire(&mut self, args: &[V::Value<'s>]) {
        self.entry();
        self.beat();
        if self.scope.call(&self.dispatch, args).is_err() {
            self.raise();
        }
        self.settle();
        self.beat();
    }

    /// Moves the pulse: called once before and once after each JS entry, so the
    /// counter is odd exactly while a script runs. Compiled out unless the host
    /// type asks for the pulse.
    #[inline(always)]
    fn beat(&self) {
        if H::PULSE {
            let pulse = &self.shared.pulse;
            // One writer (this thread), so a plain load and store is enough.
            pulse.store(pulse.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
        }
    }

    /// Brings the engine to rest after a JS call, with Node's ordering: all
    /// `nextTick` callbacks, then all promise jobs, repeated until both are empty.
    /// Then reports any promise rejection that is still unhandled.
    fn settle(&mut self) {
        // Fix: a script that caught an out-of-memory error kept running on an
        // exhausted heap. The engine's sticky mark is checked before and after the
        // queued jobs run, so a caught failure ends the program like an uncaught one.
        self.check_memory();
        loop {
            if self.shared.tick.replace(false) && self.scope.call(&self.ticks, &[]).is_err() {
                self.raise();
            }
            while self.scope.run_job() {}
            if !self.shared.tick.get() {
                break;
            }
        }
        self.check_memory();
        // The job queue is empty: the debugger qualifies unhandled rejections now,
        // before they are reported below.
        if let Err(e) = self.instance.debug_checkpoint() {
            self.die(&format!("debugger: {e}"));
        }
        // Each unhandled rejection is reported: under the default host the first
        // one exits, and a host that survives them sees every one.
        while !self.rejections.borrow().is_empty() {
            let (_, reason) = self.rejections.borrow_mut().remove(0);
            let text = self.describe_value(&self.scope.local(&reason));
            self.uncaught(true, &text);
        }
    }

    /// Before a JS entry: lets an attached debugger hold it (break on start).
    fn entry(&self) {
        if let Err(e) = self.instance.debug_entry() {
            self.die(&format!("debugger: {e}"));
        }
    }

    /// Exits with status 1 when an allocation failed since the last check, whether
    /// or not the script caught the error ([`Scope::take_out_of_memory`]).
    fn check_memory(&self) {
        if self.scope.take_out_of_memory() {
            self.die("the engine ran out of memory");
        }
    }

    /// Formats a JS value with the glue's inspector.
    ///
    /// # Arguments
    /// * `value` - a thrown value or rejection reason.
    ///
    /// # Returns
    /// Its text: name, message and stack for errors.
    fn describe_value(&self, value: &V::Value<'s>) -> String {
        let described = self.scope.call(&self.describe, std::slice::from_ref(value));
        match described.and_then(|text| self.scope.with_str(&text, str::to_owned)) {
            Ok(text) => text,
            Err(Thrown) => {
                let _ = self.scope.exception();
                String::from("<unprintable>")
            }
        }
    }

    /// Reports the pending exception as an uncaught script error. Whether the
    /// program goes on is the host's choice ([`Host::uncaught`]).
    fn raise(&self) {
        let thrown = self.scope.exception();
        let text = self.describe_value(&thrown);
        self.uncaught(false, &text);
    }

    /// Reports the pending exception and exits with status 1. For failures the
    /// loop itself cannot continue from (the engine could not allocate an event's
    /// argument), whatever the host's policy for script errors is.
    fn fatal(&self) -> ! {
        let thrown = self.scope.exception();
        let text = self.describe_value(&thrown);
        self.die(&text)
    }

    /// Flushes stdout and hands an uncaught script error to the host.
    ///
    /// # Arguments
    /// * `in_promise` - whether it is a rejected promise rather than a thrown
    ///   exception.
    /// * `description` - the error as `console` would show it.
    fn uncaught(&self, in_promise: bool, description: &str) {
        let host = &self.shared.host;
        self.shared.io.borrow_mut().flush_stdout(host);
        host.uncaught(in_promise, description);
    }

    /// Flushes stdout, logs an error the loop cannot recover from and exits with
    /// status 1.
    ///
    /// # Arguments
    /// * `message` - what failed.
    fn die(&self, message: &str) -> ! {
        let host = &self.shared.host;
        self.shared.io.borrow_mut().flush_stdout(host);
        host.log(Log::Error(message));
        host.exit(1)
    }

    /// Delivers the events that ops queued while JS was running.
    fn drain_deferred(&mut self) {
        loop {
            let next = self.shared.io.borrow_mut().take_deferred();
            match next {
                None => return,
                Some(Deferred::Close(key, fault)) => self.fire_close(key, fault),
                Some(Deferred::Drain(key)) => self.fire(&[self.int(ev::DRAIN), self.int(key as i32)]),
                Some(Deferred::Resume(key)) => self.http_pump(key),
            }
        }
    }

    /// Tells JS a TCP socket closed.
    ///
    /// # Arguments
    /// * `key` - slab key the socket had.
    /// * `fault` - the error that closed it, if any.
    fn fire_close(&mut self, key: usize, fault: Option<Fault>) {
        match fault {
            Some(fault) => self.fire(&[
                self.int(ev::CLOSE),
                self.int(key as i32),
                self.text(&fault.message),
                self.text(fault.code),
            ]),
            None => self.fire(&[self.int(ev::CLOSE), self.int(key as i32)]),
        }
    }

    // ------------------------------------------------------------ loop phases

    /// Fires every timer that is due. Each callback is its own JS entry, so promise
    /// jobs run between timers.
    fn run_timers(&mut self) {
        let mut now = None;
        loop {
            let due = {
                let mut io = self.shared.io.borrow_mut();
                // Read the clock only when a timer exists, and once per phase.
                io.next_deadline()
                    .and_then(|_| io.timer_pop(*now.get_or_insert_with(|| self.shared.host.monotonic())))
            };
            let Some((key, one_shot)) = due else { return };
            self.fire(&[self.int(ev::TIMER), self.int(key as i32), self.scope.boolean(one_shot)]);
            self.drain_deferred();
        }
    }

    /// Runs the `setImmediate` callbacks queued before this phase, one JS entry each.
    fn run_immediates(&mut self) {
        loop {
            self.entry();
            self.beat();
            let more = match self.scope.call(&self.dispatch, &[self.int(ev::IMMEDIATE)]) {
                Ok(more) => self.scope.as_bool(&more).unwrap_or(false),
                // The callback that threw was already taken off the queue; the
                // next phase runs whatever is left.
                Err(Thrown) => {
                    self.raise();
                    false
                }
            };
            self.settle();
            self.beat();
            self.drain_deferred();
            if !more {
                return;
            }
        }
    }

    /// Runs the tasks other threads posted, one JS entry each.
    fn drain_tasks(&mut self) {
        // Cleared before draining, so a task posted during the drain wakes the loop
        // again instead of being missed.
        self.pending.store(false, Ordering::Release);
        while let Ok(task) = self.tasks.try_recv() {
            self.entry();
            self.beat();
            task(&self.scope);
            self.settle();
            self.beat();
            self.drain_deferred();
        }
    }

    /// Handles one readiness event.
    ///
    /// A key can be reused within one batch of events, so an event may arrive for a
    /// newer handle than the one it was raised for. That is harmless by construction:
    /// a spurious read returns `WouldBlock` and a spurious flush finds nothing queued.
    ///
    /// # Arguments
    /// * `event` - the event from mio.
    fn on_event(&mut self, event: &Event) {
        let token = event.token();
        if token == WAKER {
            // Posted tasks share this waker; the step drains them after the batch.
            self.drain_pool();
            return;
        }
        let key = token.0;
        let (listener, http) = match self.shared.io.borrow().entries.get(key) {
            Some(Entry::Listener { http, .. }) => (true, *http),
            Some(Entry::Conn(conn)) => (false, conn.flags & flag::HTTP != 0),
            _ => return,
        };
        if listener {
            self.accept(key, http);
        } else {
            if event.is_writable() {
                self.on_writable(key);
            }
            let hup = event.is_read_closed() || event.is_error();
            if event.is_readable() || hup {
                if http {
                    self.http_readable(key, hup);
                } else {
                    self.tcp_read(key, hup);
                }
            }
        }
        self.drain_deferred();
    }

    /// Accepts every pending connection on a listener.
    ///
    /// # Arguments
    /// * `key` - slab key of the listener.
    /// * `http` - whether its connections speak HTTP.
    fn accept(&mut self, key: usize, http: bool) {
        loop {
            let accepted = {
                let mut io = self.shared.io.borrow_mut();
                let Some(Entry::Listener { sock, .. }) = io.entries.get(key) else { return };
                match sock.accept() {
                    Ok((stream, peer)) => io.adopt(stream, key, http).map(|conn| (conn, peer)),
                    Err(e) => Err(e),
                }
            };
            match accepted {
                // HTTP connections have no JS object until a request arrives.
                Ok(_) if http => {}
                Ok((conn, peer)) => self.fire(&[
                    self.int(ev::CONNECTION),
                    self.int(key as i32),
                    self.int(conn as i32),
                    self.text(&peer.ip().to_string()),
                    self.int(i32::from(peer.port())),
                ]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // Typically EMFILE or ECONNABORTED: drop this connection, keep serving.
                Err(e) => {
                    self.shared.host.log(Log::Warning(&format!("accept failed: {e}")));
                    return;
                }
            }
        }
    }

    /// Handles a writable event.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    fn on_writable(&mut self, key: usize) {
        let wake = self.shared.io.borrow_mut().writable(key);
        match wake {
            Wake::Quiet => {}
            Wake::Connected => self.fire(&[self.int(ev::CONNECT), self.int(key as i32)]),
            Wake::Failed(fault) => self.fire_close(key, Some(fault)),
        }
    }

    /// Reads a TCP socket and delivers the bytes to JS.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `hup` - whether the kernel reported the peer's close; when set, the socket is
    ///   read to end of stream instead of stopping at a short read.
    fn tcp_read(&mut self, key: usize, hup: bool) {
        loop {
            let read = {
                let mut io = self.shared.io.borrow_mut();
                let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return };
                // After end of stream there is nothing to read; before the connect
                // completes, the writable handler reports success or failure.
                if conn.flags & (flag::READ_EOF | flag::CONNECTING) != 0 {
                    return;
                }
                conn.stream.read(self.scratch)
            };
            match read {
                Ok(0) => {
                    self.shared.io.borrow_mut().read_eof(key);
                    self.fire(&[self.int(ev::END), self.int(key as i32)]);
                    self.shared.io.borrow_mut().finish_read(key);
                    return;
                }
                Ok(n) => {
                    let chunk = match self.scope.bytes_copy(&self.scratch[..n]) {
                        Ok(chunk) => chunk,
                        Err(Thrown) => self.fatal(),
                    };
                    self.fire(&[self.int(ev::DATA), self.int(key as i32), chunk]);
                    if n < self.scratch.len() && !hup {
                        return;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.shared.io.borrow_mut().close(key);
                    self.fire_close(key, Some(Fault::new(&e, "read", "")));
                    return;
                }
            }
        }
    }

    /// Delivers worker-pool results to JS.
    fn drain_pool(&mut self) {
        self.shared.io.borrow().pool.begin_drain();
        loop {
            let message = self.shared.io.borrow_mut().pool.try_recv();
            match message {
                None => return,
                Some(ResultMsg::Fs { req, result: Ok(value) }) => match ops::fs_value::<V>(&self.scope, value) {
                    Ok(value) => self.fire(&[self.int(ev::FS), self.int(req), value]),
                    Err(Thrown) => self.fatal(),
                },
                Some(ResultMsg::Fs { req, result: Err(fault) }) => self.fire(&[
                    self.int(ev::FS),
                    self.int(req),
                    self.scope.undefined(),
                    self.text(&fault.message),
                    self.text(fault.code),
                ]),
                Some(ResultMsg::Resolved { conn, addr }) => {
                    self.shared.io.borrow_mut().connect(conn, addr);
                }
            }
            self.drain_deferred();
        }
    }

    // -------------------------------------------------------------- HTTP pump

    /// Handles a readable event on an HTTP connection.
    ///
    /// While a response is in flight the socket is deliberately left unread: the
    /// kernel buffer fills and TCP flow control slows the client, instead of this
    /// process buffering without bound.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `hup` - whether the kernel reported the peer's close.
    fn http_readable(&mut self, key: usize, hup: bool) {
        {
            let mut io = self.shared.io.borrow_mut();
            let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return };
            conn.flags |= if hup { flag::READABLE | flag::RDHUP } else { flag::READABLE };
            if conn.flags & flag::BUSY != 0 {
                return;
            }
        }
        self.http_pump(key);
    }

    /// Drives an HTTP connection: dispatches buffered requests, then reads and
    /// dispatches until the socket is drained or a handler goes asynchronous.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    fn http_pump(&mut self, key: usize) {
        loop {
            // (1) Requests already buffered on the connection.
            loop {
                match self.http_take(key, None) {
                    Take::Next(_) => {}
                    Take::NeedMore => break,
                    Take::Stop(_) => return,
                }
            }

            // (2) Refill from the socket.
            let (n, in_place) = {
                let mut guard = self.shared.io.borrow_mut();
                let io = &mut *guard;
                let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return };
                if conn.flags & flag::READABLE == 0 {
                    return;
                }
                match conn.stream.read(self.scratch) {
                    // End of stream with no request in flight: nothing more can come.
                    Ok(0) => {
                        io.close(key);
                        return;
                    }
                    Ok(n) => {
                        if n < self.scratch.len() && conn.flags & flag::RDHUP == 0 {
                            conn.flags &= !flag::READABLE;
                        }
                        let in_place = conn.read_buf.is_empty();
                        if !in_place {
                            conn.read_buf.extend_from_slice(&self.scratch[..n]);
                        }
                        (n, in_place)
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        conn.flags &= !flag::READABLE;
                        return;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        io.close(key);
                        return;
                    }
                }
            };
            if !in_place {
                continue;
            }

            // (3) Fast path: parse straight out of the scratch buffer. Only bytes of
            // an incomplete request are copied into the connection's buffer.
            let mut at = 0;
            while at < n {
                match self.http_take(key, Some((at, n))) {
                    Take::Next(used) => at += used,
                    Take::NeedMore => {
                        self.stash(key, at, n);
                        break;
                    }
                    Take::Stop(used) => {
                        self.stash(key, at + used, n);
                        return;
                    }
                }
            }
        }
    }

    /// Saves unparsed scratch bytes on the connection.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `from` - start of the unparsed bytes in the scratch buffer.
    /// * `to` - end of the bytes read.
    fn stash(&mut self, key: usize, from: usize, to: usize) {
        if from < to {
            if let Some(Entry::Conn(conn)) = self.shared.io.borrow_mut().entries.get_mut(key) {
                conn.read_buf.extend_from_slice(&self.scratch[from..to]);
            }
        }
    }

    /// Tries to take one request from a window and dispatch it to JS.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `window` - `Some((start, end))` to parse that range of the scratch buffer,
    ///   `None` to parse the connection's own buffer.
    ///
    /// # Returns
    /// What the pump should do next; see [`Take`].
    fn http_take(&mut self, key: usize, window: Option<(usize, usize)>) -> Take {
        let (owner, used, args) = {
            let mut guard = self.shared.io.borrow_mut();
            let io = &mut *guard;
            let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return Take::Stop(0) };
            if conn.flags & (flag::BUSY | flag::ENDING) != 0 {
                return Take::Stop(0);
            }
            let buf: &[u8] = match window {
                Some((start, end)) => &self.scratch[start..end],
                None => &conn.read_buf,
            };
            let step = if buf.is_empty() { Step::NeedMore } else { http::advance(buf, &mut conn.phase) };
            let request = match step {
                Step::Request(request) => request,
                Step::Bad(status) => {
                    io.http_reject(key, status, &self.shared.host);
                    return Take::Stop(0);
                }
                waiting => {
                    if matches!(waiting, Step::Continue) {
                        let _ = conn.send([b"HTTP/1.1 100 Continue\r\n\r\n", &[], &[]]);
                    }
                    // The batch of pipelined requests ended: send what was corked.
                    if conn.flags & flag::CORKED != 0 {
                        io.uncork(key);
                    }
                    return Take::NeedMore;
                }
            };
            let head = &request.head;
            let mut flags = conn.flags & !(flag::KEEP_ALIVE | flag::HEAD_ONLY | flag::V10)
                | flag::BUSY
                | flag::IN_PUMP;
            if head.keep_alive {
                flags |= flag::KEEP_ALIVE;
            }
            if head.http10 {
                flags |= flag::V10;
            }
            // More requests follow in this window: batch the responses.
            if request.consumed < buf.len() {
                flags |= flag::CORKED;
            }
            let method = &buf[head.method.0 as usize..head.method.1 as usize];
            if method == b"HEAD" {
                flags |= flag::HEAD_ONLY;
            }
            conn.flags = flags;
            let used = request.consumed;
            let args = request_args::<V>(&self.scope, buf, request);
            if window.is_none() {
                conn.read_buf.advance(used);
            }
            (conn.owner, used, args)
        };
        let [method, url, fields, body] = match args {
            Ok(args) => args,
            Err(Thrown) => self.fatal(),
        };
        self.fire(&[self.int(ev::HTTP), self.int(owner as i32), self.int(key as i32), method, url, fields, body]);

        let mut guard = self.shared.io.borrow_mut();
        let io = &mut *guard;
        let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return Take::Stop(used) };
        conn.flags &= !flag::IN_PUMP;
        if conn.flags & (flag::BUSY | flag::ENDING) != 0 {
            // The handler went asynchronous or the connection is closing: nothing
            // more will be batched, so release earlier responses now.
            io.uncork(key);
            return Take::Stop(used);
        }
        Take::Next(used)
    }
}

/// Builds the JS arguments for a request. Only allocates JS values; no JS code runs,
/// which is why it may be called while the loop state is borrowed.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `buf` - the window the request was parsed from.
/// * `request` - the parsed request.
///
/// # Returns
/// The method (an index into [`http::METHODS`], or the token for other methods), the
/// URL, the raw header-field block (parsed lazily in JS) and the body (`undefined`
/// when absent).
///
/// # Errors
/// [`Thrown`] when allocation fails.
fn request_args<'s, V: Vm>(
    scope: &V::Scope<'s>,
    buf: &[u8],
    request: http::Request,
) -> Outcome<[V::Value<'s>; 4]> {
    let head = &request.head;
    let text = |span: (u32, u32)| String::from_utf8_lossy(&buf[span.0 as usize..span.1 as usize]);
    let method = text(head.method);
    let method = match http::METHODS.iter().position(|known| *known == method) {
        Some(index) => scope.int(index as i32),
        None => scope.string(&method)?,
    };
    let url = scope.string(&text(head.path))?;
    let fields = scope.string(&text(head.fields))?;
    let body = match request.body {
        Body::None => scope.undefined(),
        Body::Span(start, end) => scope.bytes_copy(&buf[start..end])?,
        Body::Owned(bytes) => scope.bytes(bytes)?,
    };
    Ok([method, url, fields, body])
}
