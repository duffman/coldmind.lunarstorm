//! `node:events`: `EventEmitter` as a native class that scripts subclass.
//!
//! Decisions:
//! - Listeners live in a `Map` from event name to an array of entries, held in a
//!   non-enumerable own property ([`STORE`]) created on the first listener. A
//!   `Map` takes symbols as names, which a property key from Rust cannot; the
//!   storage stays in the JS heap, so listeners that reference their emitter are
//!   collected with it.
//! - Arrays are copy-on-write: `on`/`off` store a new array, so `emit` iterates
//!   the array it read and a listener added or removed during an emit does not
//!   change that emit (pinned by `node_surface`).
//! - An entry is the listener itself, or for `once` a record `{ listener, fired }`
//!   instead of a wrapper function; see [`ONCE`].

use super::{arg, f, p, Node, Slot};
use crate::bindings::declared;
use lunarstorm_vm::{Call, ClassDecl, ClassSpec, ErrorKind, FnDecl, Item, Kind, ModuleDecl, Outcome, Param, Scope, Sig, Ty, Vm};
use std::rc::Rc;

/// The own property holding an emitter's listener `Map`.
///
/// Fix (node_fixes `emitter_keys_hide_listener_storage`): the glue kept listeners
/// in an enumerable `_ev`, which `Object.keys`, `for...in` and `console.log` of
/// every emitter showed. Decision: define it non-enumerable (and not writable or
/// configurable) with `Object.defineProperty` on the first listener; Node's own
/// name, `_events`, is kept for scripts that look for it.
const STORE: &str = "_events";

/// What a listener receives.
const LISTENER_SIG: Sig = Sig { params: &[], rest: Some(p("args", Ty::Any, "the emit arguments")), returns: Ty::Unknown, about: "A listener." };
/// An event name.
const EVENT: Param = p("type", Ty::Any, "event name: a string or a symbol");
/// A listener argument.
const LISTENER: Param = p("listener", Ty::Function(&LISTENER_SIG), "called with the emitter as `this`");
/// What chaining methods return.
const THIS: Ty = Ty::Named("this");

const ON: FnDecl = f("on", &[EVENT, LISTENER], None, THIS, "Adds a listener.");
const ADD_LISTENER: FnDecl = f("addListener", &[EVENT, LISTENER], None, THIS, "Adds a listener; the same function as `on`.");
const ONCE: FnDecl = f("once", &[EVENT, LISTENER], None, THIS, "Adds a listener that is removed before its first call.");
const OFF: FnDecl = f("off", &[EVENT, LISTENER], None, THIS, "Removes a listener, or a `once` listener by its function.");
const REMOVE_LISTENER: FnDecl = f("removeListener", &[EVENT, LISTENER], None, THIS, "Removes a listener; the same function as `off`.");
const EMIT: FnDecl = f(
    "emit",
    &[EVENT],
    Some(p("args", Ty::Any, "listener arguments")),
    Ty::Bool,
    "Calls every listener of an event; an `error` event with no listener throws.",
);
const REMOVE_ALL: FnDecl = f(
    "removeAllListeners",
    &[p("type", Ty::Any, "event name; every event when left out")],
    None,
    THIS,
    "Removes all listeners of one event, or of every event.",
);
const LISTENERS: FnDecl = f("listeners", &[EVENT], None, Ty::Array(&Ty::Function(&LISTENER_SIG)), "A copy of the listeners of an event.");
const LISTENER_COUNT: FnDecl = f("listenerCount", &[EVENT], None, Ty::Number, "How many listeners an event has.");

/// The `EventEmitter` class. Node exports the class itself as `events`, with
/// `EventEmitter` as a property of it.
pub(crate) const EVENT_EMITTER: ClassDecl = ClassDecl {
    name: "EventEmitter",
    extends: None,
    constructor: Sig { params: &[], rest: None, returns: Ty::Named("EventEmitter"), about: "An emitter with no listeners." },
    methods: &[ON, ADD_LISTENER, ONCE, OFF, REMOVE_LISTENER, EMIT, REMOVE_ALL, LISTENERS, LISTENER_COUNT],
    accessors: &[],
    statics: &[],
    about: "Node-compatible event emitter.",
};

/// # Returns
/// The declaration of `node:events`.
pub(crate) fn declare() -> ModuleDecl {
    ModuleDecl { specifier: "node:events", items: vec![Item::Class(EVENT_EMITTER)], about: "The event emitter." }
}

