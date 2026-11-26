//! `node:net`: `Socket` and `Server` as native classes extending `EventEmitter`.
//!
//! Decisions:
//! - The state a script can see (`connecting`, `destroyed`, `remoteAddress`,
//!   `listening`, ...) and the glue's bookkeeping (`_k`, the reactor slab key;
//!   `_queue`, writes made while connecting; `_ended`) stay own properties of the
//!   instance, as in the glue, so subclasses and `console.log` see the same object.
//! - Sockets and servers are kept in [`Node::handles`] by slab key; the loop's
//!   events find their receiver there ([`super::dispatch`]).
//! - Work Node defers with `process.nextTick(() => this.emit(...))` is queued as
//!   one of three small natives kept in [`Node`] ([`Slot::EmitLater`],
//!   [`Slot::AfterDestroy`], [`Slot::CloseLater`]) with the target as an argument,
//!   so no closure holds a handle.
//! - `http.Server` is a subclass of [`Server`]; the base constructor sees the
//!   same arguments as the derived one, so it checks whether it builds an HTTP
//!   server and, if so, takes the second argument as the `request` listener.

use super::{arg, coded_error, f, method, p, Node, Slot};
use crate::host_services_trait::Host;
use crate::reactor_io_state::{Fault, Shared};
use crate::internal_natives;
use lunarstorm_engine_contract::{Call, ClassDecl, ClassSpec, ErrorKind, FnDecl, Item, Kind, ModuleDecl, Outcome, Scope, Sig, Ty, Vm};
use std::rc::Rc;

/// What chaining methods return.
const THIS: Ty = Ty::Named("this");
/// A callback with no arguments that matter.
const CALLBACK_SIG: Sig = Sig { params: &[], rest: Some(p("args", Ty::Any, "event arguments")), returns: Ty::Unknown, about: "A callback." };
/// An optional callback.
const CALLBACK: Ty = Ty::Optional(&Ty::Function(&CALLBACK_SIG));

const CONNECT: FnDecl = f(
    "connect",
    &[p("port", Ty::Any, "port, or `{ port, host }`"), p("host", Ty::Any, "host name, or the callback"), p("cb", CALLBACK, "called on connect")],
    None,
    THIS,
    "Starts an outbound connection.",
);
const WRITE: FnDecl = f(
    "write",
    &[p("data", Ty::Union(&[Ty::String, Ty::Bytes]), "bytes or UTF-8 text"), p("encoding", Ty::Any, "`utf8`, or the callback"), p("cb", CALLBACK, "called once the data is handed to the kernel")],
    None,
    Ty::Bool,
    "Sends data; false when the caller should wait for `drain`.",
);
const END: FnDecl = f("end", &[p("data", Ty::Optional(&Ty::Union(&[Ty::String, Ty::Bytes])), "final bytes")], None, THIS, "Sends optional final data, then closes the sending side.");
const DESTROY: FnDecl = f("destroy", &[p("error", Ty::Any, "emitted as `error` before `close`")], None, THIS, "Closes the socket at once, discarding unsent data.");

/// `net.Socket`.
pub(crate) const SOCKET: ClassDecl = ClassDecl {
    name: "Socket",
    extends: Some("EventEmitter"),
    constructor: Sig { params: &[p("options", Ty::Any, "`{ allowHalfOpen }`")], rest: None, returns: Ty::Named("Socket"), about: "An unconnected socket." },
    methods: &[CONNECT, WRITE, END, DESTROY],
    accessors: &[],
    statics: &[],
    about: "A TCP socket. Emits connect, data, end, drain, error and close.",
};

const LISTEN: FnDecl = f(
    "listen",
    &[p("port", Ty::Any, "port (0 picks a free one), or `{ port, host }`"), p("host", Ty::Any, "address, or the callback"), p("cb", CALLBACK, "called once listening")],
    None,
    THIS,
    "Binds and starts accepting; bind errors are emitted as `error`.",
);
const ADDRESS: FnDecl = f("address", &[], None, Ty::Any, "The bound `{ address, family, port }`, or null.");
const CLOSE: FnDecl = f("close", &[p("cb", CALLBACK, "called once closed")], None, THIS, "Stops accepting.");

