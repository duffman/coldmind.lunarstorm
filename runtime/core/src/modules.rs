//! Module resolution and loading: the Runtime's implementation of the VM callbacks.
//!
//! Four kinds of specifier are supported:
//! - **host modules** (`lunarstorm:ui`): ES modules a host registered through
//!   [`crate::Bindings::module`], matched by exact specifier and compiled only when
//!   a script imports them.
//! - **builtins** (`node:fs`, or bare `fs`): native modules (`Scope::native_module`,
//!   like `lunarstorm:ipc`) declared on first import from an object the bootstrap
//!   script returned in its registry: `default` is the object, every own key a
//!   named export. The export list stays in the JS glue with nothing to keep in
//!   sync here, and nothing is published on the global object.
//! - **`lunarstorm:internal`**: a native module exporting the Runtime's natives
//!   (`ops`) and the glue's `live` slot to host modules. Only a host module may
//!   import it; a program's own files cannot.
//! - **files** (`./x`, `../x`, `/x`) and **packages** (`left-pad`, `@scope/x/sub`,
//!   `#private`): resolved and turned into ES modules by `lunarstorm-modules`:
//!   TypeScript, JSON and CommonJS load as ES modules, packages are found in
//!   `node_modules` and mapped through `package.json` `exports` and `imports`.
//!
//! Decisions:
//! - Every file read, `package.json` included, goes through the host's policy.
//! - Builtins and host modules win over packages of the same name, as in Node.
//! - The callbacks receive a context handle and hold only the shared loop state,
//!   never the Runtime, so the engine cannot re-enter the Runtime through them.
//! - **package modules**: once a verified `.jbn` package is loaded (see
//!   [`crate::Source::Package`]), a module of the package resolves its imports
//!   through the package's import table and loads as bytecode. Nothing of a
//!   package is read from disk.
//! - Loading answers [`Load::Ready`] always: module files are read synchronously,
//!   which every engine supports. [`Load::Later`] is left to a future loader for
//!   engines that report `async_module_load`.

use crate::bindings::{HostModule, ModuleTable};
use crate::host::{Access, Host};
use crate::io::Shared;
use crate::ipc::{Ipc, MODULE as IPC_MODULE};
use crate::worker::{Workers, MODULE as WORKER_MODULE, NATIVE_MODULE as WORKER_NATIVE, SOURCE as WORKER_SOURCE};
use lunarstorm_modules::{FACADE, Modules, REQUIRE};
use lunarstorm_vm::{Callbacks, Image, Load, ModuleMeta, ModuleSource, Scope, Vm};
use lunarstorm_jbn::Package;
use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

/// Builtin module names, without the `node:` prefix.
const BUILTINS: [&str; 9] =
    ["events", "net", "http", "fs", "fs/promises", "path", "process", "buffer", "timers"];

/// The specifier host modules import to reach the Runtime's natives.
pub const INTERNAL_MODULE: &str = "lunarstorm:internal";

/// What the JS glue returned for the loader: the builtin modules and the internals.
struct Parts<V: Vm> {
    /// Object of the builtin modules by name.
    modules: V::Persistent,
    /// `{ ops, live }`, the exports of `lunarstorm:internal`.
    internal: V::Persistent,
    /// `(name) => 'a,b,c'`, the export names of one builtin.
    exports_of: V::Persistent,
}

/// The glue's registry, shared between the Runtime and its [`Loader`].
///
/// Persistent handles into the heap: [`Registry::clear`] must run before the VM
/// instance is destroyed.
pub struct Registry<V: Vm>(RefCell<Option<Parts<V>>>);

impl<V: Vm> Default for Registry<V> {
    /// # Returns
    /// An empty registry, as it is before `start`.
    fn default() -> Self {
        Self(RefCell::new(None))
    }
}

impl<V: Vm> Registry<V> {
    /// Keeps the registry the glue returned.
    ///
    /// # Arguments
    /// * `modules` - the object of builtins by name.
    /// * `internal` - the `{ ops, live }` object.
    /// * `exports_of` - the function listing a builtin's export names.
    pub fn set(&self, modules: V::Persistent, internal: V::Persistent, exports_of: V::Persistent) {
        *self.0.borrow_mut() = Some(Parts { modules, internal, exports_of });
    }

    /// Drops the handles.
    pub fn clear(&self) {
        self.0.borrow_mut().take();
    }
}

