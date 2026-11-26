//! Node's builtin modules and globals, as natives.
//!
//! One file per module (D4): each has `declare()`, the declaration its exports
//! are installed from, and `install(...)`, which builds the module's object.
//! [`install`] runs them at `start`, after the `ops` object and the host's
//! bindings, and hands the JS glue the result: `{ modules, ...internals }`.
//!
//! Decisions:
//! - During the port the remaining glue (net, http, fs, dispatch, registry)
//!   reaches these modules through the object [`install`] returns: `modules` is
//!   the glue's module registry, already holding the ported builtins, and the
//!   other keys are the helpers the glue used to define itself (`emit2`,
//!   `nextTick`, `asBuffer`, ...). Scripts never see that object.
//! - Values the natives keep between calls (builtins such as `Map.prototype.get`,
//!   the `EventEmitter` class, `Buffer.prototype`, `process`) are persistent
//!   handles in [`Node`], which the Runtime owns and clears before the VM
//!   instance goes. Closures capture the `Rc<Node>`, never a handle of their own:
//!   a handle inside a GC-managed function is a root the collector cannot see
//!   through, so a cycle through it would never be freed.
//! - Builtins are captured once, at install, instead of looked up on the global
//!   object per call: a script that replaces `Object.keys` does not change how
//!   `console.log` prints.

pub(crate) mod buffer;
pub(crate) mod console;
pub(crate) mod events;
pub(crate) mod path;
pub(crate) mod process;
pub(crate) mod timers;

use crate::host::Host;
use crate::io::Shared;
use lunarstorm_vm::{FnDecl, Kind, ModuleDecl, Outcome, Param, Scope, Sig, Ty, Vm};
use std::cell::RefCell;
use std::rc::Rc;

/// The specifier of the declarations of names installed on the global object
/// (`console`, `TextEncoder`, `crypto`); not importable.
pub const GLOBAL: &str = "globalThis";

/// A value [`Node`] keeps for the natives, by index.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Slot {
    /// `String`
    String,
    /// `Number`
    Number,
    /// `parseInt`
    ParseInt,
    /// `parseFloat`
    ParseFloat,
    /// `JSON.stringify`
    Stringify,
    /// `Object.keys`
    Keys,
    /// `Object.defineProperty`
    DefineProperty,
    /// `Object.setPrototypeOf`
    SetPrototypeOf,
    /// `Object.prototype.toString`
    ObjectToString,
    /// `Object.prototype.isPrototypeOf`
    IsPrototypeOf,
    /// `Array.isArray`
    IsArray,
    /// `Array.from`
    ArrayFrom,
    /// `ArrayBuffer.isView`
    IsView,
    /// `Reflect.construct`
    Construct,
    /// `BigInt`
    BigInt,
    /// `Error.prototype`
    ErrorProto,
    /// `Date.prototype`
    DateProto,
    /// `RegExp.prototype`
    RegExpProto,
    /// `Map`
    Map,
    /// `Map.prototype`
    MapProto,
    /// `Map.prototype.get`
    MapGet,
    /// `Map.prototype.set`
    MapSet,
    /// `Map.prototype.delete`
    MapDelete,
    /// `Map.prototype.clear`
    MapClear,
    /// `Set.prototype`
    SetProto,
    /// `Uint8Array`
    Uint8Array,
    /// The native `EventEmitter` class.
    EventEmitter,
    /// The native `Buffer` class.
    Buffer,
    /// `Buffer.prototype`
    BufferProto,
    /// The `process` object.
    Process,
    /// The console's live-region slot (`{ wrap }`), shared with `lunarstorm:ui`.
    Live,
}

/// How many [`Slot`]s exist.
const SLOTS: usize = Slot::Live as usize + 1;

