//! The [`Runtime`] a host owns: its construction, `register`, `start`, `load`,
//! `call`, `poll`, `run` and `stop`. One loop step lives in [`crate::event_loop`].
//!
//! A host drives a [`Runtime`] through seven calls: [`Runtime::register`],
//! [`Runtime::start`], [`Runtime::load`], [`Runtime::call`], [`Runtime::post`],
//! [`Runtime::poll`] and [`Runtime::stop`]. [`Runtime::run`] is `poll` in a loop for
//! hosts that have no loop of their own.
//!
//! The loop is a single-threaded, edge-triggered mio reactor that owns every socket
//! and the VM instance. It never blocks on anything but `poll` and never runs
//! blocking work; that goes to the worker pool ([`crate::blocking_work_pool`]). JS runs here, one
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

use crate::runtime_public_types::{engine, Arg, CallId, Poster, Source, State, Task, Wait};
use crate::event_loop::{Turn, EVENT_BATCH, SCRATCH};
use crate::host_bindings_registry::Bindings;
use crate::extension_manifest_check::ExtensionSet;
use crate::cross_thread_message_channel::{Channel, Inbox, Nudge};
use crate::ipc_module::Ipc;
use crate::worker_threads_module::{Link, Workers};
use crate::host_services_trait::{Access, Host};
use crate::reactor_io_state::{Io, Shared, WAKER};
use crate::module_loader_callbacks::{self as loader, Loader, Registry, Rejections};
use crate::node_builtins::{self, Node};
use crate::internal_natives;
use crate::blocking_work_pool::Pool;
use crossbeam_channel::{unbounded, Receiver};
use lunarstorm_engine_contract::{
    Context, Image, Instance, Interrupt, Kind, Limits, ModuleMeta, ModuleSource, Outcome, Scope, Thrown, Vm,
};
use mio::{Events, Poll, Waker};
use std::cell::{Cell, OnceCell, RefCell};
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

/// # Returns
/// How many times this process has compiled JS glue: always 0.
///
/// Decision: the Node surface is native (`src/node/`), so `start` compiles no
/// script. The counter stays for the test that guarded the per-start compile
/// (`glue_is_compiled_once_per_process`): starting more runtimes must not raise it.
#[doc(hidden)]
pub fn glue_compiles() -> usize {
    0
}

/// The glue's hooks, kept alive across scope entries.
pub(crate) struct Hooks<V: Vm> {
    /// JS entry point for every event.
    pub(crate) dispatch: V::Persistent,
    /// Runs the `process.nextTick` queue.
    pub(crate) ticks: V::Persistent,
    /// Formats a thrown value for the uncaught-error record.
    pub(crate) describe: V::Persistent,
    /// Reads `process.exitCode`.
    pub(crate) exit_code: V::Persistent,
    /// Calls an export and reports how its promise settles.
    pub(crate) invoke: V::Persistent,
    /// Namespace of the entry module, once loaded; where `call` finds exports.
    pub(crate) namespace: Option<V::Persistent>,
}

/// One VM instance and everything that lives inside it.
///
/// Decision: field order is drop order. Persistent handles go first, then the
/// context, then the instance that owns the heap they point into.
pub(crate) struct Engine<V: Vm> {
    /// The glue's hooks.
    pub(crate) hooks: Hooks<V>,
    /// The one realm scripts run in.
    pub(crate) context: V::Context,
    /// The heap.
    pub(crate) instance: V::Instance,
}

