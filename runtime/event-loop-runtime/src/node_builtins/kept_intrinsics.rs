//! [`Node`]: what the builtins keep between calls, by [`Slot`]: captured
//! intrinsics (`Map.prototype.get`, `Object.keys`, ...), the classes the natives
//! build (`EventEmitter`, `Buffer.prototype`, `process`), the timer queues and
//! the open handles; plus the small value helpers the builtins share.

use lunarstorm_engine_contract::{Kind, Outcome, Scope, Vm};
use std::cell::RefCell;
use std::rc::Rc;
use super::*;

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
    /// `JSON.parse`
    Parse,
    /// `Reflect.deleteProperty`
    DeleteProperty,
    /// `Function.prototype.bind`
    Bind,
    /// `Date`
    Date,
    /// `Symbol.asyncIterator`
    AsyncIterator,
    /// The native `fs.Stats` class.
    Stats,
    /// The native `net.Socket` class.
    Socket,
    /// `net.Socket.prototype`
    SocketProto,
    /// The native `http.IncomingMessage` class.
    IncomingMessage,
    /// The native `http.ServerResponse` class.
    ServerResponse,
    /// `http.Server.prototype`
    HttpServerProto,
    /// `(target, name, ...args)`: `target.emit(name, ...args)`, queued with `nextTick`.
    EmitLater,
    /// The `next` of a request body's async iterator.
    BodyNext,
    /// The native `net.Server` class.
    NetServer,
    /// The native `http.Server` class.
    HttpServer,
    /// `(socket, error)`: emits `error` (if any) then `close`; queued by `destroy`.
    AfterDestroy,
    /// `(server, cb)`: emits `close` then calls `cb`; queued by `server.close`.
    CloseLater,
    /// `(request)`: emits a request's buffered body as `data` and `end`.
    BodyDeliver,
    /// `queueMicrotask`, as installed on the global object.
    QueueMicrotask,
    /// The builtin module registry (`modules`).
    Modules,
}

/// How many [`Slot`]s exist.
pub(crate) const SLOTS: usize = Slot::Modules as usize + 1;

/// The builtins [`install`] captures: `(slot, owner path on the global object, property)`.
pub(crate) const INTRINSICS: [(Slot, &[&str], &str); 31] = [
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
    (Slot::Parse, &["JSON"], "parse"),
    (Slot::DeleteProperty, &["Reflect"], "deleteProperty"),
    (Slot::Bind, &["Function", "prototype"], "bind"),
    (Slot::Date, &[], "Date"),
    (Slot::AsyncIterator, &["Symbol"], "asyncIterator"),
];

/// The builtins' state that outlives one call: kept values and the scheduling
/// queues. Owned by the Runtime, shared with the natives' closures.
pub(crate) struct Node<V: Vm> {
    /// Values by [`Slot`]; empty before install and after [`Node::clear`].
    slots: RefCell<Vec<Option<V::Persistent>>>,
    /// Timers, immediates and the `nextTick` queue.
    pub(crate) timers: timers_and_ticks::Queues<V>,
    /// Live sockets and servers by reactor slab key: what an I/O event is
    /// delivered to.
    pub(crate) handles: RefCell<Vec<Option<V::Persistent>>>,
    /// Asynchronous fs requests waiting for the worker pool.
    pub(crate) fs: fs_module::Pending<V>,
}

impl<V: Vm> Node<V> {
    /// # Returns
    /// Empty state, as before `start`.
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            slots: RefCell::new((0..SLOTS).map(|_| None).collect()),
            timers: timers_and_ticks::Queues::default(),
            handles: RefCell::default(),
            fs: fs_module::Pending::default(),
        })
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
        let handles = std::mem::take(&mut *self.handles.borrow_mut());
        drop(handles);
        self.fs.clear();
    }

    /// Keeps a socket or server as the receiver of the events of a slab key.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `key` - the reactor slab key.
    /// * `handle` - the socket or server; `None` forgets the key.
    pub(crate) fn set_handle<'s>(&self, scope: &V::Scope<'s>, key: usize, handle: Option<&V::Value<'s>>) {
        let kept = handle.map(|value| scope.persist(value));
        let old = {
            let mut handles = self.handles.borrow_mut();
            if handles.len() <= key {
                handles.resize_with(key + 1, || None);
            }
            std::mem::replace(&mut handles[key], kept)
        };
        drop(old);
    }

    /// # Arguments
    /// * `scope` - the context handle.
    /// * `key` - the reactor slab key.
    ///
    /// # Returns
    /// The socket or server kept for the key, if any.
    pub(crate) fn handle<'s>(&self, scope: &V::Scope<'s>, key: usize) -> Option<V::Value<'s>> {
        self.handles.borrow().get(key).and_then(Option::as_ref).map(|handle| scope.local(handle))
    }
}

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
