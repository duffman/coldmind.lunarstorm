//! Module resolution and loading: the Runtime's implementation of the VM callbacks.
//!
//! Three kinds of specifier are supported:
//! - **host modules** (`lunarstorm:ui`): ES modules a host registered through
//!   [`crate::Bindings::module`], matched by exact specifier and compiled only when
//!   a script imports them.
//! - **builtins** (`node:fs`, or bare `fs`): thin ES modules generated on first
//!   import that re-export an object the bootstrap script placed on the hidden
//!   `__lunarstorm` global. Generating them from the object's own keys keeps the
//!   export list in one place (the JS glue) with nothing to keep in sync here.
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
use lunarstorm_vm::{Callbacks, Load, ModuleMeta, ModuleSource, Scope, Vm};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// Builtin module names, without the `node:` prefix.
const BUILTINS: [&str; 9] =
    ["events", "net", "http", "fs", "fs/promises", "path", "process", "buffer", "timers"];

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
}

impl<V: Vm, H: Host> Loader<V, H> {
    /// # Arguments
    /// * `shared` - loop state and host.
    /// * `rejections` - list the loop drains after each JS entry.
    /// * `modules` - host modules from the registered bindings.
    ///
    /// # Returns
    /// The callbacks to hand to the VM instance.
    pub fn new(shared: Rc<Shared<H>>, rejections: Rejections<V>, modules: Vec<(&'static str, &'static str)>) -> Self {
        Self { shared, rejections, modules }
    }

    /// Generates the source of a builtin module from the glue's export object.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `builtin` - builtin name without the `node:` prefix.
    ///
    /// # Returns
    /// The module source, or `None` when the glue is not installed.
    fn builtin_source<'s>(scope: &V::Scope<'s>, builtin: &str) -> Option<String> {
        let registry = scope.get(&scope.global(), "__lunarstorm").ok()?;
        let internal = scope.get(&registry, "internal").ok()?;
        let exports_of = scope.get(&internal, "exportsOf").ok()?;
        let names = scope.call(&exports_of, &[scope.string(builtin).ok()?]).ok()?;
        let names = scope.with_str(&names, str::to_owned).ok()?;
        Some(format!("const m = __lunarstorm[{builtin:?}];\nexport default m;\nexport const {{ {names} }} = m;\n"))
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
    /// * `scope` - the context handle, used to read a builtin's export names.
    /// * `name` - name produced by [`Callbacks::resolve`].
    ///
    /// # Returns
    /// The module's source, ready now.
    ///
    /// # Errors
    /// A message when the host's policy refuses the file or it cannot be read.
    fn load<'s>(&self, scope: &V::Scope<'s>, name: &str) -> Result<Load, String> {
        if let Some((_, source)) = self.modules.iter().find(|(specifier, _)| *specifier == name) {
            return Ok(Load::Ready(ModuleSource { source: source.as_bytes().to_vec(), meta: None }));
        }
        if let Some(builtin) = name.strip_prefix("node:") {
            let source = Self::builtin_source(scope, builtin).ok_or_else(|| String::from("builtin is unavailable"))?;
            return Ok(Load::Ready(ModuleSource { source: source.into_bytes(), meta: None }));
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