/// `net.Server`.
pub(crate) const SERVER: ClassDecl = ClassDecl {
    name: "Server",
    extends: Some("EventEmitter"),
    constructor: Sig {
        params: &[p("options", Ty::Any, "`{ allowHalfOpen }`, or the connection listener"), p("listener", CALLBACK, "connection listener")],
        rest: None,
        returns: Ty::Named("Server"),
        about: "A server that is not listening yet.",
    },
    methods: &[LISTEN, ADDRESS, CLOSE],
    accessors: &[],
    statics: &[],
    about: "A TCP server. Emits listening, connection, error and close.",
};

const CREATE_SERVER: FnDecl = f(
    "createServer",
    &[p("options", Ty::Any, "options, or the connection listener"), p("listener", CALLBACK, "connection listener")],
    None,
    Ty::Named("Server"),
    "Creates a TCP server.",
);
const CONNECT_FN: FnDecl = f(
    "connect",
    &[p("port", Ty::Any, "port, or `{ port, host }`"), p("host", Ty::Any, "host name, or the callback"), p("cb", CALLBACK, "called on connect")],
    None,
    Ty::Named("Socket"),
    "Opens a TCP connection.",
);
const CREATE_CONNECTION: FnDecl = FnDecl { name: "createConnection", sig: CONNECT_FN.sig };

/// # Returns
/// The declaration of `node:net`.
pub(crate) fn declare() -> ModuleDecl {
    ModuleDecl {
        specifier: "node:net",
        items: vec![Item::Class(SOCKET), Item::Class(SERVER), Item::Function(CREATE_SERVER), Item::Function(CONNECT_FN), Item::Function(CREATE_CONNECTION)],
        about: "TCP sockets and servers.",
    }
}

/// # Arguments
/// * `scope` - the context handle.
/// * `fault` - an I/O failure.
///
/// # Returns
/// It as an `Error` with its `code`.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when allocation fails.
pub(crate) fn fault_error<'s, V: Vm>(scope: &V::Scope<'s>, fault: &Fault) -> Outcome<V::Value<'s>> {
    coded_error::<V>(scope, &fault.message, Some(fault.code))
}

/// # Arguments
/// * `scope` - the context handle.
/// * `object` - the instance.
/// * `key` - the property.
///
/// # Returns
/// The property as a number; NaN when it is not one.
///
/// # Errors
/// What a getter throws.
pub(crate) fn number<'s, V: Vm>(scope: &V::Scope<'s>, object: &V::Value<'s>, key: &str) -> Outcome<f64> {
    Ok(scope.as_number(&scope.get(object, key)?).unwrap_or(f64::NAN))
}

/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `object` - the instance.
/// * `key` - the property.
///
/// # Returns
/// Whether the property is truthy.
///
/// # Errors
/// What a getter throws.
pub(crate) fn flag<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, object: &V::Value<'s>, key: &str) -> Outcome<bool> {
    Ok(node.truthy(scope, &scope.get(object, key)?))
}

/// The instance's reactor slab key (`_k`).
///
/// # Arguments
/// * `scope` - the context handle.
/// * `this` - the socket or server.
///
/// # Returns
/// The key; `None` when not attached (-1).
///
/// # Errors
/// What a getter throws.
pub(crate) fn key<'s, V: Vm>(scope: &V::Scope<'s>, this: &V::Value<'s>) -> Outcome<Option<usize>> {
    let key = number::<V>(scope, this, "_k")?;
    Ok((key >= 0.0).then_some(key as usize))
}

/// Queues `function(...args)` on the `nextTick` queue.
///
/// # Arguments
/// * `node` - the kept builtins and queues.
/// * `shared` - loop state (the wake-up flag).
/// * `scope` - the context handle.
/// * `function` - the callback.
/// * `args` - its arguments.
pub(crate) fn later<'s, V: Vm, H: Host>(node: &Node<V>, shared: &Shared<H>, scope: &V::Scope<'s>, function: &V::Value<'s>, args: &[V::Value<'s>]) {
    node.timers.next_tick(scope, &shared.tick, function, args);
}

