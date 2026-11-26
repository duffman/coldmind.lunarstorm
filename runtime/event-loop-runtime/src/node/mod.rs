//! Node's builtin modules and globals, as natives.
//!
//! One file per module (D4): each has `declare()`, the declaration its exports
//! are installed from, and `install(...)`, which builds the module's object.
//! [`install`] runs them at `start`, after the `ops` object and the host's
//! bindings, and hands the JS glue the result: `{ modules, ...internals }`.
//!
//! Decisions:
//! - [`install`] returns the object the loop and the loader read (D3): the
//!   hooks `dispatch`, `ticks`, `describe`, `exitCode` and `invoke`, and the
//!   registry parts `modules`, `internal` and `exportsOf`. It is what the JS glue
//!   used to return; no script runs at `start`. Scripts never see that object.
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
pub(crate) mod fs;
pub(crate) mod http;
pub(crate) mod net;
pub(crate) mod path;
pub(crate) mod process;
pub(crate) mod timers;

use crate::host::Host;
use crate::io::Shared;
use lunarstorm_engine_contract::{FnDecl, Kind, ModuleDecl, Outcome, Param, Scope, Sig, Ty, Vm};
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
const SLOTS: usize = Slot::Modules as usize + 1;

/// The builtins [`install`] captures: `(slot, owner path on the global object, property)`.
const INTRINSICS: [(Slot, &[&str], &str); 31] = [
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
    pub(crate) timers: timers::Queues<V>,
    /// Live sockets and servers by reactor slab key: what an I/O event is
    /// delivered to.
    pub(crate) handles: RefCell<Vec<Option<V::Persistent>>>,
    /// Asynchronous fs requests waiting for the worker pool.
    pub(crate) fs: fs::Pending<V>,
}

impl<V: Vm> Node<V> {
    /// # Returns
    /// Empty state, as before `start`.
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            slots: RefCell::new((0..SLOTS).map(|_| None).collect()),
            timers: timers::Queues::default(),
            handles: RefCell::default(),
            fs: fs::Pending::default(),
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
    let mut modules = vec![path::declare(), events::declare(), timers::declare(), buffer::declare(), process::declare()];
    modules.extend(fs::declare());
    modules.push(net::declare());
    modules.push(http::declare());
    modules.push(global);
    modules
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
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created or a builtin is missing.
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
    let (fs, promises) = fs::install(scope, shared, node)?;
    scope.set(&modules, "fs", fs)?;
    scope.set(&modules, "fs/promises", promises)?;
    node.keep(scope, Slot::QueueMicrotask, &scope.get(&global, "queueMicrotask")?);
    let (net, net_server) = net::install(scope, shared, node)?;
    scope.set(&modules, "net", net)?;
    scope.set(&modules, "http", http::install(scope, shared, node, &net_server)?)?;
    hooks(scope, shared, node, ops, &native, &modules)?;
    Ok(native)
}

