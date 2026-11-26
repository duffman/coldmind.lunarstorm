//! Host-supplied bindings: native functions, values and modules a host registers
//! before `load`.
//!
//! The Runtime owns only the generic bindings (timers, fs, net, http, console).
//! Anything specific to a kind of host (a terminal for the CLI host, windows and
//! menus for a desktop host) is registered by that host through this type.
//!
//! Decisions:
//! - Registered functions and values are added to the same internal `ops` object
//!   the Runtime's own natives live on. Scripts do not see that object; a host
//!   exposes its natives by also registering a module whose source wraps them
//!   (`import { ops } from 'lunarstorm:internal'`; only host modules may).
//! - A binding is written against the VM contract, generic over [`Vm`], so a host
//!   works with any engine.
//! - The boxed installers run once, at `start`. Calls into a registered function
//!   afterwards go through the argument check ([`declared`]) to the closure.
//! - Functions, classes and native modules require a declaration
//!   ([`lunarstorm_vm::types`]): it checks every call's arguments and lists the
//!   binding in [`crate::Runtime::declarations`]. Only [`Bindings::module`]
//!   (source text, for third-party extensions) and [`Bindings::values`] stay
//!   undeclared.

use crate::modules::INTERNAL_MODULE;
use lunarstorm_vm::{Call, ClassDecl, ClassSpec, ErrorKind, FnDecl, Item, ModuleDecl, Outcome, Scope, Sig, Vm};

/// Adds one binding to the `ops` object: `(scope, ops)`.
type Installer<V> = Box<dyn for<'s> FnOnce(&<V as Vm>::Scope<'s>, &<V as Vm>::Value<'s>) -> Outcome<()>>;

/// A native function, boxed so members of different closure types fit one list.
///
/// Decision: boxing costs one indirect call per method call; the contract itself
/// stays free of `dyn` ([`lunarstorm_vm::ClassSpec`]), only this host-facing
/// convenience pays it.
pub type Native<V> = Box<dyn for<'a> Fn(&Call<'a, V>) -> Outcome<<V as Vm>::Value<'a>>>;

/// Builds a native module's export values: pushes one per declared item, in
/// order, onto the list it receives. Runs at the module's first import.
///
/// Decision: an out-parameter rather than a returned `Vec`, because a
/// higher-ranked closure cannot return a type naming `'s` only through an
/// associated type (`V::Value<'s>`); see [`Call`].
pub type Exports<V> = Box<dyn for<'s> Fn(&<V as Vm>::Scope<'s>, &mut Vec<<V as Vm>::Value<'s>>) -> Outcome<()>>;

/// A class constructor: runs on `new` and returns the instance's data.
pub type Constructor<V, T> = Box<dyn for<'a> Fn(&Call<'a, V>) -> Outcome<T>>;

/// One accessor's `(getter, setter)`; a missing half is `None`.
pub type AccessorImpl<V> = (Option<Native<V>>, Option<Native<V>>);

/// The host modules by specifier, as the loader receives them.
pub(crate) type ModuleTable<V> = Vec<(&'static str, HostModule<V>)>;

/// The implementation of a [`ClassDecl`]; each list is in the declaration's order.
pub struct ClassImpl<V: Vm, T> {
    /// Runs on `new` (after the arguments are checked) and returns the instance's data.
    pub constructor: Constructor<V, T>,
    /// One per [`ClassDecl::methods`].
    pub methods: Vec<Native<V>>,
    /// `(getter, setter)` per [`ClassDecl::accessors`]; a missing half is `None`.
    pub accessors: Vec<AccessorImpl<V>>,
    /// One per [`ClassDecl::statics`].
    pub statics: Vec<Native<V>>,
}

/// A module a host registered.
pub(crate) enum HostModule<V: Vm> {
    /// Source text, compiled on first import.
    Source(&'static str),
    /// A native module: its declaration names the exports [`Exports`] builds.
    Native(ModuleDecl, Exports<V>),
}

/// A set of bindings for [`crate::Runtime::register`].
pub struct Bindings<V: Vm> {
    /// Functions and values to add to `ops`, in registration order.
    installers: Vec<Installer<V>>,
    /// Host modules by specifier.
    modules: ModuleTable<V>,
    /// Names of the functions on `ops`, for [`Bindings::merge`]'s collision check.
    functions: Vec<&'static str>,
    /// Declarations of the functions and classes on `ops`. Kept when `start`
    /// takes the installers, so [`crate::Runtime::declarations`] still reads them.
    declared: Vec<Item>,
    /// Declarations of the native modules, kept likewise.
    native: Vec<ModuleDecl>,
}

impl<V: Vm> Default for Bindings<V> {
    /// # Returns
    /// An empty set.
    fn default() -> Self {
        Self { installers: Vec::new(), modules: Vec::new(), functions: Vec::new(), declared: Vec::new(), native: Vec::new() }
    }
}

/// Creates a native function whose arguments are checked against `sig` first.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `name` - the function's `name`, and the `<fn>` of its error messages.
/// * `sig` - the declaration the arguments are checked against.
/// * `f` - the implementation; it runs only when every argument fits.
///
/// # Returns
/// The function.
///
/// # Errors
/// [`lunarstorm_vm::Thrown`] when allocation fails. The function, when called with
/// an argument that does not fit, throws `TypeError: <fn>: <param> must be <type>`.
pub fn declared<'s, V: Vm, F>(scope: &V::Scope<'s>, name: &'static str, sig: Sig, f: F) -> Outcome<V::Value<'s>>
where
    F: for<'a> Fn(&Call<'a, V>) -> Outcome<V::Value<'a>> + 'static,
{
    scope.function(name, move |call: &Call<'_, V>| {
        if let Err(message) = sig.check::<V>(call.scope(), call.args()) {
            return Err(call.scope().throw_error(ErrorKind::Type, &format!("{name}: {message}")));
        }
        f(call)
    })
}