/// The listener `Map` of an emitter.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `this` - the emitter.
/// * `create` - whether to create the storage when the emitter has none yet.
///
/// # Returns
/// The `Map`, or `None` when there is none and `create` is false.
///
/// # Errors
/// A `TypeError` when `this` is not an object.
fn store<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, this: &V::Value<'s>, create: bool) -> Outcome<Option<V::Value<'s>>> {
    let found = scope.get(this, STORE)?;
    if scope.kind(&found) == Kind::Object {
        return Ok(Some(found));
    }
    if !create {
        return Ok(None);
    }
    let map = node.construct(scope, &node.get(scope, Slot::Map), &[])?;
    node.define_hidden(scope, this, STORE, map.clone())?;
    Ok(Some(map))
}

/// The entries of one event.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `this` - the emitter.
/// * `event` - the event name.
///
/// # Returns
/// The entries, in order; empty when the event has none.
///
/// # Errors
/// A `TypeError` when `this` is not an object.
fn entries<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, this: &V::Value<'s>, event: &V::Value<'s>) -> Outcome<Vec<V::Value<'s>>> {
    let Some(map) = store(node, scope, this, false)? else { return Ok(Vec::new()) };
    let list = scope.call_with(&node.get(scope, Slot::MapGet), &map, std::slice::from_ref(event))?;
    if scope.kind(&list) == Kind::Undefined {
        return Ok(Vec::new());
    }
    node.items(scope, &list)
}

/// Stores a new entry array for an event; an empty one removes the event.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `this` - the emitter.
/// * `event` - the event name.
/// * `list` - the entries.
///
/// # Errors
/// A `TypeError` when `this` is not an object or cannot take the storage.
fn put<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, this: &V::Value<'s>, event: &V::Value<'s>, list: &[V::Value<'s>]) -> Outcome<()> {
    let Some(map) = store(node, scope, this, !list.is_empty())? else { return Ok(()) };
    if list.is_empty() {
        scope.call_with(&node.get(scope, Slot::MapDelete), &map, std::slice::from_ref(event))?;
    } else {
        scope.call_with(&node.get(scope, Slot::MapSet), &map, &[event.clone(), scope.array(list)?])?;
    }
    Ok(())
}

/// # Arguments
/// * `scope` - the context handle.
/// * `entry` - a listener or a `once` record.
///
/// # Returns
/// The listener function the entry stands for.
///
/// # Errors
/// What reading the record throws.
fn listener<'s, V: Vm>(scope: &V::Scope<'s>, entry: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    if scope.kind(entry) == Kind::Object {
        scope.get(entry, "listener")
    } else {
        Ok(entry.clone())
    }
}

/// Removes the first entry an identity test picks.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `this` - the emitter.
/// * `event` - the event name.
/// * `matches` - picks the entry to remove.
///
/// # Errors
/// What reading or storing the entries throws.
fn remove<'s, V: Vm>(
    node: &Node<V>,
    scope: &V::Scope<'s>,
    this: &V::Value<'s>,
    event: &V::Value<'s>,
    matches: impl Fn(&V::Value<'s>) -> Outcome<bool>,
) -> Outcome<()> {
    let mut list = entries(node, scope, this, event)?;
    for index in 0..list.len() {
        if matches(&list[index])? {
            list.remove(index);
            return put(node, scope, this, event, &list);
        }
    }
    Ok(())
}

/// Calls the listeners of an event with the emitter as `this`.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `this` - the emitter.
/// * `event` - the event name.
/// * `args` - the listener arguments.
///
/// # Returns
/// Whether the event had listeners.
///
/// # Errors
/// What a listener throws.
fn fire<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, this: &V::Value<'s>, event: &V::Value<'s>, args: &[V::Value<'s>]) -> Outcome<bool> {
    let list = entries(node, scope, this, event)?;
    for entry in &list {
        if scope.kind(entry) == Kind::Function {
            scope.call_with(entry, this, args)?;
            continue;
        }
        // A `once` record: taken out before its call. `fired` covers an outer
        // emit still iterating the array a nested emit already fired it from.
        if node.truthy(scope, &scope.get(entry, "fired")?) {
            continue;
        }
        scope.set(entry, "fired", scope.boolean(true))?;
        let id = scope.identity(entry);
        remove(node, scope, this, event, |other| Ok(scope.kind(other) == Kind::Object && scope.identity(other) == id))?;
        scope.call_with(&scope.get(entry, "listener")?, this, args)?;
    }
    Ok(!list.is_empty())
}

