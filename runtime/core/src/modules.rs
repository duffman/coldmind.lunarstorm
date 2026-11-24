//! Module resolution and loading: the Runtime's implementation of the VM callbacks.
//!
//! Three kinds of specifier are supported:
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
//! - **files** (`./x`, `../x`, `/x`): resolved against the importing module, trying
//!   the exact path, then `.js`, `.mjs` and `/index.js`, and read under the host's
//!   policy.
//!
//! Decisions:
//! - Bare package specifiers (`import x from "left-pad"`) are rejected with a clear
//!   error; there is no `node_modules` lookup.
//! - The callbacks receive a context handle and hold only the shared loop state,
//!   never the Runtime, so the engine cannot re-enter the Runtime through them.
//! - Loading answers [`Load::Ready`] always: module files are read synchronously,
//!   which every engine supports. [`Load::Later`] is left to a future loader for
//!   engines that report `async_module_load`.

use crate::host::{Access, Host};
use crate::io::Shared;
use crate::ipc::{Ipc, MODULE as IPC_MODULE};
use crate::worker::{Workers, MODULE as WORKER_MODULE, NATIVE_MODULE as WORKER_NATIVE, SOURCE as WORKER_SOURCE};
use lunarstorm_vm::{Callbacks, Load, ModuleMeta, ModuleSource, Scope, Vm};
use std::cell::RefCell;
use std::path::Path;
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

/// Rejected promises that have no handler yet: the promise's identity and its reason.
pub type Rejections<V> = Rc<RefCell<Vec<(usize, <V as Vm>::Persistent)>>>;

/// The Runtime's VM callbacks: module resolve/load and rejection tracking.
pub struct Loader<V: Vm, H: Host> {
    /// Loop state; gives access to the host's fs policy.
    shared: Rc<Shared<H>>,
    /// Where unhandled rejections are recorded for the loop to report.
    rejections: Rejections<V>,
    /// `(specifier, source)` of the modules the host registered.
    modules: Vec<(&'static str, &'static str)>,
    /// The `lunarstorm:ipc` channel, for the module's exports.
    ipc: Rc<Ipc<V>>,
    /// The `lunarstorm:worker` state, for the module's natives.
    workers: Rc<Workers<V>>,
    /// The builtin modules and internals the glue returned.
    registry: Rc<Registry<V>>,
}

impl<V: Vm, H: Host> Loader<V, H> {
    /// # Arguments
    /// * `shared` - loop state and host.
    /// * `rejections` - list the loop drains after each JS entry.
    /// * `modules` - host modules from the registered bindings.
    /// * `ipc` - the Runtime's `lunarstorm:ipc` state.
    /// * `workers` - the Runtime's `lunarstorm:worker` state.
    /// * `registry` - filled by `start` with what the glue returns.
    ///
    /// # Returns
    /// The callbacks to hand to the VM instance.
    pub fn new(
        shared: Rc<Shared<H>>,
        rejections: Rejections<V>,
        modules: Vec<(&'static str, &'static str)>,
        ipc: Rc<Ipc<V>>,
        workers: Rc<Workers<V>>,
        registry: Rc<Registry<V>>,
    ) -> Self {
        Self { shared, rejections, modules, ipc, workers, registry }
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
    /// canonical absolute path of a file.
    ///
    /// # Errors
    /// A message when the file does not exist or the specifier is a bare package name.
    fn resolve<'s>(&self, _scope: &V::Scope<'s>, base: &str, name: &str) -> Result<String, String> {
        if self.modules.iter().any(|(specifier, _)| *specifier == name) {
            return Ok(name.to_owned());
        }
        // Only a host that gave its program a channel serves `lunarstorm:ipc`.
        if H::IPC && name == IPC_MODULE {
            return Ok(name.to_owned());
        }
        // `lunarstorm:worker` is JS over natives that only it may import.
        if H::WORKERS && (name == WORKER_MODULE || (name == WORKER_NATIVE && base == WORKER_MODULE)) {
            return Ok(name.to_owned());
        }
        // Only host modules (their canonical name is their specifier, never a path)
        // reach the natives; a program's own files are refused.
        if name == INTERNAL_MODULE && self.modules.iter().any(|(specifier, _)| *specifier == base) {
            return Ok(name.to_owned());
        }
        let bare = name.strip_prefix("node:").unwrap_or(name);
        if BUILTINS.contains(&bare) {
            return Ok(format!("node:{bare}"));
        }
        if !(name.starts_with("./") || name.starts_with("../") || name.starts_with('/')) {
            return Err(String::from("not a builtin module, and package imports are not supported"));
        }
        let joined = Path::new(base).parent().unwrap_or(Path::new("/")).join(name);
        let joined = joined.to_string_lossy();
        ["", ".js", ".mjs", "/index.js"]
            .iter()
            .map(|suffix| format!("{joined}{suffix}"))
            .find(|candidate| Path::new(candidate).is_file())
            .and_then(|found| std::fs::canonicalize(found).ok())
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| String::from("module not found"))
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
        if let Some((_, source)) = self.modules.iter().find(|(specifier, _)| *specifier == name) {
            return Ok(Load::Ready(ModuleSource { source: source.as_bytes().to_vec(), meta: None }));
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
            return Ok(Load::Ready(ModuleSource { source: WORKER_SOURCE.as_bytes().to_vec(), meta: None }));
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
        if !self.shared.host.allow(Access::Read(name)) {
            return Err(String::from("permission denied by host policy"));
        }
        let source = std::fs::read(name).map_err(|e| e.to_string())?;
        Ok(Load::Ready(ModuleSource { source, meta: Some(ModuleMeta { filename: name.to_owned(), main: false }) }))
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