/// Builds a native class from its declaration and implementation.
///
/// Decisions: `decl.extends` names the base; it is looked up on `ops` first (a
/// native class registered earlier), then on the global object (`Uint8Array`).
/// A method's or setter's error names the member (`add: n must be number`), the
/// constructor's names the class.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `ops` - the `ops` object, for the base lookup.
/// * `decl` - the declaration.
/// * `imp` - the implementation, matching the declaration's lists.
///
/// # Returns
/// The constructor.
///
/// # Errors
/// [`lunarstorm_vm::Thrown`]: an `Error` when the implementation's lists do not
/// match the declaration, or what [`Scope::class`] throws.
fn class<'s, V: Vm, T: 'static>(scope: &V::Scope<'s>, ops: &V::Value<'s>, decl: ClassDecl, imp: ClassImpl<V, T>) -> Outcome<V::Value<'s>> {
    let ClassImpl { constructor, methods, accessors, statics } = imp;
    if methods.len() != decl.methods.len() || accessors.len() != decl.accessors.len() || statics.len() != decl.statics.len() {
        return Err(scope.throw_error(ErrorKind::Error, &format!("class {}: implementation does not match its declaration", decl.name)));
    }
    let mut spec = ClassSpec::new(decl.name);
    if let Some(base) = decl.extends {
        let found = scope.get(ops, base)?;
        let found = if scope.kind(&found) == lunarstorm_vm::Kind::Undefined { scope.get(&scope.global(), base)? } else { found };
        spec = spec.extends(found);
    }
    for (method, f) in decl.methods.iter().zip(methods) {
        spec = spec.method(method.name, declared(scope, method.name, method.sig, f)?);
    }
    for (field, (get, set)) in decl.accessors.iter().zip(accessors) {
        let get = get.map(|get| scope.function(field.name, get)).transpose()?;
        let set = match set {
            Some(set) => {
                let (ty, label) = (field.ty, field.name);
                Some(scope.function(field.name, move |call: &Call<'_, V>| {
                    let scope = call.scope();
                    let value = call.args().first().cloned().unwrap_or_else(|| scope.undefined());
                    if let Err(ty) = ty.check::<V>(scope, &value) {
                        return Err(scope.throw_error(ErrorKind::Type, &format!("{label}: value must be {ty}")));
                    }
                    set(call)
                })?)
            }
            None => None,
        };
        spec = spec.accessor(field.name, get, set);
    }
    for (method, f) in decl.statics.iter().zip(statics) {
        spec = spec.static_member(method.name, declared(scope, method.name, method.sig, f)?);
    }
    let (sig, name) = (decl.constructor, decl.name);
    scope.class(spec, move |call: &Call<'_, V>| match sig.check::<V>(call.scope(), call.args()) {
        Ok(()) => constructor(call),
        Err(message) => Err(call.scope().throw_error(ErrorKind::Type, &format!("{name}: {message}"))),
    })
}

impl<V: Vm> Bindings<V> {
    /// # Returns
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a declared native function as `ops.<name>`.
    ///
    /// # Arguments
    /// * `name` - property name on `ops`; must not collide with a Runtime native.
    /// * `sig` - its declaration; every call's arguments are checked against it.
    /// * `f` - the implementation, in the VM's native function signature.
    ///
    /// # Returns
    /// The set, for chaining.
    pub fn function<F>(mut self, name: &'static str, sig: Sig, f: F) -> Self
    where
        F: for<'a> Fn(&Call<'a, V>) -> Outcome<V::Value<'a>> + 'static,
    {
        self.functions.push(name);
        self.declared.push(Item::Function(FnDecl { name, sig }));
        self.installers.push(Box::new(move |scope, ops| {
            let function = declared(scope, name, sig, f)?;
            scope.set(ops, name, function)
        }));
        self
    }

