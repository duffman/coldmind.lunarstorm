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
//!   afterwards go straight from the engine to the closure.

use lunarstorm_vm::{Call, Outcome, Scope, Vm};

/// Adds one binding to the `ops` object: `(scope, ops)`.
type Installer<V> = Box<dyn for<'s> FnOnce(&<V as Vm>::Scope<'s>, &<V as Vm>::Value<'s>) -> Outcome<()>>;

/// A set of bindings for [`crate::Runtime::register`].
pub struct Bindings<V: Vm> {
    /// Functions and values to add to `ops`, in registration order.
    installers: Vec<Installer<V>>,
    /// `(specifier, source)` of host modules.
    modules: Vec<(&'static str, &'static str)>,
}

impl<V: Vm> Default for Bindings<V> {
    /// # Returns
    /// An empty set.
    fn default() -> Self {
        Self { installers: Vec::new(), modules: Vec::new() }
    }
}

impl<V: Vm> Bindings<V> {
    /// # Returns
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a native function as `ops.<name>`.
    ///
    /// # Arguments
    /// * `name` - property name on `ops`; must not collide with a Runtime native.
    /// * `f` - the implementation, in the VM's native function signature.
    ///
    /// # Returns
    /// The set, for chaining.
    pub fn function<F>(mut self, name: &'static str, f: F) -> Self
    where
        F: for<'a> Fn(&Call<'a, V>) -> Outcome<V::Value<'a>> + 'static,
    {
        self.installers.push(Box::new(move |scope, ops| {
            let function = scope.function(name, f)?;
            scope.set(ops, name, function)
        }));
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
    /// # Arguments
    /// * `specifier` - what scripts write in `import`, e.g. `lunarstorm:ui`.
    /// * `source` - the module's source; compiled on first import only.
    ///
    /// # Returns
    /// The set, for chaining.
    pub fn module(mut self, specifier: &'static str, source: &'static str) -> Self {
        self.modules.push((specifier, source));
        self
    }

    /// Adds another set after this one.
    ///
    /// # Arguments
    /// * `other` - the bindings to append.
    pub(crate) fn extend(&mut self, other: Self) {
        self.installers.extend(other.installers);
        self.modules.extend(other.modules);
    }

    /// Splits the set for `start`.
    ///
    /// # Returns
    /// The installers to run against `ops`, and the module table for the loader.
    pub(crate) fn take(&mut self) -> (Vec<Installer<V>>, Vec<(&'static str, &'static str)>) {
        (std::mem::take(&mut self.installers), std::mem::take(&mut self.modules))
    }
}