/// Queues `target.emit(name, ...args)` on the `nextTick` queue.
///
/// # Arguments
/// * `node` - the kept builtins and queues.
/// * `shared` - loop state.
/// * `scope` - the context handle.
/// * `target` - the emitter.
/// * `name` - the event.
/// * `args` - the event's arguments.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when allocation fails.
pub(crate) fn emit_later<'s, V: Vm, H: Host>(
    node: &Node<V>,
    shared: &Shared<H>,
    scope: &V::Scope<'s>,
    target: &V::Value<'s>,
    name: &str,
    args: &[V::Value<'s>],
) -> Outcome<()> {
    let mut all = vec![target.clone(), scope.string(name)?];
    all.extend_from_slice(args);
    later(node, shared, scope, &node.get(scope, Slot::EmitLater), &all);
    Ok(())
}

/// Builds the deferred-emit natives ([`Slot::EmitLater`], [`Slot::AfterDestroy`],
/// [`Slot::CloseLater`]) and keeps them.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `node` - where they are kept.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when allocation fails.
fn deferred<'s, V: Vm>(scope: &V::Scope<'s>, node: &Rc<Node<V>>) -> Outcome<()> {
    // `(target, name, ...args)`: `target.emit(name, ...args)`.
    let emit_later = scope.function("emitLater", move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        let target = arg::<V>(scope, args, 0);
        method::<V>(scope, &target, "emit", args.get(1..).unwrap_or(&[]))?;
        Ok(scope.undefined())
    })?;
    node.keep(scope, Slot::EmitLater, &emit_later);
    // `(socket, error)`: `error` when there is one, then `close(hadError)`.
    let n = node.clone();
    let after_destroy = scope.function("afterDestroy", move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        let (target, error) = (arg::<V>(scope, args, 0), arg::<V>(scope, args, 1));
        let had = n.truthy(scope, &error);
        if had {
            method::<V>(scope, &target, "emit", &[scope.string("error")?, error])?;
        }
        method::<V>(scope, &target, "emit", &[scope.string("close")?, scope.boolean(had)])?;
        Ok(scope.undefined())
    })?;
    node.keep(scope, Slot::AfterDestroy, &after_destroy);
    // `(server, cb)`: `close`, then the callback.
    let close_later = scope.function("closeLater", move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        let (target, cb) = (arg::<V>(scope, args, 0), arg::<V>(scope, args, 1));
        method::<V>(scope, &target, "emit", &[scope.string("close")?])?;
        if scope.kind(&cb) == Kind::Function {
            scope.call(&cb, &[])?;
        }
        Ok(scope.undefined())
    })?;
    node.keep(scope, Slot::CloseLater, &close_later);
    Ok(())
}

/// `socket.write(data, encoding?, cb?)`.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `shared` - loop state.
/// * `scope` - the context handle.
/// * `this` - the socket.
/// * `args` - the call's arguments.
///
/// # Returns
/// Whether the kernel took the bytes (false: wait for `drain`).
///
/// # Errors
/// `Unsupported encoding`, or what the engine throws lending the data.
pub(crate) fn write<'s, V: Vm, H: Host>(node: &Node<V>, shared: &Shared<H>, scope: &V::Scope<'s>, this: &V::Value<'s>, args: &[V::Value<'s>]) -> Outcome<bool> {
    let data = arg::<V>(scope, args, 0);
    let (encoding, mut cb) = (arg::<V>(scope, args, 1), arg::<V>(scope, args, 2));
    if scope.kind(&encoding) == Kind::Function {
        cb = encoding;
    } else {
        super::buffer_and_text_codecs::utf8_only(node, scope, &encoding, ErrorKind::Type)?;
    }
    if flag(node, scope, this, "destroyed")? || flag(node, scope, this, "_ended")? {
        let error = coded_error::<V>(scope, "write after end", Some("ERR_STREAM_WRITE_AFTER_END"))?;
        emit_later(node, shared, scope, this, "error", &[error])?;
        return Ok(false);
    }
    if flag(node, scope, this, "connecting")? {
        let mut queue = scope.get(this, "_queue")?;
        if scope.kind(&queue) != Kind::Object {
            queue = scope.array(&[])?;
            scope.set(this, "_queue", queue.clone())?;
        }
        method::<V>(scope, &queue, "push", &[scope.array(&[data, cb])?])?;
        return Ok(true);
    }
    let Some(key) = key::<V>(scope, this)? else { return Ok(false) };
    let ok = scope.with_bytes(&data, |bytes| shared.io.borrow_mut().tcp_write(key, bytes))?;
    if scope.kind(&cb) == Kind::Function {
        if ok {
            later(node, shared, scope, &cb, &[]);
        } else {
            method::<V>(scope, this, "once", &[scope.string("drain")?, cb])?;
        }
    }
    Ok(ok)
}