/// The builtin a specifier names (`fs`, `node:fs`), as its module name.
///
/// # Returns
/// `node:<name>`, or `None` when it is not a builtin.
pub fn builtin(specifier: &str) -> Option<String> {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    BUILTINS.contains(&bare).then(|| format!("node:{bare}"))
}

/// A host module or builtin a specifier names.
///
/// # Arguments
/// * `hosted` - specifiers of the host modules.
/// * `specifier` - as written, without the `require()` prefix.
///
/// # Returns
/// Its module name, or `None`.
pub fn native<'a>(mut hosted: impl Iterator<Item = &'a str>, specifier: &str) -> Option<String> {
    if hosted.any(|name| name == specifier) {
        return Some(specifier.to_owned());
    }
    builtin(specifier)
}

/// Resolves a specifier of a source module: host modules and builtins first, then
/// files and packages.
///
/// # Arguments
/// * `native` - see [`native`].
/// * `sources` - the Runtime's source modules.
/// * `base` - canonical name of the importing module.
/// * `name` - the specifier as written.
///
/// # Returns
/// The canonical name of the target.
///
/// # Errors
/// A message when nothing answers to the specifier.
pub fn resolve(native: &dyn Fn(&str) -> Option<String>, sources: &Modules, base: &str, name: &str) -> Result<String, String> {
    if let Some(native) = native(name.strip_prefix(REQUIRE).unwrap_or(name)) {
        return Ok(native);
    }
    sources.resolve(base, name).map_err(|e| e.to_string())
}

/// The source modules of one Runtime: every read is checked against the host's
/// policy.
///
/// # Arguments
/// * `shared` - loop state holding the host.
pub fn sources<H: Host>(shared: &Rc<Shared<H>>) -> Rc<Modules> {
    let shared = shared.clone();
    Rc::new(Modules::new(Box::new(move |path| {
        let name = path.to_string_lossy();
        if !shared.host.allow(Access::Read(&name)) {
            return Err(format!("{name}: permission denied by host policy"));
        }
        std::fs::read(path).map_err(|e| format!("{name}: {e}"))
    })))
}

/// Rejected promises that have no handler yet: the promise's identity and its reason.
pub type Rejections<V> = Rc<RefCell<Vec<(usize, <V as Vm>::Persistent)>>>;

/// The Runtime's VM callbacks: module resolve/load and rejection tracking.
pub struct Loader<V: Vm, H: Host> {
    /// Where unhandled rejections are recorded for the loop to report.
    rejections: Rejections<V>,
    /// The modules the host registered, by specifier.
    modules: ModuleTable<V>,
    /// Loop state; the exports of the channel modules are closures over it.
    shared: Rc<Shared<H>>,
    /// The `lunarstorm:ipc` channel, for the module's exports.
    ipc: Rc<Ipc<V>>,
    /// The `lunarstorm:worker` state, for the module's natives.
    workers: Rc<Workers<V>>,
    /// The builtin modules and internals the glue returned.
    registry: Rc<Registry<V>>,
    /// Files and packages.
    sources: Rc<Modules>,
    /// The `.jbn` package the Runtime runs, once loaded.
    package: Rc<OnceCell<Package>>,
}