/// A JS runtime, generic over the engine and the host that owns it.
pub struct Runtime<V: Vm, H: Host> {
    /// The VM, between `start` and `stop`.
    pub(crate) engine: Option<Engine<V>>,
    /// Loop state and host, shared with the native functions.
    pub(crate) shared: Rc<Shared<H>>,
    /// Unhandled promise rejections, filled by the VM callbacks.
    pub(crate) rejections: Rejections<V>,
    /// The builtin modules and internals `node_builtins::install` returned; handles into the heap,
    /// cleared before the instance is destroyed.
    pub(crate) registry: Rc<Registry<V>>,
    /// Files and packages, shared with the loader; the entry module goes through it too.
    pub(crate) sources: Rc<lunarstorm_module_sources::Modules>,
    /// The `.jbn` package being run, shared with the loader; set once by `load`.
    pub(crate) package: Rc<OnceCell<lunarstorm_jbn_package_format::Package>>,
    /// Specifiers of the host modules, kept by `start` for [`Runtime::package`].
    pub(crate) hosted: Vec<&'static str>,
    /// The readiness source.
    pub(crate) reactor: Poll,
    /// Readiness events of the current step.
    pub(crate) events: Events,
    /// Read buffer shared by every socket.
    pub(crate) scratch: Box<[u8]>,
    /// Host bindings waiting for `start`.
    pub(crate) bindings: Bindings<V>,
    /// Tasks posted from other threads.
    pub(crate) tasks: Receiver<Task<V>>,
    /// The sending side, cloned for each [`Runtime::poster`].
    pub(crate) poster: Poster<V>,
    /// The `lunarstorm:ipc` channel and receiver.
    pub(crate) ipc: Rc<Ipc<V>>,
    /// The `lunarstorm:worker` children and, for a worker, the link to its parent.
    pub(crate) workers: Rc<Workers<V>>,
    /// The builtins' kept values and queues; handles into the heap, cleared before
    /// the instance is destroyed.
    pub(crate) node: Rc<Node<V>>,
    /// Limits for the instance `start` creates.
    pub(crate) limits: Limits,
    /// Id of the next `call`.
    pub(crate) next_call: u32,
    /// Kernel readiness polls made so far.
    pub(crate) polls: u64,
    /// When the loop last asked the kernel for readiness while work was queued;
    /// `None` after a step that polled because nothing was queued.
    pub(crate) polled: Option<Instant>,
}