/// `socket.end(data?)`.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `shared` - loop state.
/// * `scope` - the context handle.
/// * `this` - the socket.
/// * `data` - final bytes; `undefined` for none.
///
/// # Errors
/// What `write` throws.
pub(crate) fn end<'s, V: Vm, H: Host>(node: &Node<V>, shared: &Shared<H>, scope: &V::Scope<'s>, this: &V::Value<'s>, data: V::Value<'s>) -> Outcome<()> {
    if flag(node, scope, this, "_ended")? || flag(node, scope, this, "destroyed")? {
        return Ok(());
    }
    if scope.kind(&data) != Kind::Undefined {
        method::<V>(scope, this, "write", &[data])?;
    }
    scope.set(this, "_ended", scope.boolean(true))?;
    if !flag(node, scope, this, "connecting")? {
        if let Some(key) = key::<V>(scope, this)? {
            shared.io.borrow_mut().tcp_end(key);
        }
    }
    Ok(())
}

/// Sets up a fresh socket's own properties, as the glue's constructor did.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `this` - the socket.
/// * `options` - `{ allowHalfOpen }` (or the accepting server).
///
/// # Errors
/// What a setter throws.
fn socket_fields<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, this: &V::Value<'s>, options: &V::Value<'s>) -> Outcome<()> {
    scope.set(this, "_k", scope.int(-1))?;
    scope.set(this, "connecting", scope.boolean(false))?;
    scope.set(this, "destroyed", scope.boolean(false))?;
    let half = super::is_object::<V>(scope, options) && flag(node, scope, options, "allowHalfOpen")?;
    scope.set(this, "allowHalfOpen", scope.boolean(half))?;
    scope.set(this, "remoteAddress", scope.undefined())?;
    scope.set(this, "remotePort", scope.undefined())?;
    scope.set(this, "_queue", scope.undefined())?;
    scope.set(this, "_ended", scope.boolean(false))?;
    Ok(())
}

/// Builds `Socket`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state.
/// * `node` - the kept builtins; the class is kept as [`Slot::Socket`].
///
/// # Returns
/// The class.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
fn socket<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>) -> Outcome<V::Value<'s>> {
    let (s, n) = (shared.clone(), node.clone());
    let connect = scope.function(CONNECT.name, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        let (mut port, mut host, mut cb) = (arg::<V>(scope, args, 0), arg::<V>(scope, args, 1), arg::<V>(scope, args, 2));
        if scope.kind(&port) == Kind::Object {
            cb = host;
            host = scope.get(&port, "host")?;
            port = scope.get(&port, "port")?;
        }
        if scope.kind(&host) == Kind::Function {
            cb = host;
            host = scope.undefined();
        }
        let address = if n.truthy(scope, &host) { n.text(scope, &host)? } else { String::from("127.0.0.1") };
        let port = super::process_and_globals::to_int32(scope.as_number(&n.call(scope, Slot::Number, &[port])?).unwrap_or(0.0));
        scope.set(this, "remoteAddress", scope.string(&address)?)?;
        scope.set(this, "remotePort", scope.int(port))?;
        scope.set(this, "connecting", scope.boolean(true))?;
        let key = internal_natives::connect(&s, &address, port as u16).map_err(|fault| internal_natives::throw_fault::<V>(scope, fault))?;
        scope.set(this, "_k", scope.number(key as f64))?;
        n.set_handle(scope, key, Some(this));
        if scope.kind(&cb) == Kind::Function {
            method::<V>(scope, this, "once", &[scope.string("connect")?, cb])?;
        }
        Ok(this.clone())
    })?;
    let (s, n) = (shared.clone(), node.clone());
    let write_fn = scope.function(WRITE.name, move |call: &Call<'_, V>| Ok(call.scope().boolean(write(&n, &s, call.scope(), call.this(), call.args())?)))?;
    let (s, n) = (shared.clone(), node.clone());
    let end_fn = scope.function(END.name, move |call: &Call<'_, V>| {
        let scope = call.scope();
        end(&n, &s, scope, call.this(), arg::<V>(scope, call.args(), 0))?;
        Ok(call.this().clone())
    })?;
    let (s, n) = (shared.clone(), node.clone());
    let destroy = scope.function(DESTROY.name, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        if flag(&n, scope, this, "destroyed")? {
            return Ok(this.clone());
        }
        scope.set(this, "destroyed", scope.boolean(true))?;
        scope.set(this, "connecting", scope.boolean(false))?;
        if let Some(key) = key::<V>(scope, this)? {
            s.io.borrow_mut().destroy(key);
            n.set_handle(scope, key, None);
            scope.set(this, "_k", scope.int(-1))?;
        }
        let error = arg::<V>(scope, call.args(), 0);
        later(&n, &s, scope, &n.get(scope, Slot::AfterDestroy), &[this.clone(), error]);
        Ok(this.clone())
    })?;
    let spec = ClassSpec::new(SOCKET.name)
        .extends(node.get(scope, Slot::EventEmitter))
        .method(CONNECT.name, connect)
        .method(WRITE.name, write_fn)
        .method(END.name, end_fn)
        .method(DESTROY.name, destroy);
    let n = node.clone();
    let class = scope.class::<(), _>(spec, move |call: &Call<'_, V>| {
        let scope = call.scope();
        socket_fields(&n, scope, call.this(), &arg::<V>(scope, call.args(), 0))
    })?;
    node.keep(scope, Slot::Socket, &class);
    node.keep(scope, Slot::SocketProto, &scope.get(&class, "prototype")?);
    Ok(class)
}