/// The builtins [`install`] captures: `(slot, owner path on the global object, property)`.
const INTRINSICS: [(Slot, &[&str], &str); 26] = [
    (Slot::String, &[], "String"),
    (Slot::Number, &[], "Number"),
    (Slot::ParseInt, &[], "parseInt"),
    (Slot::ParseFloat, &[], "parseFloat"),
    (Slot::Stringify, &["JSON"], "stringify"),
    (Slot::Keys, &["Object"], "keys"),
    (Slot::DefineProperty, &["Object"], "defineProperty"),
    (Slot::SetPrototypeOf, &["Object"], "setPrototypeOf"),
    (Slot::ObjectToString, &["Object", "prototype"], "toString"),
    (Slot::IsPrototypeOf, &["Object", "prototype"], "isPrototypeOf"),
    (Slot::IsArray, &["Array"], "isArray"),
    (Slot::ArrayFrom, &["Array"], "from"),
    (Slot::IsView, &["ArrayBuffer"], "isView"),
    (Slot::Construct, &["Reflect"], "construct"),
    (Slot::BigInt, &[], "BigInt"),
    (Slot::ErrorProto, &["Error"], "prototype"),
    (Slot::DateProto, &["Date"], "prototype"),
    (Slot::RegExpProto, &["RegExp"], "prototype"),
    (Slot::Map, &[], "Map"),
    (Slot::MapProto, &["Map"], "prototype"),
    (Slot::MapGet, &["Map", "prototype"], "get"),
    (Slot::MapSet, &["Map", "prototype"], "set"),
    (Slot::MapDelete, &["Map", "prototype"], "delete"),
    (Slot::MapClear, &["Map", "prototype"], "clear"),
    (Slot::SetProto, &["Set"], "prototype"),
    (Slot::Uint8Array, &[], "Uint8Array"),
];

/// The builtins' state that outlives one call: kept values and the scheduling
/// queues. Owned by the Runtime, shared with the natives' closures.
pub(crate) struct Node<V: Vm> {
    /// Values by [`Slot`]; empty before install and after [`Node::clear`].
    slots: RefCell<Vec<Option<V::Persistent>>>,
    /// Timers, immediates and the `nextTick` queue.
    pub(crate) timers: timers::Queues<V>,
}

impl<V: Vm> Node<V> {
    /// # Returns
    /// Empty state, as before `start`.
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self { slots: RefCell::new((0..SLOTS).map(|_| None).collect()), timers: timers::Queues::default() })
    }

    /// Keeps a value in a slot.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `slot` - where.
    /// * `value` - what.
    pub(crate) fn keep<'s>(&self, scope: &V::Scope<'s>, slot: Slot, value: &V::Value<'s>) {
        self.slots.borrow_mut()[slot as usize] = Some(scope.persist(value));
    }

    /// # Arguments
    /// * `scope` - the context handle.
    /// * `slot` - which value.
    ///
    /// # Returns
    /// The kept value; `undefined` when the slot is empty (after [`Node::clear`]).
    pub(crate) fn get<'s>(&self, scope: &V::Scope<'s>, slot: Slot) -> V::Value<'s> {
        match &self.slots.borrow()[slot as usize] {
            Some(handle) => scope.local(handle),
            None => scope.undefined(),
        }
    }

    /// Drops every handle into the heap. Runs before the VM instance is destroyed.
    pub(crate) fn clear(&self) {
        // Taken out first: dropping a handle must not run while the cell is borrowed.
        let slots = std::mem::take(&mut *self.slots.borrow_mut());
        drop(slots);
        self.timers.clear();
    }
}

/// Shorthand for a [`Param`].
///
/// # Arguments
/// * `name` - the parameter name.
/// * `ty` - its type.
/// * `about` - one line.
///
/// # Returns
/// The parameter.
pub(crate) const fn p(name: &'static str, ty: Ty, about: &'static str) -> Param {
    Param { name, ty, about }
}

/// Shorthand for a [`FnDecl`].
///
/// # Arguments
/// * `name` - the function's name.
/// * `params` - its positional parameters.
/// * `rest` - its rest parameter, if any.
/// * `returns` - its result.
/// * `about` - one line.
///
/// # Returns
/// The declaration.
pub(crate) const fn f(name: &'static str, params: &'static [Param], rest: Option<Param>, returns: Ty, about: &'static str) -> FnDecl {
    FnDecl { name, sig: Sig { params, rest, returns, about } }
}

