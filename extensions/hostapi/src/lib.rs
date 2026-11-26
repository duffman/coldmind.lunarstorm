//! HostApi: the extension through which a host gives its programs the host's own
//! API.
//!
//! HostApi opens one plugin host, `area`. Each API area a host offers is a plugin
//! there, an [`Area`], and the host's manifest selects the active ones:
//!
//! | module | exports |
//! |---|---|
//! | `lunarstorm:hostapi` | `areas`: the names of the active areas, in manifest order |
//! | `lunarstorm:hostapi/<area>` | the API of one active area |
//!
//! A host installs it with its areas as a tuple, e.g.
//! `type Extensions<V: Vm> = (HostApi<V, (Ui,)>,);`.
//!
//! Decisions:
//! - An area is a plugin with the extension's own shape: a name and a
//!   [`Bindings`] set per Runtime. HostApi only selects, orders and fences them.
//! - An area serves only `lunarstorm:hostapi/<area>` and below; HostApi refuses
//!   anything else, so one area cannot shadow another.
//! - The plugin host is `Many` and `Optional`: a host may expose any number of
//!   areas, and a program with none still gets `areas`, empty.
//! - `lunarstorm:hostapi` is a native module: `areas` is a frozen array built in
//!   Rust at the first import. It replaces the source module `js/hostapi.js`
//!   (archived), which froze a copy of `ops.hostapi_areas`; that `ops` value is gone.

use std::marker::PhantomData;

use lunarstorm_event_loop_runtime::vm::{Field, Item, ModuleDecl, Scope, Ty, Vm};
use lunarstorm_event_loop_runtime::{Bindings, Cardinality, Extension, PluginHost, Requirement, Use};

/// The extension's name, and the first segment of its module specifiers.
pub const NAME: &str = "hostapi";

/// The name of HostApi's plugin host.
pub const PLUGIN_HOST: &str = "area";

/// The specifier of HostApi's own module.
const MODULE: &str = "lunarstorm:hostapi";

/// # Returns
/// The declaration of `lunarstorm:hostapi`.
fn module_decl() -> ModuleDecl {
    ModuleDecl {
        specifier: MODULE,
        items: vec![Item::Value(Field { name: "areas", ty: Ty::Array(&Ty::String), about: "The names of the active areas, in manifest order; frozen." })],
        about: "Which of the host's API areas this program may use; each is `lunarstorm:hostapi/<area>`.",
    }
}

/// One API area of a host: a plugin of HostApi's plugin host `area`.
pub trait Area<V: Vm>: 'static {
    /// The area's name, `[a-z][a-z0-9]{0,31}`. Its modules are
    /// `lunarstorm:hostapi/<NAME>` and below.
    const NAME: &'static str;

    /// Builds what the area installs in one Runtime. Called when the manifest
    /// lists the area.
    ///
    /// # Arguments
    /// * `using` - HostApi's manifest entry: the active areas and HostApi's
    ///   configuration.
    ///
    /// # Returns
    /// The area's natives and modules.
    ///
    /// # Errors
    /// A message; the Runtime fails to start with
    /// `extension hostapi: area <NAME>: <message>`.
    fn bindings(&self, using: &Use<'_>) -> Result<Bindings<V>, String>;

    /// The functions the area's module exports, for [`HostApi::spec`].
    ///
    /// Decision: a `const` next to the area's bindings rather than a separate
    /// description file, so the API is described once, where it is defined; an
    /// area that lists nothing still appears in the spec, with no functions.
    const FUNCTIONS: &'static [Function] = &[];
}

/// One argument of an API function, as [`HostApi::spec`] lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arg {
    /// The argument's name.
    pub name: &'static str,
    /// It may be left out.
    pub optional: bool,
    /// It may repeat (a rest argument).
    pub multiple: bool,
}

/// One function an area's module exports, as [`HostApi::spec`] lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function {
    /// The export's name.
    pub name: &'static str,
    /// One line on what it does.
    pub summary: &'static str,
    /// Its arguments, in order.
    pub args: &'static [Arg],
}

/// A host's areas as one static type: `()`, `(A,)`, `(A, B)`, ... up to 8.
pub trait AreaSet<V: Vm>: 'static {
    /// # Returns
    /// Every member's name, in tuple order.
    fn names(&self) -> Vec<&'static str>;

    /// Builds one member's bindings.
    ///
    /// # Arguments
    /// * `name` - the area's name.
    /// * `using` - HostApi's manifest entry.
    ///
    /// # Returns
    /// The bindings, or `None` when no member has that name.
    fn bindings(&self, name: &str, using: &Use<'_>) -> Option<Result<Bindings<V>, String>>;

    /// # Returns
    /// Every member's name and functions, in tuple order.
    fn functions(&self) -> Vec<(&'static str, &'static [Function])>;
}