/// Builds `Server`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state.
/// * `node` - the kept builtins.
///
/// # Returns
/// The class.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
fn server<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>) -> Outcome<V::Value<'s>> {
    let (s, n) = (shared.clone(), node.clone());
    let listen = scope.function(LISTEN.name, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        let (mut port, mut host, mut cb) = (arg::<V>(scope, args, 0), arg::<V>(scope, args, 1), arg::<V>(scope, args, 2));
        if scope.kind(&port) == Kind::Object {
            if scope.kind(&host) == Kind::Function {
                cb = host;
            }
            host = scope.get(&port, "host")?;
            port = scope.get(&port, "port")?;
        }
        if scope.kind(&port) == Kind::Function {
            cb = port;
            port = scope.int(0);
        }
        if scope.kind(&host) == Kind::Function {
            cb = host;
            host = scope.undefined();
        }
        let port = if scope.kind(&port) == Kind::Undefined { 0.0 } else { scope.as_number(&n.call(scope, Slot::Number, &[port])?).unwrap_or(f64::NAN) };
        if !(port.is_finite() && port.fract() == 0.0 && (0.0..=65535.0).contains(&port)) {
            return Err(scope.throw_error(ErrorKind::Range, "Port should be >= 0 and < 65536"));
        }
        if flag(&n, scope, this, "listening")? {
            let error = coded_error::<V>(scope, "Listen method has been called more than once", Some("ERR_SERVER_ALREADY_LISTEN"))?;
            return Err(scope.throw(error));
        }
        if scope.kind(&cb) == Kind::Function {
            method::<V>(scope, this, "once", &[scope.string("listening")?, cb])?;
        }
        let address = if n.truthy(scope, &host) { n.text(scope, &host)? } else { String::from("0.0.0.0") };
        let http = flag(&n, scope, this, "_http")?;
        match internal_natives::listen(&s, &address, port as u16, http) {
            Ok((key, bound)) => {
                scope.set(this, "_k", scope.number(key as f64))?;
                scope.set(this, "_port", scope.number(f64::from(bound)))?;
                scope.set(this, "_host", scope.string(&address)?)?;
                scope.set(this, "listening", scope.boolean(true))?;
                n.set_handle(scope, key, Some(this));
                emit_later(&n, &s, scope, this, "listening", &[])?;
            }
            Err(fault) => {
                let error = fault_error::<V>(scope, &fault)?;
                emit_later(&n, &s, scope, this, "error", &[error])?;
            }
        }
        Ok(this.clone())
    })?;
    let n = node.clone();
    let address = scope.function(ADDRESS.name, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        if !flag(&n, scope, this, "listening")? {
            return Ok(scope.null());
        }
        let host = n.text(scope, &scope.get(this, "_host")?)?;
        let info = scope.object()?;
        scope.set(&info, "address", scope.string(&host)?)?;
        scope.set(&info, "family", scope.string(if host.contains(':') { "IPv6" } else { "IPv4" })?)?;
        scope.set(&info, "port", scope.get(this, "_port")?)?;
        Ok(info)
    })?;
    let (s, n) = (shared.clone(), node.clone());
    let close = scope.function(CLOSE.name, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        let cb = arg::<V>(scope, call.args(), 0);
        if !flag(&n, scope, this, "listening")? {
            if scope.kind(&cb) == Kind::Function {
                let error = coded_error::<V>(scope, "Server is not running", Some("ERR_SERVER_NOT_RUNNING"))?;
                later(&n, &s, scope, &cb, &[error]);
            }
            return Ok(this.clone());
        }
        if let Some(key) = key::<V>(scope, this)? {
            s.io.borrow_mut().listener_close(key);
            n.set_handle(scope, key, None);
        }
        scope.set(this, "_k", scope.int(-1))?;
        scope.set(this, "listening", scope.boolean(false))?;
        later(&n, &s, scope, &n.get(scope, Slot::CloseLater), &[this.clone(), cb]);
        Ok(this.clone())
    })?;
    let spec = ClassSpec::new(SERVER.name)
        .extends(node.get(scope, Slot::EventEmitter))
        .method(LISTEN.name, listen)
        .method(ADDRESS.name, address)
        .method(CLOSE.name, close);
    let n = node.clone();
    let class = scope.class::<(), _>(spec, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        let (mut options, mut listener) = (arg::<V>(scope, args, 0), arg::<V>(scope, args, 1));
        // An `http.Server` passes its own arguments here: the second is its
        // `request` listener, and it takes no TCP options (the glue's `super()`).
        let http = n.instance_of(scope, Slot::HttpServerProto, this)?;
        if scope.kind(&options) == Kind::Function {
            listener = options;
            options = scope.undefined();
        }
        scope.set(this, "_k", scope.int(-1))?;
        scope.set(this, "_http", scope.boolean(http))?;
        scope.set(this, "_host", scope.undefined())?;
        scope.set(this, "_port", scope.int(0))?;
        let half = !http && super::is_object::<V>(scope, &options) && flag(&n, scope, &options, "allowHalfOpen")?;
        scope.set(this, "allowHalfOpen", scope.boolean(half))?;
        scope.set(this, "listening", scope.boolean(false))?;
        if scope.kind(&listener) == Kind::Function {
            let event = if http { "request" } else { "connection" };
            method::<V>(scope, this, "on", &[scope.string(event)?, listener])?;
        }
        Ok(())
    })?;
    Ok(class)
}