/// # Returns
/// The declarations of every ported builtin module, then one [`GLOBAL`]
/// declaration of the names installed on the global object.
pub fn declarations() -> Vec<ModuleDecl> {
    let mut global = ModuleDecl { specifier: GLOBAL, items: Vec::new(), about: "Names on the global object." };
    global.items.extend(timers::globals());
    global.items.extend(buffer::globals());
    global.items.extend(console::globals());
    global.items.extend(process::globals());
    vec![path::declare(), events::declare(), timers::declare(), buffer::declare(), process::declare(), global]
}

/// Installs the ported builtins and their globals.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state and host.
/// * `node` - the state the natives keep; filled here.
/// * `ops` - the `ops` object with the host's bindings (the console reads `ops.color`).
///
/// # Returns
/// The object the glue receives: `modules` (the builtin registry so far) and the
/// helpers the remaining glue uses.
///
/// # Errors
/// [`lunarstorm_vm::Thrown`] when a value cannot be created or a builtin is missing.
pub(crate) fn install<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>, ops: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    let global = scope.global();
    for (slot, path, key) in INTRINSICS {
        let mut owner = global.clone();
        for step in path {
            owner = scope.get(&owner, step)?;
        }
        node.keep(scope, slot, &scope.get(&owner, key)?);
    }
    let native = scope.object()?;
    let modules = scope.object()?;
    scope.set(&native, "modules", modules.clone())?;
    scope.set(&modules, "path", path::install::<V, H>(scope, shared)?)?;
    scope.set(&modules, "events", events::install(scope, node, &native)?)?;
    scope.set(&modules, "timers", timers::install(scope, shared, node, &native)?)?;
    scope.set(&modules, "buffer", buffer::install(scope, node, &native)?)?;
    console::install(scope, shared, node, ops, &native)?;
    scope.set(&modules, "process", process::install(scope, shared, node, &native)?)?;
    Ok(native)
}

// ---------------------------------------------------------------- helpers