/// Sets the hooks the loop calls on `native`: `dispatch`, `describe`,
/// `exitCode`, `invoke`, and the registry parts `modules`, `internal` and
/// `exportsOf` (`ticks` is set by [`timers`]).
///
/// Decision (D3): these were the JS glue's return value; they are natives now,
/// so the loop's calls into them stay the same and no script is compiled at
/// `start`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state.
/// * `node` - the kept builtins.
/// * `ops` - the `ops` object, exported by `lunarstorm:internal`.
/// * `native` - where the hooks are set.
/// * `modules` - the builtin registry.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
fn hooks<'s, V: Vm, H: Host>(
    scope: &V::Scope<'s>,
    shared: &Rc<Shared<H>>,
    node: &Rc<Node<V>>,
    ops: &V::Value<'s>,
    native: &V::Value<'s>,
    modules: &V::Value<'s>,
) -> Outcome<()> {
    // What `lunarstorm:internal` exports, for host modules only: the natives plus
    // the host's (`ops`) and the console's live-region slot (`live`).
    let internal = scope.object()?;
    scope.set(&internal, "ops", ops.clone())?;
    scope.set(&internal, "live", scope.get(native, "live")?)?;
    scope.set(native, "internal", internal)?;
    node.keep(scope, Slot::Modules, modules);
    let n = node.clone();
    let exports_of = scope.function("exportsOf", move |call: &lunarstorm_engine_contract::Call<'_, V>| {
        let scope = call.scope();
        let modules = n.get(scope, Slot::Modules);
        let name = n.text(scope, &arg::<V>(scope, call.args(), 0))?;
        let keys = n.call(scope, Slot::Keys, &[scope.get(&modules, &name)?])?;
        let names = n.items(scope, &keys)?.iter().map(|key| n.text(scope, key)).collect::<Outcome<Vec<_>>>()?;
        scope.string(&names.join(","))
    })?;
    scope.set(native, "exportsOf", exports_of)?;
    let n = node.clone();
    let dispatch_fn = scope.function("dispatch", move |call: &lunarstorm_engine_contract::Call<'_, V>| dispatch(&n, call.scope(), call.args()))?;
    scope.set(native, "dispatch", dispatch_fn)?;
    let n = node.clone();
    let describe = scope.function("describe", move |call: &lunarstorm_engine_contract::Call<'_, V>| {
        let scope = call.scope();
        scope.string(&console::inspect(&n, scope, &arg::<V>(scope, call.args(), 0))?)
    })?;
    scope.set(native, "describe", describe)?;
    let n = node.clone();
    let exit_code = scope.function("exitCode", move |call: &lunarstorm_engine_contract::Call<'_, V>| {
        let scope = call.scope();
        let code = scope.get(&n.get(scope, Slot::Process), "exitCode")?;
        let code = scope.as_number(&n.call(scope, Slot::Number, &[code])?).unwrap_or(0.0);
        Ok(scope.int(process::to_int32(code)))
    })?;
    scope.set(native, "exitCode", exit_code)?;
    let (s, n) = (shared.clone(), node.clone());
    let invoke = scope.function("invoke", move |call: &lunarstorm_engine_contract::Call<'_, V>| invoke(&n, &s, call.scope(), call.args()))?;
    scope.set(native, "invoke", invoke)?;
    Ok(())
}

/// Reduces a value to what crosses the host boundary: null, boolean, number,
/// string or bytes. Decision: anything else travels as JSON text, so the host
/// interface stays engine-neutral and holds no JS handles.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `value` - what an export returned or resolved to.
///
/// # Returns
/// The neutral value.
fn plain<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, value: &V::Value<'s>) -> crate::runtime::Arg {
    use crate::runtime::Arg;
    match scope.kind(value) {
        Kind::Undefined | Kind::Null => Arg::Null,
        Kind::Bool | Kind::Number | Kind::String | Kind::Bytes => crate::ops::neutral::<V>(scope, value),
        _ => {
            let json = node.call(scope, Slot::Stringify, std::slice::from_ref(value));
            match json {
                Ok(json) if scope.kind(&json) == Kind::String => crate::ops::neutral::<V>(scope, &json),
                Ok(_) => Arg::Null,
                // A cycle or a throwing `toJSON`: the inspector's text instead.
                Err(_) => console::inspect(node, scope, value).map(Arg::Text).unwrap_or(Arg::Null),
            }
        }
    }
}

/// `invoke(fn, id, ...args)`: calls an export for the host (`Runtime::call`) and
/// reports how its promise settles in `Shared::calls`. A synchronous throw
/// rejects the same promise, as `new Promise((r) => r(fn(...args)))` did.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `shared` - loop state; the settled calls go there.
/// * `scope` - the context handle.
/// * `args` - `(fn, id, ...args)`.
///
/// # Returns
/// `undefined`.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when the promise cannot be made.
fn invoke<'s, V: Vm, H: Host>(node: &Rc<Node<V>>, shared: &Rc<Shared<H>>, scope: &V::Scope<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
    let function = arg::<V>(scope, args, 0);
    let id = scope.as_number(&arg::<V>(scope, args, 1)).unwrap_or(0.0) as u32;
    let outcome = scope.call(&function, args.get(2..).unwrap_or(&[])).map_err(|_| scope.exception());
    let (promise, resolver) = scope.promise()?;
    scope.settle(&resolver, outcome)?;
    let (s, n) = (shared.clone(), node.clone());
    let fulfilled = scope.function("fulfilled", move |call: &lunarstorm_engine_contract::Call<'_, V>| {
        let scope = call.scope();
        let value = plain(&n, scope, &arg::<V>(scope, call.args(), 0));
        s.calls.borrow_mut().push((id, Ok(value)));
        Ok(scope.undefined())
    })?;
    let (s, n) = (shared.clone(), node.clone());
    let rejected = scope.function("rejected", move |call: &lunarstorm_engine_contract::Call<'_, V>| {
        let scope = call.scope();
        let text = console::inspect(&n, scope, &arg::<V>(scope, call.args(), 0)).unwrap_or_default();
        s.calls.borrow_mut().push((id, Err(text)));
        Ok(scope.undefined())
    })?;
    method::<V>(scope, &promise, "then", &[fulfilled, rejected])?;
    Ok(scope.undefined())
}