impl<V: Vm, H: Host> Loader<V, H> {
    /// # Arguments
    /// * `shared` - loop state and host.
    /// * `rejections` - list the loop drains after each JS entry.
    /// * `modules` - host modules from the registered bindings.
    /// * `ipc` - the Runtime's `lunarstorm:ipc` state.
    /// * `workers` - the Runtime's `lunarstorm:worker` state.
    /// * `registry` - filled by `start` with what the glue returns.
    /// * `sources` - the Runtime's source modules, see [`sources`].
    /// * `package` - the package the Runtime runs, set when one is loaded.
    ///
    /// # Returns
    /// The callbacks to hand to the VM instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        shared: Rc<Shared<H>>,
        rejections: Rejections<V>,
        modules: ModuleTable<V>,
        ipc: Rc<Ipc<V>>,
        workers: Rc<Workers<V>>,
        registry: Rc<Registry<V>>,
        sources: Rc<Modules>,
        package: Rc<OnceCell<Package>>,
    ) -> Self {
        Self { shared, rejections, modules, ipc, workers, registry, sources, package }
    }

    /// A host module, builtin or channel module a specifier names; also what a
    /// `require()` and a package's computed `import()` may reach.
    ///
    /// Decision: `lunarstorm:ipc` and `lunarstorm:worker` answer only for a host
    /// that opted in ([`Host::IPC`], [`Host::WORKERS`]).
    ///
    /// # Returns
    /// Its module name, or `None`.
    fn native(&self, specifier: &str) -> Option<String> {
        if (H::IPC && specifier == IPC_MODULE) || (H::WORKERS && specifier == WORKER_MODULE) {
            return Some(specifier.to_owned());
        }
        native(self.modules.iter().map(|(name, _)| *name), specifier)
    }

    /// Declares the native module of one builtin from the glue's registry.
    ///
    /// Decision: `default` is the builtin's object and each own key a named export
    /// (a key called `default` is skipped), the same shape the generated ES module
    /// had, so importers do not change.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `name` - canonical name, `node:<builtin>`.
    ///
    /// # Errors
    /// A message when the glue is not installed or the builtin has no object.
    fn declare_builtin<'s>(&self, scope: &V::Scope<'s>, name: &str) -> Result<(), String> {
        let unavailable = || String::from("builtin is unavailable");
        let parts = self.registry.0.borrow();
        let parts = parts.as_ref().ok_or_else(unavailable)?;
        let builtin = name.strip_prefix("node:").unwrap_or(name);
        let object = scope.get(&scope.local(&parts.modules), builtin).map_err(|_| unavailable())?;
        let names = scope.call(&scope.local(&parts.exports_of), &[scope.string(builtin).map_err(|_| unavailable())?]).map_err(|_| unavailable())?;
        let names = scope.with_str(&names, str::to_owned).map_err(|_| unavailable())?;
        let mut exports = vec![("default", object.clone())];
        let keys: Vec<&str> = names.split(',').filter(|key| !key.is_empty() && *key != "default").collect();
        for key in &keys {
            exports.push((*key, scope.get(&object, key).map_err(|_| unavailable())?));
        }
        scope.native_module(name, &exports).map_err(|_| {
            let _ = scope.exception();
            format!("cannot declare {name}")
        })
    }

    /// Declares `lunarstorm:internal`: `ops` and `live` from the glue's registry.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    ///
    /// # Errors
    /// A message when the glue is not installed.
    fn declare_internal<'s>(&self, scope: &V::Scope<'s>) -> Result<(), String> {
        let unavailable = || String::from("internals are unavailable");
        let parts = self.registry.0.borrow();
        let parts = parts.as_ref().ok_or_else(unavailable)?;
        let internal = scope.local(&parts.internal);
        let ops = scope.get(&internal, "ops").map_err(|_| unavailable())?;
        let live = scope.get(&internal, "live").map_err(|_| unavailable())?;
        scope.native_module(INTERNAL_MODULE, &[("ops", ops), ("live", live)]).map_err(|_| {
            let _ = scope.exception();
            String::from("cannot declare lunarstorm:internal")
        })
    }
}