/// Small operations the builtins share. Methods on [`Node`] so the VM type is
/// known from the receiver and callers need no turbofish.
impl<V: Vm> Node<V> {
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `value` - any value.
    ///
    /// # Returns
    /// Whether JS treats it as true in a condition. Objects, functions, symbols and
    /// big integers count as true (a zero `BigInt` is the one value read wrongly).
    pub(crate) fn truthy<'s>(&self, scope: &V::Scope<'s>, value: &V::Value<'s>) -> bool {
        match scope.kind(value) {
            Kind::Undefined | Kind::Null => false,
            Kind::Bool => scope.as_bool(value).unwrap_or(false),
            Kind::Number => scope.as_number(value).is_some_and(|n| n != 0.0 && !n.is_nan()),
            Kind::String => scope.with_str(value, |text| !text.is_empty()).unwrap_or(true),
            Kind::Bytes | Kind::Function | Kind::Object | Kind::Other => true,
        }
    }

    /// Converts a value to text as `String(value)` does.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `value` - any value.
    ///
    /// # Returns
    /// The text.
    ///
    /// # Errors
    /// What `String()` throws, or a `TypeError` for an unpaired surrogate.
    pub(crate) fn text<'s>(&self, scope: &V::Scope<'s>, value: &V::Value<'s>) -> Outcome<String> {
        if scope.kind(value) == Kind::String {
            return scope.with_str(value, str::to_owned);
        }
        let converted = self.call(scope, Slot::String, std::slice::from_ref(value))?;
        scope.with_str(&converted, str::to_owned)
    }

    /// Calls a kept builtin with `this` undefined.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `slot` - which builtin.
    /// * `args` - the arguments.
    ///
    /// # Returns
    /// Its result.
    ///
    /// # Errors
    /// What the builtin throws.
    pub(crate) fn call<'s>(&self, scope: &V::Scope<'s>, slot: Slot, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
        scope.call(&self.get(scope, slot), args)
    }

    /// `new constructor(...args)`, through `Reflect.construct`.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `constructor` - the class.
    /// * `args` - the arguments.
    ///
    /// # Returns
    /// The instance.
    ///
    /// # Errors
    /// What the constructor throws.
    pub(crate) fn construct<'s>(&self, scope: &V::Scope<'s>, constructor: &V::Value<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
        let list = scope.array(args)?;
        self.call(scope, Slot::Construct, &[constructor.clone(), list])
    }

    /// `value instanceof C`, as `C.prototype.isPrototypeOf(value)`.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `proto` - the slot holding `C.prototype`.
    /// * `value` - any value.
    ///
    /// # Returns
    /// Whether `proto` is on the value's prototype chain.
    ///
    /// # Errors
    /// What a proxy's trap throws.
    pub(crate) fn instance_of<'s>(&self, scope: &V::Scope<'s>, proto: Slot, value: &V::Value<'s>) -> Outcome<bool> {
        let found = scope.call_with(&self.get(scope, Slot::IsPrototypeOf), &self.get(scope, proto), std::slice::from_ref(value))?;
        Ok(scope.as_bool(&found).unwrap_or(false))
    }

    /// # Arguments
    /// * `scope` - the context handle.
    /// * `value` - an array-like object.
    ///
    /// # Returns
    /// Its `length` as an integer; 0 when it is not a positive number.
    ///
    /// # Errors
    /// What a getter throws.
    pub(crate) fn length<'s>(&self, scope: &V::Scope<'s>, value: &V::Value<'s>) -> Outcome<usize> {
        let length = scope.get(value, "length")?;
        Ok(scope.as_number(&length).filter(|n| *n > 0.0).map_or(0, |n| n as usize))
    }

    /// # Arguments
    /// * `scope` - the context handle.
    /// * `value` - an array-like object.
    /// * `index` - the element's index.
    ///
    /// # Returns
    /// The element.
    ///
    /// # Errors
    /// What a getter throws.
    pub(crate) fn at<'s>(&self, scope: &V::Scope<'s>, value: &V::Value<'s>, index: usize) -> Outcome<V::Value<'s>> {
        scope.get(value, &index.to_string())
    }

    /// # Arguments
    /// * `scope` - the context handle.
    /// * `value` - an array-like object.
    ///
    /// # Returns
    /// Its elements, in order.
    ///
    /// # Errors
    /// What a getter throws.
    pub(crate) fn items<'s>(&self, scope: &V::Scope<'s>, value: &V::Value<'s>) -> Outcome<Vec<V::Value<'s>>> {
        (0..self.length(scope, value)?).map(|index| self.at(scope, value, index)).collect()
    }

    /// Defines a property that `Object.keys` does not list: not enumerable, not
    /// writable, not configurable.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `object` - where.
    /// * `key` - the property name.
    /// * `value` - its value.
    ///
    /// # Errors
    /// A `TypeError` when the object is frozen or the property exists.
    pub(crate) fn define_hidden<'s>(&self, scope: &V::Scope<'s>, object: &V::Value<'s>, key: &str, value: V::Value<'s>) -> Outcome<()> {
        let descriptor = scope.object()?;
        scope.set(&descriptor, "value", value)?;
        self.call(scope, Slot::DefineProperty, &[object.clone(), scope.string(key)?, descriptor]).map(|_| ())
    }
}

/// Reads an optional argument.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `args` - the call's arguments.
/// * `index` - position.
///
/// # Returns
/// The argument; `undefined` when it was left out.
pub(crate) fn arg<'s, V: Vm>(scope: &V::Scope<'s>, args: &[V::Value<'s>], index: usize) -> V::Value<'s> {
    args.get(index).cloned().unwrap_or_else(|| scope.undefined())
}