impl<V: Vm, H: Host> Drop for Runtime<V, H> {
    /// Releases the rejection reasons before the instance whose heap they live in.
    fn drop(&mut self) {
        self.rejections.borrow_mut().clear();
        self.registry.clear();
        self.node.clear();
        // Workers end first: their listeners are persistent handles into the heap.
        self.workers.shutdown(&self.shared.refs);
        // The receiver is a persistent handle into the instance's heap.
        self.ipc.clear(&self.shared.refs);
        // Fix: an `Inbox` outliving the Runtime kept accepting messages nobody would
        // ever read, so `post` returned `true` for them. Decision: dropping the
        // Runtime closes the channel, so the host sees `false` from then on.
        self.ipc.channel.close();
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
            refs: Cell::new(0),
            stop: Cell::new(false),
            terminated: Arc::default(),
            entry_dir: RefCell::new(String::new()),
            pulse: Arc::default(),
        });
        let pending = Arc::new(AtomicBool::new(false));
        let ipc = Ipc::new(Nudge::new(waker.clone(), pending.clone()));
        let workers = Workers::new(ipc.nudge_handle());
        let sources = loader::sources(&shared);
        Ok(Self {
            engine: None,
            shared,
            sources,
            package: Rc::default(),
            hosted: Vec::new(),
            rejections: Rc::default(),
            registry: Rc::default(),
            reactor,
            events: Events::with_capacity(EVENT_BATCH),
            scratch: vec![0u8; SCRATCH].into_boxed_slice(),
            bindings: Bindings::new(),
            tasks,
            poster: Poster { tasks: sender, waker, pending },
            ipc,
            workers,
            node: Node::new(),
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

    /// Lists every declared module a script or host module can reach: the
    /// Runtime's `ops`, then the registered host bindings, then the extensions
    /// the host's manifest uses. Creates no VM; works before and after `start`.
    ///
    /// Decision: declarations with the same specifier are merged into one, in
    /// that order, so `lunarstorm:internal` lists the Runtime's and the host's
    /// natives together, as the `ops` object holds them.
    ///
    /// # Returns
    /// The modules, each specifier once, in first-seen order.
    ///
    /// # Errors
    /// The manifest check's or an extension's message, as [`Runtime::start`]
    /// reports it.
    pub fn declarations(&self) -> Result<Vec<lunarstorm_engine_contract::ModuleDecl>, String> {
        let host = &self.shared.host;
        let extensions = host.extensions::<V>();
        crate::extension_manifest_check::check(host.manifest(), &ExtensionSet::<V>::offers(&extensions)).map_err(|errors| errors.join("\n"))?;
        let mut used = Bindings::<V>::new();
        ExtensionSet::<V>::install(&extensions, host.manifest(), &mut used)?;
        let mut merged: Vec<lunarstorm_engine_contract::ModuleDecl> = Vec::new();
        for decl in std::iter::once(internal_natives::declare()).chain(self.bindings.declarations()).chain(used.declarations()) {
            match merged.iter_mut().find(|known| known.specifier == decl.specifier) {
                Some(known) => known.items.extend(decl.items),
                None => merged.push(decl),
            }
        }
        Ok(merged)
    }

    /// # Returns
    /// A handle that posts tasks to this loop from any thread.
    pub fn poster(&self) -> Poster<V> {
        self.poster.clone()
    }

    /// Makes this Runtime a worker. Must run before [`Runtime::start`].
    ///
    /// # Arguments
    /// * `link` - the way back to the parent.
    /// * `inbound` - the channel the parent's messages arrive on; it replaces the
    ///   Runtime's own `lunarstorm:ipc` queue, so a worker's `onMessage` reads it.
    /// * `terminated` - the flag the parent sets to end this Runtime.
    ///
    /// # Panics
    /// When the Runtime has already started: the state is shared with the VM then.
    pub(crate) fn as_worker(&mut self, link: Link, inbound: Arc<Channel>, terminated: Arc<AtomicBool>) {
        Rc::get_mut(&mut self.ipc).expect("a Runtime becomes a worker before it starts").channel = inbound;
        // Defect fixed by the merge of the package loader: the source modules' read
        // closure holds a clone of `shared`, so `get_mut` below failed in every
        // worker. Decision: drop that closure first and build the source modules
        // again after the flag is replaced (an alternative, a `RefCell` around the
        // flag, would touch every reader of it).
        drop(std::mem::replace(&mut self.sources, Rc::new(lunarstorm_module_sources::Modules::new(Box::new(|_| Err(String::new()))))));
        Rc::get_mut(&mut self.shared).expect("a Runtime becomes a worker before it starts").terminated = terminated;
        self.sources = loader::sources(&self.shared);
        self.workers.attach(link);
    }

    /// # Returns
    /// A handle that wakes this Runtime's loop from another thread.
    pub(crate) fn nudge(&self) -> Nudge {
        self.ipc.nudge_handle()
    }

    /// # Returns
    /// The host's side of the program's `lunarstorm:ipc` channel, usable from any
    /// thread. It exists whether or not the host type enables the module
    /// ([`Host::IPC`]); a program that cannot import the module never reads it.
    pub fn inbox(&self) -> Inbox {
        self.ipc.inbox()
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
    /// `lunarstorm_debugger::debug::DapAdapter` on another thread. Requests wake this
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
    pub fn debug(&mut self, options: lunarstorm_engine_contract::lunarstorm_debugger::debug::DebugSessionOptions) -> Result<lunarstorm_engine_contract::lunarstorm_debugger::debug::VmDebugHandle, String> {
        let engine = self.engine.as_ref().ok_or_else(|| String::from("the runtime is not started"))?;
        let waker = self.poster.waker.clone();
        engine.instance.debug(&engine.context, Arc::new(move || {
            let _ = waker.wake();
        }), options)
    }

    /// Creates the VM instance and its context, installs the Runtime's natives and
    /// the registered host bindings, and the native Node builtins.
    ///
    /// Decision: a Runtime starts once. Isolation and reload are a new Runtime,
    /// which is a new instance; nothing is reset in place.
    ///
    /// # Errors
    /// A message when the Runtime is already started, the engine cannot be created
    /// or the builtins fail to install.
    pub fn start(&mut self) -> Result<(), String> {
        if self.engine.is_some() {
            return Err(String::from("the runtime is already started"));
        }
        // The host's extensions, fresh for this Runtime (a worker's come from the
        // worker's own host): the manifest is checked against what the host offers,
        // then each used extension's bindings join the host's own, before the VM
        // exists, so a manifest error leaves no instance behind.
        let host = &self.shared.host;
        let extensions = host.extensions::<V>();
        crate::extension_manifest_check::check(host.manifest(), &ExtensionSet::<V>::offers(&extensions)).map_err(|errors| errors.join("\n"))?;
        ExtensionSet::<V>::install(&extensions, host.manifest(), &mut self.bindings)?;
        let (installers, modules) = self.bindings.take();
        self.hosted = modules.iter().map(|(name, _)| *name).collect();
        let loader = Loader::<V, H>::new(
            self.shared.clone(),
            self.rejections.clone(),
            modules,
            self.ipc.clone(),
            self.workers.clone(),
            self.registry.clone(),
            self.sources.clone(),
            self.package.clone(),
        );
        let instance = V::Instance::new(&self.limits, loader)?;
        let context = instance.context()?;
        let shared = &self.shared;
        let hooks = context.enter(|scope| {
            let boot = || -> Outcome<Hooks<V>> {
                let ops = internal_natives::install::<V, H>(&scope, shared)?;
                // Host bindings go on `ops` before the builtins install: they read
                // some of them (`ops.color`) while they build `console`.
                for install in installers {
                    install(&scope, &ops)?;
                }
                // The builtins; the object they return holds the hooks the loop
                // calls and the module registry.
                let hooks = node_builtins::install::<V, H>(&scope, shared, &self.node, &ops)?;
                let hook = |name: &str| scope.get(&hooks, name).map(|value| scope.persist(&value));
                // The builtin modules and internals stay with the loader; nothing
                // of them is published on the global object.
                self.registry.set(hook("modules")?, hook("internal")?, hook("exportsOf")?);
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
        let (name, code, image) = match source {
            Source::File(path) => {
                let (name, code) = self.read(&path)?;
                // TypeScript, JSON and CommonJS entries become ES modules like any import.
                (name.clone(), self.sources.entry(&name, code, true, &loader::builtin).map_err(|e| e.to_string())?, None)
            }
            Source::Text { name, code } => {
                let base = self.shared.host.working_directory();
                let name = Path::new(&base).join(name).to_string_lossy().into_owned();
                (name.clone(), self.sources.entry(&name, code.into_bytes(), false, &loader::builtin).map_err(|e| e.to_string())?, None)
            }
            Source::Package(package) => {
                let manifest = package.manifest();
                if manifest.engine != engine::<V>() {
                    return Err(format!("{} is built for {}, this runtime is {}", manifest.name, manifest.engine, engine::<V>()));
                }
                let name = manifest.entry.clone();
                let image = Image::from_package(&package, &name).expect("a verified package holds its entry");
                self.package.set(package).map_err(|_| String::from("a package is already loaded"))?;
                (name, Vec::new(), Some(image))
            }
        };
        // Worker paths resolve against the directory of the entry module.
        let directory = Path::new(&name).parent().map_or_else(|| String::from("/"), |parent| parent.to_string_lossy().into_owned());
        *self.shared.entry_dir.borrow_mut() = directory;
        let source_map = if image.is_some() { None } else { self.sources.source_map(&name) };
        let namespace = self.enter(|turn, _, _, _| {
            let source = ModuleSource { source: code, meta: Some(ModuleMeta { filename: name.clone(), main: true }), image, source_map };
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
    pub fn package(&mut self, entry: &Path, name: &str, version: &str, release: u64) -> Result<lunarstorm_jbn_package_format::Builder, String> {
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
            for specifier in lunarstorm_module_sources::load_time_esm_transforms::imports(&module, &text).map_err(|e| e.to_string())? {
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
                    let source = ModuleSource { source: text.into_bytes(), meta: None, image: None, source_map: None };
                    match turn.scope.compile(&module, source) {
                        Ok(bytecode) => Ok((module, bytecode, table)),
                        Err(Thrown) => Err(format!("{module}: {}", exception_text::<V>(&turn.scope))),
                    }
                })
                .collect::<Result<Vec<_>, String>>()
        })??;
        let mut builder = lunarstorm_jbn_package_format::Builder::new(name, version, release, &engine::<V>(), &entry_name);
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
        self.registry.clear();
        self.node.clear();
        self.workers.shutdown(&self.shared.refs);
        self.ipc.clear(&self.shared.refs);
        self.ipc.channel.close();
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
        let Self { engine, shared, rejections, reactor, events, scratch, tasks, poster, polls, polled, ipc, workers, .. } = self;
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
                ipc: &**ipc,
                workers: &**workers,
                pending: &poster.pending,
                polls,
                polled,
            };
            f(&mut turn, reactor, events, hooks)
        }))
    }
}
/// Describes the pending exception without the builtins' inspector, for errors raised before or
/// outside it.
///
/// # Arguments
/// * `scope` - the context handle; its pending exception is taken.
///
/// # Returns
/// `Name: message` for an error object, the text of a thrown string, or a
/// placeholder.
pub(crate) fn exception_text<'s, V: Vm>(scope: &V::Scope<'s>) -> String {
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