impl<V: Vm, H: Host> Callbacks<V> for Loader<V, H> {
    /// Resolves one specifier.
    ///
    /// # Arguments
    /// * `_scope` - unused.
    /// * `base` - canonical name of the importing module (an absolute path for files).
    /// * `name` - the specifier as written.
    ///
    /// # Returns
    /// The specifier itself for a host module, `node:<name>` for a builtin, or the
    /// canonical absolute path of a file (with `#esm` for an imported CommonJS facade).
    ///
    /// # Errors
    /// A message when nothing answers to the specifier.
    fn resolve<'s>(&self, _scope: &V::Scope<'s>, base: &str, name: &str) -> Result<String, String> {
        if let Some(package) = self.package.get() {
            if package.manifest().module(base).is_some() {
                // Decision: a package is a closed world. Its import table answers,
                // then host modules and builtins (a computed `import()` of one);
                // anything else is an error, never a file read.
                if let Some(target) = package.import(base, name) {
                    return Ok(target.to_owned());
                }
                let wanted = name.strip_prefix(REQUIRE).unwrap_or(name);
                return self.native(wanted).ok_or_else(|| format!("{name} is not in the package"));
            }
        }
        // `lunarstorm:worker` is JS over natives that only it may import.
        if H::WORKERS && name == WORKER_NATIVE && base == WORKER_MODULE {
            return Ok(name.to_owned());
        }
        // Only host modules (their canonical name is their specifier, never a path)
        // reach the natives; a program's own files are refused.
        if name == INTERNAL_MODULE && self.modules.iter().any(|(specifier, _)| *specifier == base) {
            return Ok(name.to_owned());
        }
        resolve(&|s| self.native(s), &self.sources, base, name)
    }

    /// Loads one module by its canonical name.
    ///
    /// # Arguments
    /// * `scope` - the context handle, used to declare native modules.
    /// * `name` - name produced by [`Callbacks::resolve`].
    ///
    /// # Returns
    /// The module's source, ready now, or [`Load::Native`] for a module declared
    /// natively (`lunarstorm:ipc`, `lunarstorm:worker`, builtins, internals).
    ///
    /// # Errors
    /// A message when the host's policy refuses the file or it cannot be read.
    fn load<'s>(&self, scope: &V::Scope<'s>, name: &str) -> Result<Load, String> {
        match self.modules.iter().find(|(specifier, _)| *specifier == name) {
            Some((_, HostModule::Source(source))) => {
                return Ok(Load::Ready(ModuleSource { source: source.as_bytes().to_vec(), meta: None, image: None, source_map: None }));
            }
            Some((_, HostModule::Native(decl, exports))) => {
                let failed = |what: &str| {
                    let _ = scope.exception();
                    format!("cannot {what} {name}")
                };
                let mut values = Vec::with_capacity(decl.items.len());
                exports(scope, &mut values).map_err(|_| failed("create the exports of"))?;
                if values.len() != decl.items.len() {
                    return Err(format!("{name}: {} exports declared, {} built", decl.items.len(), values.len()));
                }
                let pairs: Vec<_> = decl.items.iter().map(|item| item.name()).zip(values).collect();
                scope.native_module(name, &pairs).map_err(|_| failed("declare"))?;
                return Ok(Load::Native);
            }
            None => {}
        }
        if H::IPC && name == IPC_MODULE {
            // A native module: its exports are closures over this Runtime's state.
            let exports = self.ipc.exports(scope, &self.shared).map_err(|_| {
                let _ = scope.exception();
                String::from("cannot create the exports of lunarstorm:ipc")
            })?;
            scope.native_module(name, &exports).map_err(|_| {
                let _ = scope.exception();
                String::from("cannot declare lunarstorm:ipc")
            })?;
            return Ok(Load::Native);
        }
        if H::WORKERS && name == WORKER_MODULE {
            return Ok(Load::Ready(ModuleSource { source: WORKER_SOURCE.as_bytes().to_vec(), meta: None, image: None, source_map: None }));
        }
        if H::WORKERS && name == WORKER_NATIVE {
            let exports = self.workers.exports(scope, &self.shared, &self.ipc).map_err(|_| {
                let _ = scope.exception();
                String::from("cannot create the exports of lunarstorm:worker")
            })?;
            scope.native_module(name, &exports).map_err(|_| {
                let _ = scope.exception();
                String::from("cannot declare lunarstorm:worker")
            })?;
            return Ok(Load::Native);
        }
        if name == INTERNAL_MODULE {
            self.declare_internal(scope)?;
            return Ok(Load::Native);
        }
        if name.starts_with("node:") {
            self.declare_builtin(scope, name)?;
            return Ok(Load::Native);
        }
        let filename = name.strip_suffix(FACADE).unwrap_or(name).to_owned();
        if let Some(image) = self.package.get().and_then(|package| Image::from_package(package, name)) {
            let meta = Some(ModuleMeta { filename, main: false });
            return Ok(Load::Ready(ModuleSource { source: Vec::new(), meta, image: Some(image), source_map: None }));
        }
        let source = self.sources.load(name, &|specifier| self.native(specifier)).map_err(|e| e.to_string())?;
        let source_map = self.sources.source_map(name);
        Ok(Load::Ready(ModuleSource { source, meta: Some(ModuleMeta { filename, main: false }), image: None, source_map }))
    }

    /// Tracks unhandled promise rejections.
    ///
    /// The engine reports a rejection with `handled == false` when a promise is
    /// rejected with no handler, and again with `true` if a handler is attached
    /// later. Entries still listed after a full microtask drain are genuinely
    /// unhandled; the loop reports the first one.
    ///
    /// # Arguments
    /// * `scope` - the context handle, used to keep the reason alive.
    /// * `promise` - identity of the promise.
    /// * `reason` - the rejection reason.
    /// * `handled` - whether a handler was attached.
    fn rejection<'s>(&self, scope: &V::Scope<'s>, promise: usize, reason: V::Value<'s>, handled: bool) {
        let mut list = self.rejections.borrow_mut();
        if handled {
            list.retain(|(pending, _)| *pending != promise);
        } else {
            list.push((promise, scope.persist(&reason)));
        }
    }
}