/// Builds the `EventEmitter` class and the internal `emit2`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `node` - the kept builtins; the class is kept as [`Slot::EventEmitter`].
/// * `native` - the object the glue receives; `emit2` is set on it.
///
/// # Returns
/// The class, which is also the `node:events` module object.
///
/// # Errors
/// [`lunarstorm_vm::Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm>(scope: &V::Scope<'s>, node: &Rc<Node<V>>, native: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    let n = node.clone();
    let on = declared(scope, ON.name, ON.sig, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        let mut list = entries(&n, scope, this, &args[0])?;
        list.push(args[1].clone());
        put(&n, scope, this, &args[0], &list)?;
        Ok(this.clone())
    })?;
    let n = node.clone();
    // Fix (node_fixes `emitter_listeners_unwrap_once`): the glue stored a wrapper
    // function, which `listeners()` returned instead of the function given to
    // `once`. Decision: store a record `{ listener, fired }`; `listeners()` maps it
    // back, `off` matches its `listener`, `emit` removes it before the call.
    // Dropped alternative: a native wrapper closure, which would hold the listener
    // as a persistent handle the collector cannot trace.
    let once = declared(scope, ONCE.name, ONCE.sig, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        let record = scope.object()?;
        scope.set(&record, "listener", args[1].clone())?;
        let mut list = entries(&n, scope, this, &args[0])?;
        list.push(record);
        put(&n, scope, this, &args[0], &list)?;
        Ok(this.clone())
    })?;
    let n = node.clone();
    let off = declared(scope, OFF.name, OFF.sig, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        let id = scope.identity(&args[1]);
        remove(&n, scope, this, &args[0], |entry| {
            let entry = listener::<V>(scope, entry)?;
            Ok(scope.kind(&entry) == Kind::Function && scope.identity(&entry) == id)
        })?;
        Ok(this.clone())
    })?;
    let n = node.clone();
    let emit = declared(scope, EMIT.name, EMIT.sig, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        if fire(&n, scope, this, &args[0], &args[1..])? {
            return Ok(scope.boolean(true));
        }
        let is_error = scope.kind(&args[0]) == Kind::String && scope.with_str(&args[0], |name| name == "error")?;
        if !is_error {
            return Ok(scope.boolean(false));
        }
        let thrown = arg::<V>(scope, args, 1);
        if n.instance_of(scope, Slot::ErrorProto, &thrown)? {
            return Err(scope.throw(thrown));
        }
        let message = format!("Unhandled error: {}", n.text(scope, &thrown)?);
        Err(scope.throw_error(ErrorKind::Error, &message))
    })?;
    let n = node.clone();
    let remove_all = declared(scope, REMOVE_ALL.name, REMOVE_ALL.sig, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        let event = arg::<V>(scope, call.args(), 0);
        if let Some(map) = store(&n, scope, this, false)? {
            if scope.kind(&event) == Kind::Undefined {
                scope.call_with(&n.get(scope, Slot::MapClear), &map, &[])?;
            } else {
                scope.call_with(&n.get(scope, Slot::MapDelete), &map, &[event])?;
            }
        }
        Ok(this.clone())
    })?;
    let n = node.clone();
    let listeners = declared(scope, LISTENERS.name, LISTENERS.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let list = entries(&n, scope, call.this(), &call.args()[0])?;
        let functions = list.iter().map(|entry| listener::<V>(scope, entry)).collect::<Outcome<Vec<_>>>()?;
        scope.array(&functions)
    })?;
    let n = node.clone();
    let count = declared(scope, LISTENER_COUNT.name, LISTENER_COUNT.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let list = entries(&n, scope, call.this(), &call.args()[0])?;
        Ok(scope.number(list.len() as f64))
    })?;
    // `addListener` and `removeListener` are the same functions as `on` and
    // `off`, as in Node and the glue.
    let spec = ClassSpec::new(EVENT_EMITTER.name)
        .method(ON.name, on.clone())
        .method(ADD_LISTENER.name, on)
        .method(ONCE.name, once)
        .method(OFF.name, off.clone())
        .method(REMOVE_LISTENER.name, off)
        .method(EMIT.name, emit)
        .method(REMOVE_ALL.name, remove_all)
        .method(LISTENERS.name, listeners)
        .method(LISTENER_COUNT.name, count);
    let class = scope.class::<(), _>(spec, |_: &Call<'_, V>| Ok(()))?;
    // Enumerable on purpose: the module's named exports are the class's own keys.
    scope.set(&class, EVENT_EMITTER.name, class.clone())?;
    node.keep(scope, Slot::EventEmitter, &class);

    // `emit` for the glue's hot paths: two arguments, no `error` check, and the
    // emitter's own `emit` is not consulted (a subclass's override is bypassed).
    let n = node.clone();
    let emit2 = scope.function("emit2", move |call: &Call<'_, V>| {
        let scope = call.scope();
        let args = call.args();
        // A missing emitter throws, as the glue's `self._ev` read did.
        let this = arg::<V>(scope, args, 0);
        let pair = [arg::<V>(scope, args, 2), arg::<V>(scope, args, 3)];
        Ok(scope.boolean(fire(&n, scope, &this, &arg::<V>(scope, args, 1), &pair)?))
    })?;
    scope.set(native, "emit2", emit2)?;
    Ok(class)
}