    /// Registers a declared native class as `ops.<decl.name>`, built on
    /// [`Scope::class`]: subclassable from JS, instances carry `T`.
    ///
    /// # Arguments
    /// * `decl` - the declaration; constructor, method and setter arguments are
    ///   checked against it.
    /// * `imp` - the implementation, its lists in the declaration's order.
    ///
    /// # Returns
    /// The set, for chaining. `start` fails when `imp` does not match `decl`.
    pub fn class<T: 'static>(mut self, decl: ClassDecl, imp: ClassImpl<V, T>) -> Self {
        self.functions.push(decl.name);
        self.declared.push(Item::Class(decl));
        self.installers.push(Box::new(move |scope, ops| {
            let constructor = class(scope, ops, decl, imp)?;
            scope.set(ops, decl.name, constructor)
        }));
        self
    }

    /// Registers a native ES module: its exports are values built in Rust, named
    /// by the declaration.
    ///
    /// # Arguments
    /// * `decl` - the declaration; `decl.specifier` is what scripts import.
    /// * `exports` - pushes one value per `decl.items`, in order, at first import;
    ///   functions among them are usually made with [`declared`].
    ///
    /// # Returns
    /// The set, for chaining.
    pub fn native_module<F>(mut self, decl: ModuleDecl, exports: F) -> Self
    where
        F: for<'s> Fn(&V::Scope<'s>, &mut Vec<V::Value<'s>>) -> Outcome<()> + 'static,
    {
        self.native.push(decl.clone());
        self.modules.push((decl.specifier, HostModule::Native(decl, Box::new(exports))));
        self
    }

    /// Registers code that adds values to `ops` itself, for constants that are
    /// cheaper as properties than as calls.
    ///
    /// # Arguments
    /// * `install` - receives the scope and the `ops` object once, at `start`.
    ///
    /// # Returns
    /// The set, for chaining.
    pub fn values<F>(mut self, install: F) -> Self
    where
        F: for<'s> FnOnce(&V::Scope<'s>, &V::Value<'s>) -> Outcome<()> + 'static,
    {
        self.installers.push(Box::new(install));
        self
    }

    /// Registers an ES module scripts can import by an exact specifier.
    ///
    /// Decision: undeclared, for third-party extensions that ship JS; in-tree
    /// bindings declare their modules ([`Bindings::native_module`]).
    ///
    /// # Arguments
    /// * `specifier` - what scripts write in `import`, e.g. `lunarstorm:ui`.
    /// * `source` - the module's source; compiled on first import only.
    ///
    /// # Returns
    /// The set, for chaining.
    pub fn module(mut self, specifier: &'static str, source: &'static str) -> Self {
        self.modules.push((specifier, HostModule::<V>::Source(source)));
        self
    }

    /// Adds another set after this one.
    ///
    /// # Arguments
    /// * `other` - the bindings to append.
    pub(crate) fn extend(&mut self, other: Self) {
        self.installers.extend(other.installers);
        self.modules.extend(other.modules);
        self.functions.extend(other.functions);
        self.declared.extend(other.declared);
        self.native.extend(other.native);
    }

    /// Adds an extension's or a plugin's set after this one, refusing what would
    /// take another binding's place.
    ///
    /// Decision: an extension serves only `lunarstorm:<extension>` and
    /// `lunarstorm:<extension>/...`, and neither a module nor an `ops` function may
    /// replace one already bound (by the host or an earlier extension). A silent
    /// replacement would change another component's behaviour depending on
    /// installation order.
    ///
    /// # Arguments
    /// * `owner` - the namespace the modules must lie in, after `lunarstorm:`: an
    ///   extension's name (`probe`), or a plugin's path (`hostapi/ui`).
    /// * `other` - the bindings to add.
    ///
    /// # Errors
    /// `module <specifier> is outside lunarstorm:<owner>`, or
    /// `<module or ops.function> is already bound`; the caller adds whose.
    pub fn merge(&mut self, owner: &str, other: Self) -> Result<(), String> {
        let namespace = format!("lunarstorm:{owner}");
        for (specifier, _) in &other.modules {
            let inside = specifier.strip_prefix(namespace.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
            if !inside {
                return Err(format!("module {specifier} is outside {namespace}"));
            }
            if self.modules.iter().any(|(bound, _)| bound == specifier) {
                return Err(format!("module {specifier} is already bound"));
            }
        }
        for name in &other.functions {
            if self.functions.contains(name) {
                return Err(format!("ops.{name} is already bound"));
            }
        }
        self.extend(other);
        Ok(())
    }

    /// Splits the set for `start`.
    ///
    /// # Returns
    /// The installers to run against `ops`, and the module table for the loader.
    pub(crate) fn take(&mut self) -> (Vec<Installer<V>>, ModuleTable<V>) {
        (std::mem::take(&mut self.installers), std::mem::take(&mut self.modules))
    }

    /// # Returns
    /// The set's declarations: what it adds to `ops` as `lunarstorm:internal`
    /// (when anything), then its native modules.
    pub(crate) fn declarations(&self) -> Vec<ModuleDecl> {
        let internal = (!self.declared.is_empty())
            .then(|| ModuleDecl { specifier: INTERNAL_MODULE, items: self.declared.clone(), about: "Host natives on `ops`." });
        internal.into_iter().chain(self.native.iter().cloned()).collect()
    }
}