/// The single entry point for loop events (D3). Codes are
/// [`crate::runtime::ev`].
///
/// # Arguments
/// * `node` - the kept builtins and handles.
/// * `scope` - the context handle.
/// * `args` - `(kind, id, a, b, c, d, e)`.
///
/// # Returns
/// For an immediate, whether more remain; otherwise `undefined`.
///
/// # Errors
/// What a listener or callback throws.
fn dispatch<'s, V: Vm>(node: &Rc<Node<V>>, scope: &V::Scope<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
    use crate::runtime::ev;
    let number = |index: usize| scope.as_number(&arg::<V>(scope, args, index)).unwrap_or(-1.0);
    let (kind, id) = (number(0) as i32, number(1));
    let a = || arg::<V>(scope, args, 2);
    let handle = || if id >= 0.0 { node.handle(scope, id as usize) } else { None };
    let event = |name: &str| scope.string(name);
    match kind {
        ev::TIMER => {
            if id >= 0.0 {
                node.timers.fire_timer(scope, id as usize, scope.as_bool(&a()).unwrap_or(true))?;
            }
        }
        ev::IMMEDIATE => return Ok(scope.boolean(node.timers.run_immediate(scope)?)),
        ev::HTTP => {
            let raw_method = arg::<V>(scope, args, 3);
            let method_name = match scope.as_number(&raw_method) {
                Some(index) => scope.string(http::METHODS.get(index as usize).copied().unwrap_or(""))?,
                None => raw_method,
            };
            let request = node.construct(
                scope,
                &node.get(scope, Slot::IncomingMessage),
                &[method_name, arg::<V>(scope, args, 4), arg::<V>(scope, args, 5), arg::<V>(scope, args, 6)],
            )?;
            let response = node.construct(scope, &node.get(scope, Slot::ServerResponse), &[a()])?;
            scope.set(&response, "req", request.clone())?;
            let heard = match handle() {
                Some(server) => events::fire(node, scope, &server, &event("request")?, &[request, response.clone()])?,
                None => false,
            };
            // No listener: answer rather than leave the client hanging.
            if !heard {
                let head = method::<V>(scope, &response, "writeHead", &[scope.int(404)])?;
                method::<V>(scope, &head, "end", &[])?;
            }
        }
        ev::DATA => {
            if let Some(socket) = handle() {
                let chunk = buffer::as_buffer(node, scope, a())?;
                events::fire(node, scope, &socket, &event("data")?, &[chunk])?;
            }
        }
        ev::CONNECTION => {
            let Some(server) = handle() else { return Ok(scope.undefined()) };
            let socket = node.construct(scope, &node.get(scope, Slot::Socket), std::slice::from_ref(&server))?;
            let conn = a();
            scope.set(&socket, "_k", conn.clone())?;
            scope.set(&socket, "remoteAddress", arg::<V>(scope, args, 3))?;
            scope.set(&socket, "remotePort", arg::<V>(scope, args, 4))?;
            scope.set(&socket, "server", server.clone())?;
            if let Some(key) = scope.as_number(&conn).filter(|key| *key >= 0.0) {
                node.set_handle(scope, key as usize, Some(&socket));
            }
            events::fire(node, scope, &server, &event("connection")?, &[socket])?;
        }
        ev::END => {
            let Some(socket) = handle() else { return Ok(scope.undefined()) };
            events::fire(node, scope, &socket, &event("end")?, &[])?;
            if !net::flag(node, scope, &socket, "allowHalfOpen")? {
                method::<V>(scope, &socket, "end", &[])?;
            }
        }
        ev::CLOSE => {
            let Some(socket) = handle() else { return Ok(scope.undefined()) };
            node.set_handle(scope, id as usize, None);
            scope.set(&socket, "_k", scope.int(-1))?;
            scope.set(&socket, "destroyed", scope.boolean(true))?;
            scope.set(&socket, "connecting", scope.boolean(false))?;
            let message = a();
            let had = scope.kind(&message) != Kind::Undefined;
            if had {
                let code = arg::<V>(scope, args, 3);
                let code = if scope.kind(&code) == Kind::Undefined { None } else { Some(node.text(scope, &code)?) };
                let error = coded_error::<V>(scope, &node.text(scope, &message)?, code.as_deref())?;
                method::<V>(scope, &socket, "emit", &[event("error")?, error])?;
            }
            events::fire(node, scope, &socket, &event("close")?, &[scope.boolean(had)])?;
        }
        ev::DRAIN => {
            if let Some(socket) = handle() {
                events::fire(node, scope, &socket, &event("drain")?, &[])?;
            }
        }
        ev::CONNECT => {
            let Some(socket) = handle() else { return Ok(scope.undefined()) };
            scope.set(&socket, "connecting", scope.boolean(false))?;
            let queue = scope.get(&socket, "_queue")?;
            scope.set(&socket, "_queue", scope.undefined())?;
            // Replay what was written while connecting, then the deferred end().
            let ended = net::flag(node, scope, &socket, "_ended")?;
            scope.set(&socket, "_ended", scope.boolean(false))?;
            if scope.kind(&queue) == Kind::Object {
                for entry in node.items(scope, &queue)? {
                    method::<V>(scope, &socket, "write", &[node.at(scope, &entry, 0)?, node.at(scope, &entry, 1)?])?;
                }
            }
            if ended {
                method::<V>(scope, &socket, "end", &[])?;
            }
            events::fire(node, scope, &socket, &event("connect")?, &[])?;
        }
        ev::FS => {
            let message = arg::<V>(scope, args, 3);
            let result = if scope.kind(&message) == Kind::Undefined {
                Ok(a())
            } else {
                Err((node.text(scope, &message)?, node.text(scope, &arg::<V>(scope, args, 4))?))
            };
            fs::done(node, scope, id as i32, result)?;
        }
        _ => {}
    }
    Ok(scope.undefined())
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

