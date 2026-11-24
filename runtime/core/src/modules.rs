//! Module resolution and loading: the Runtime's implementation of the VM callbacks.
//!
//! Four kinds of specifier are supported:
//! - **host modules** (`lunarstorm:ui`): ES modules a host registered through
//!   [`crate::Bindings::module`], matched by exact specifier and compiled only when
//!   a script imports them.
//! - **builtins** (`node:fs`, or bare `fs`): thin ES modules generated on first
//!   import that re-export an object the bootstrap script placed on the hidden
//!   `__lunarstorm` global. Generating them from the object's own keys keeps the
//!   export list in one place (the JS glue) with nothing to keep in sync here.
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

use crate::host::{Access, Host};
use crate::io::Shared;
use lunarstorm_modules::{FACADE, Modules, REQUIRE};
use lunarstorm_vm::{Callbacks, Load, ModuleMeta, ModuleSource, Scope, Vm};
use lunarstorm_jbn::Package;
use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

/// Builtin module names, without the `node:` prefix.
const BUILTINS: [&str; 9] =
    ["events", "net", "http", "fs", "fs/promises", "path", "process", "buffer", "timers"];

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
    /// `(specifier, source)` of the modules the host registered.
    modules: Vec<(&'static str, &'static str)>,
    /// Files and packages.
    sources: Rc<Modules>,
    /// The `.jbn` package the Runtime runs, once loaded.
    package: Rc<OnceCell<Package>>,
    /// The host type: it fixes which Runtime these callbacks belong to.
    host: std::marker::PhantomData<H>,
}

impl<V: Vm, H: Host> Loader<V, H> {
    /// # Arguments
    /// * `rejections` - list the loop drains after each JS entry.
    /// * `modules` - host modules from the registered bindings.
    /// * `sources` - the Runtime's source modules, see [`sources`].
    /// * `package` - the package the Runtime runs, set when one is loaded.
    ///
    /// # Returns
    /// The callbacks to hand to the VM instance.
    pub fn new(
        rejections: Rejections<V>,
        modules: Vec<(&'static str, &'static str)>,
        sources: Rc<Modules>,
        package: Rc<OnceCell<Package>>,
    ) -> Self {
        Self { rejections, modules, sources, package, host: std::marker::PhantomData }
    }

    /// A host module or builtin a `require()` may name.
    ///
    /// # Returns
    /// Its module name, or `None`.
    fn native(&self, specifier: &str) -> Option<String> {
        native(self.modules.iter().map(|(name, _)| *name), specifier)
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
        resolve(&|s| self.native(s), &self.sources, base, name)
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
            return Ok(Load::Ready(ModuleSource { source: source.as_bytes().to_vec(), meta: None, bytecode: false, source_map: None }));
        }
        if let Some(builtin) = name.strip_prefix("node:") {
            let source = Self::builtin_source(scope, builtin).ok_or_else(|| String::from("builtin is unavailable"))?;
            return Ok(Load::Ready(ModuleSource { source: source.into_bytes(), meta: None, bytecode: false, source_map: None }));
        }
        let filename = name.strip_suffix(FACADE).unwrap_or(name).to_owned();
        if let Some(bytecode) = self.package.get().and_then(|package| package.bytecode(name)) {
            let meta = Some(ModuleMeta { filename, main: false });
            return Ok(Load::Ready(ModuleSource { source: bytecode.to_vec(), meta, bytecode: true, source_map: None }));
        }
        let source = self.sources.load(name, &|specifier| self.native(specifier)).map_err(|e| e.to_string())?;
        let source_map = self.sources.source_map(name);
        Ok(Load::Ready(ModuleSource { source, meta: Some(ModuleMeta { filename, main: false }), bytecode: false, source_map }))
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