impl<V: Vm> AreaSet<V> for () {
    /// # Returns
    /// No names.
    fn names(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// # Returns
    /// `None`: there are no members.
    fn bindings(&self, _name: &str, _using: &Use<'_>) -> Option<Result<Bindings<V>, String>> {
        None
    }

    /// # Returns
    /// Nothing: there are no members.
    fn functions(&self) -> Vec<(&'static str, &'static [Function])> {
        Vec::new()
    }
}

/// Implements [`AreaSet`] for a tuple of areas.
macro_rules! area_set {
    ($($member:ident $index:tt),+) => {
        impl<V: Vm, $($member: Area<V>),+> AreaSet<V> for ($($member,)+) {
            /// # Returns
            /// Every member's name, in tuple order.
            fn names(&self) -> Vec<&'static str> {
                vec![$($member::NAME),+]
            }

            /// Builds the bindings of the member called `name`.
            ///
            /// # Arguments
            /// * `name` - the area's name.
            /// * `using` - HostApi's manifest entry.
            ///
            /// # Returns
            /// The bindings, or `None` when no member has that name.
            fn bindings(&self, name: &str, using: &Use<'_>) -> Option<Result<Bindings<V>, String>> {
                $(
                    if name == $member::NAME {
                        return Some(self.$index.bindings(using));
                    }
                )+
                None
            }

            /// # Returns
            /// Every member's name and functions, in tuple order.
            fn functions(&self) -> Vec<(&'static str, &'static [Function])> {
                vec![$(($member::NAME, $member::FUNCTIONS)),+]
            }
        }
    };
}

area_set!(A 0);
area_set!(A 0, B 1);
area_set!(A 0, B 1, C 2);
area_set!(A 0, B 1, C 2, D 3);
area_set!(A 0, B 1, C 2, D 3, E 4);
area_set!(A 0, B 1, C 2, D 3, E 4, F 5);
area_set!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
area_set!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);

/// The HostApi extension over the areas `A`.
pub struct HostApi<V, A> {
    /// The areas this host offers.
    areas: A,
    /// The engine the areas bind to.
    engine: PhantomData<fn() -> V>,
}

impl<V: Vm, A: AreaSet<V>> HostApi<V, A> {
    /// # Arguments
    /// * `areas` - the areas this host offers, as a tuple.
    ///
    /// # Returns
    /// The extension, for [`lunarstorm_event_loop_runtime::Host::extensions`].
    pub fn new(areas: A) -> Self {
        Self { areas, engine: PhantomData }
    }

    /// The host's whole API as one JSON document: every area this host offers (not
    /// only the ones a manifest activates), in tuple order, with its functions.
    ///
    /// Shape: `{"version": 1, "areas": [{"name", "module", "functions": [{"name",
    /// "summary", "args": [{"name", "optional", "multiple"}]}]}]}`. Tools (the dev
    /// console's API tree, the bundler) read this instead of keeping their own copy.
    ///
    /// # Returns
    /// The spec.
    pub fn spec(&self) -> serde_json::Value {
        use serde_json::json;
        let areas: Vec<_> = self
            .areas
            .functions()
            .into_iter()
            .map(|(name, functions)| {
                let functions: Vec<_> = functions
                    .iter()
                    .map(|f| {
                        let args: Vec<_> = f.args.iter().map(|a| json!({"name": a.name, "optional": a.optional, "multiple": a.multiple})).collect();
                        json!({"name": f.name, "summary": f.summary, "args": args})
                    })
                    .collect();
                json!({"name": name, "module": format!("lunarstorm:{NAME}/{name}"), "functions": functions})
            })
            .collect();
        // Decision: a format version, so a tool can refuse a spec it cannot read.
        json!({"version": 1, "areas": areas})
    }
}

impl<V: Vm, A: AreaSet<V>> Extension<V> for HostApi<V, A> {
    const NAME: &'static str = NAME;

    /// # Returns
    /// The plugin host `area`, offering every member of `A`.
    fn plugin_hosts(&self) -> Vec<PluginHost> {
        vec![PluginHost { name: PLUGIN_HOST, cardinality: Cardinality::Many, requirement: Requirement::Optional, plugins: self.areas.names() }]
    }

    /// Builds `lunarstorm:hostapi` and the bindings of every active area, in
    /// manifest order.
    ///
    /// # Arguments
    /// * `using` - HostApi's manifest entry, already checked against
    ///   [`Extension::plugin_hosts`].
    ///
    /// # Returns
    /// The bindings.
    ///
    /// # Errors
    /// `area <name>: <message>` when an area fails or serves a module outside
    /// `lunarstorm:hostapi/<name>`.
    fn bindings(&self, using: &Use<'_>) -> Result<Bindings<V>, String> {
        let names: Vec<String> = using.plugins().to_vec();
        let mut areas = Bindings::<V>::new();
        for name in &names {
            let built = self.areas.bindings(name, using).ok_or_else(|| format!("no area {name}"))?;
            let built = built.map_err(|e| format!("area {name}: {e}"))?;
            areas.merge(&format!("{NAME}/{name}"), built).map_err(|e| format!("area {name}: {e}"))?;
        }
        let mut bindings = Bindings::<V>::new().native_module(module_decl(), move |scope, exports| {
            let items = names.iter().map(|name| scope.string(name)).collect::<Result<Vec<_>, _>>()?;
            let areas = scope.array(&items)?;
            let object = scope.get(&scope.global(), "Object")?;
            let freeze = scope.get(&object, "freeze")?;
            scope.call(&freeze, std::slice::from_ref(&areas))?;
            exports.push(areas);
            Ok(())
        });
        // The areas, already fenced to their own paths, go after HostApi's module;
        // the merge refuses an `ops` name two areas both bind.
        bindings.merge(NAME, areas)?;
        Ok(bindings)
    }
}