/// Builds an `Error` carrying a Node-style `code`, as the glue's `codedError` did.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `message` - the message.
/// * `code` - the symbolic code (`ECONNRESET`), if any.
///
/// # Returns
/// The error object.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when allocation fails.
pub(crate) fn coded_error<'s, V: Vm>(scope: &V::Scope<'s>, message: &str, code: Option<&str>) -> Outcome<V::Value<'s>> {
    let error = scope.error(lunarstorm_engine_contract::ErrorKind::Error, message)?;
    if let Some(code) = code {
        scope.set(&error, "code", scope.string(code)?)?;
    }
    Ok(error)
}

/// Calls a method by name, looked up on the receiver at call time, so a
/// subclass's override is honoured (`this.emit(...)` in the glue).
///
/// # Arguments
/// * `scope` - the context handle.
/// * `this` - the receiver.
/// * `name` - the method name.
/// * `args` - the arguments.
///
/// # Returns
/// The method's result.
///
/// # Errors
/// What the lookup or the method throws; a `TypeError` when it is not a function.
pub(crate) fn method<'s, V: Vm>(scope: &V::Scope<'s>, this: &V::Value<'s>, name: &str, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
    let function = scope.get(this, name)?;
    if scope.kind(&function) != Kind::Function {
        return Err(scope.throw_error(lunarstorm_engine_contract::ErrorKind::Type, &format!("{name} is not a function")));
    }
    scope.call_with(&function, this, args)
}

/// Whether a property read on a value cannot throw for want of an object:
/// objects, functions and typed arrays.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - any value.
///
/// # Returns
/// `true` for an object-like value.
pub(crate) fn is_object<'s, V: Vm>(scope: &V::Scope<'s>, value: &V::Value<'s>) -> bool {
    matches!(scope.kind(value), Kind::Object | Kind::Function | Kind::Bytes)
}