/// Builds `node:net`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state: the reactor and the host's policy.
/// * `node` - the kept builtins and handles.
///
/// # Returns
/// The module object; `Server` is also returned for `node:http` to extend.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>) -> Outcome<(V::Value<'s>, V::Value<'s>)> {
    deferred(scope, node)?;
    let socket_class = socket(scope, shared, node)?;
    let server_class = server(scope, shared, node)?;
    let module = scope.object()?;
    scope.set(&module, SERVER.name, server_class.clone())?;
    scope.set(&module, SOCKET.name, socket_class)?;
    node.keep(scope, Slot::NetServer, &server_class);
    let n = node.clone();
    let create = scope.function(CREATE_SERVER.name, move |call: &Call<'_, V>| {
        let scope = call.scope();
        n.construct(scope, &n.get(scope, Slot::NetServer), &[arg::<V>(scope, call.args(), 0), arg::<V>(scope, call.args(), 1)])
    })?;
    scope.set(&module, CREATE_SERVER.name, create)?;
    let n = node.clone();
    let connect = scope.function(CONNECT_FN.name, move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        let port = arg::<V>(scope, args, 0);
        let options = if scope.kind(&port) == Kind::Object { port } else { scope.undefined() };
        let socket = n.construct(scope, &n.get(scope, Slot::Socket), &[options])?;
        method::<V>(scope, &socket, "connect", args)
    })?;
    scope.set(&module, CONNECT_FN.name, connect.clone())?;
    scope.set(&module, CREATE_CONNECTION.name, connect)?;
    Ok((module, server_class))
}
