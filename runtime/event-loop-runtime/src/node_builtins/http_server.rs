//! `node:http`: the server side, as native classes. The reactor parses requests
//! and frames responses; this module is the Node-shaped surface over it.
//!
//! Decisions:
//! - `IncomingMessage` arrives complete: the body is already buffered. `data` and
//!   `end` are emitted in a promise job once a listener for either is attached,
//!   and `for await` yields the body once.
//! - Header fields are parsed on the first read of `headers`, so a handler that
//!   never reads them pays nothing.
//! - `ServerResponse` keeps its fields in an own `_fields` object by lowercased
//!   name, as the glue did; framing fields (`content-length`,
//!   `transfer-encoding`), `date` and `connection` are written by the reactor.
//! - [`METHODS`] is the reactor's own table (`crate::http::METHODS` order): the
//!   reactor sends a known method as its index, so the glue's hand-copied list is
//!   gone.

use super::tcp_sockets_and_servers::{flag, key};
use super::{arg, coded_error, f, method, p, Node, Slot};
use crate::host_services_trait::Host;
use crate::reactor_io_state::Shared;
use lunarstorm_engine_contract::{Call, ClassDecl, ClassSpec, ErrorKind, Field, FnDecl, Item, Kind, ModuleDecl, Outcome, Scope, Sig, Ty, Vm};
use std::rc::Rc;

/// Request methods the reactor sends as an index. Index-aligned with the reactor's
/// method table.
pub(crate) const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"];

/// What chaining methods return.
const THIS: Ty = Ty::Named("this");
/// A header value.
const VALUE: Ty = Ty::Union(&[Ty::String, Ty::Number, Ty::Array(&Ty::String)]);
/// A body chunk.
const CHUNK: Ty = Ty::Union(&[Ty::String, Ty::Bytes]);

const ON: FnDecl = f("on", &[p("type", Ty::Any, "event name"), p("listener", Ty::Any, "listener")], None, THIS, "Adds a listener; the first `data` or `end` listener starts body delivery.");

/// `http.IncomingMessage`.
pub(crate) const INCOMING_MESSAGE: ClassDecl = ClassDecl {
    name: "IncomingMessage",
    extends: Some("EventEmitter"),
    constructor: Sig {
        params: &[
            p("method", Ty::String, "request method"),
            p("url", Ty::String, "request target"),
            p("raw", Ty::String, "raw header-field block"),
            p("body", Ty::Optional(&Ty::Bytes), "request body"),
        ],
        rest: None,
        returns: Ty::Named("IncomingMessage"),
        about: "A request as the reactor delivers it.",
    },
    methods: &[ON],
    accessors: &[Field { name: "headers", ty: Ty::Any, about: "Header fields by lowercased name." }],
    statics: &[],
    about: "An HTTP request; the body is already buffered.",
};

const SET_HEADER: FnDecl = f("setHeader", &[p("name", Ty::String, "field name"), p("value", VALUE, "field value")], None, THIS, "Sets a header field.");
const GET_HEADER: FnDecl = f("getHeader", &[p("name", Ty::String, "field name")], None, Ty::Any, "A header field's value.");
const HAS_HEADER: FnDecl = f("hasHeader", &[p("name", Ty::String, "field name")], None, Ty::Bool, "Whether a header field is set.");
const REMOVE_HEADER: FnDecl = f("removeHeader", &[p("name", Ty::String, "field name")], None, Ty::Undefined, "Removes a header field.");
const WRITE_HEAD: FnDecl = f(
    "writeHead",
    &[p("status", Ty::Number, "status code"), p("reason", Ty::Any, "reason phrase (ignored) or the fields"), p("fields", Ty::Any, "header fields")],
    None,
    THIS,
    "Sets the status and header fields; nothing is sent until write() or end().",
);
const WRITE: FnDecl = f("write", &[p("chunk", CHUNK, "body bytes")], None, Ty::Bool, "Streams a piece of the body; false after end().");
const END: FnDecl = f("end", &[p("chunk", Ty::Optional(&CHUNK), "final body bytes")], None, THIS, "Finishes the response.");

/// `http.ServerResponse`.
pub(crate) const SERVER_RESPONSE: ClassDecl = ClassDecl {
    name: "ServerResponse",
    extends: Some("EventEmitter"),
    constructor: Sig { params: &[p("key", Ty::Number, "the connection's reactor key")], rest: None, returns: Ty::Named("ServerResponse"), about: "A response." },
    methods: &[SET_HEADER, GET_HEADER, HAS_HEADER, REMOVE_HEADER, WRITE_HEAD, WRITE, END],
    accessors: &[],
    statics: &[],
    about: "An HTTP response; `end(body)` alone sends head and body in one write.",
};

/// `http.Server`.
pub(crate) const HTTP_SERVER: ClassDecl = ClassDecl {
    name: "Server",
    extends: Some("Server"),
    constructor: Sig {
        params: &[p("options", Ty::Any, "options (unused), or the request listener"), p("listener", Ty::Any, "request listener")],
        rest: None,
        returns: Ty::Named("Server"),
        about: "An HTTP server that is not listening yet.",
    },
    methods: &[],
    accessors: &[],
    statics: &[],
    about: "An HTTP server. Emits request, listening, error and close.",
};

const CREATE_SERVER: FnDecl = f(
    "createServer",
    &[p("options", Ty::Any, "options, or the request listener"), p("listener", Ty::Any, "request listener")],
    None,
    Ty::Named("Server"),
    "Creates an HTTP server.",
);

/// # Returns
/// The declaration of `node:http`.
pub(crate) fn declare() -> ModuleDecl {
    ModuleDecl {
        specifier: "node:http",
        items: vec![
            Item::Class(HTTP_SERVER),
            Item::Class(INCOMING_MESSAGE),
            Item::Class(SERVER_RESPONSE),
            Item::Value(Field { name: "METHODS", ty: Ty::Array(&Ty::String), about: "The methods the server parses." }),
            Item::Function(CREATE_SERVER),
        ],
        about: "HTTP/1.1 servers.",
    }
}

/// Parses a raw header-field block into Node's `headers` shape.
///
/// # Arguments
/// * `raw` - `name: value` lines.
///
/// # Returns
/// `(lowercased name, values)` in first-seen order. Repeated fields are joined
/// with `, ` (cookies with `; `); `set-cookie` keeps each value.
pub(crate) fn parse_headers(raw: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in raw.split('\n') {
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.is_empty() {
            continue;
        }
        let (name, value) = (name.to_lowercase(), value.trim().to_owned());
        match out.iter_mut().find(|(known, _)| *known == name) {
            Some((_, values)) if name == "set-cookie" => values.push(value),
            Some((_, values)) => {
                let joined = format!("{}{}{value}", values[0], if name == "cookie" { "; " } else { ", " });
                values[0] = joined;
            }
            None => out.push((name, vec![value])),
        }
    }
    out
}

/// Builds `IncomingMessage`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `node` - the kept builtins.
///
/// # Returns
/// The class.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
fn incoming<'s, V: Vm>(scope: &V::Scope<'s>, node: &Rc<Node<V>>) -> Outcome<V::Value<'s>> {
    let headers = scope.function("headers", move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        let known = scope.get(this, "_headers")?;
        if scope.kind(&known) == Kind::Object {
            return Ok(known);
        }
        let raw = scope.get(this, "_raw")?;
        let raw = if scope.kind(&raw) == Kind::String { scope.with_str(&raw, str::to_owned)? } else { String::new() };
        let parsed = scope.object()?;
        for (name, values) in parse_headers(&raw) {
            let value = if name == "set-cookie" {
                let items = values.iter().map(|value| scope.string(value)).collect::<Outcome<Vec<_>>>()?;
                scope.array(&items)?
            } else {
                scope.string(&values[0])?
            };
            scope.set(&parsed, &name, value)?;
        }
        scope.set(this, "_headers", parsed.clone())?;
        Ok(parsed)
    })?;
    // Delivers the buffered body: `data` (when there is one), then `end`.
    let n = node.clone();
    let deliver = scope.function("deliver", move |call: &Call<'_, V>| {
        let scope = call.scope();
        let request = arg::<V>(scope, call.args(), 0);
        let body = scope.get(&request, "_body")?;
        if scope.kind(&body) != Kind::Undefined {
            let chunk = super::buffer_and_text_codecs::as_buffer(&n, scope, body)?;
            super::event_emitter::fire(&n, scope, &request, &scope.string("data")?, &[chunk])?;
        }
        super::event_emitter::fire(&n, scope, &request, &scope.string("end")?, &[])?;
        Ok(scope.undefined())
    })?;
    node.keep(scope, Slot::BodyDeliver, &deliver);
    let n = node.clone();
    let on = scope.function(ON.name, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        let base = scope.get(&scope.get(&n.get(scope, Slot::EventEmitter), "prototype")?, "on")?;
        scope.call_with(&base, this, &[arg::<V>(scope, args, 0), arg::<V>(scope, args, 1)])?;
        let event = arg::<V>(scope, args, 0);
        let body_event = scope.kind(&event) == Kind::String && scope.with_str(&event, |name| name == "data" || name == "end")?;
        if body_event && !flag(&n, scope, this, "_flowing")? {
            scope.set(this, "_flowing", scope.boolean(true))?;
            let bound = scope.call_with(&n.get(scope, Slot::Bind), &n.get(scope, Slot::BodyDeliver), &[scope.undefined(), this.clone()])?;
            n.call(scope, Slot::QueueMicrotask, &[bound])?;
        }
        Ok(this.clone())
    })?;
    // `next` of the body iterator: the body once, then done.
    let n = node.clone();
    let next = scope.function("next", move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        let result = scope.object()?;
        let body = scope.get(this, "_body")?;
        if scope.kind(&body) == Kind::Undefined {
            scope.set(&result, "value", scope.undefined())?;
            scope.set(&result, "done", scope.boolean(true))?;
        } else {
            scope.set(this, "_body", scope.undefined())?;
            scope.set(&result, "value", super::buffer_and_text_codecs::as_buffer(&n, scope, body)?)?;
            scope.set(&result, "done", scope.boolean(false))?;
        }
        let (promise, resolver) = scope.promise()?;
        scope.settle(&resolver, Ok(result))?;
        Ok(promise)
    })?;
    node.keep(scope, Slot::BodyNext, &next);
    let n = node.clone();
    let iterate = scope.function("[Symbol.asyncIterator]", move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        scope.set(this, "_flowing", scope.boolean(true))?;
        let iterator = scope.object()?;
        scope.set(&iterator, "_body", scope.get(this, "_body")?)?;
        scope.set(&iterator, "next", n.get(scope, Slot::BodyNext))?;
        Ok(iterator)
    })?;
    let spec = ClassSpec::new(INCOMING_MESSAGE.name).extends(node.get(scope, Slot::EventEmitter)).method(ON.name, on).accessor("headers", Some(headers), None);
    let class = scope.class::<(), _>(spec, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        scope.set(this, "method", arg::<V>(scope, args, 0))?;
        scope.set(this, "url", arg::<V>(scope, args, 1))?;
        scope.set(this, "httpVersion", scope.string("1.1")?)?;
        scope.set(this, "complete", scope.boolean(true))?;
        scope.set(this, "_raw", arg::<V>(scope, args, 2))?;
        scope.set(this, "_body", arg::<V>(scope, args, 3))?;
        scope.set(this, "_headers", scope.undefined())?;
        scope.set(this, "_flowing", scope.boolean(false))?;
        Ok(())
    })?;
    let proto = scope.get(&class, "prototype")?;
    let descriptor = scope.object()?;
    scope.set(&descriptor, "value", iterate)?;
    scope.set(&descriptor, "writable", scope.boolean(true))?;
    scope.set(&descriptor, "configurable", scope.boolean(true))?;
    node.call(scope, Slot::DefineProperty, &[proto, node.get(scope, Slot::AsyncIterator), descriptor])?;
    node.keep(scope, Slot::IncomingMessage, &class);
    Ok(class)
}

/// The response's status and serialized fields, and marks the head sent.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `this` - the response.
///
/// # Returns
/// `(status, field lines, whether connection: close was asked for)`.
///
/// # Errors
/// What a getter or `String()` throws.
fn head<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, this: &V::Value<'s>) -> Outcome<(u16, String, bool)> {
    scope.set(this, "headersSent", scope.boolean(true))?;
    // Fix: `res.statusCode = '404'` sent status 0 (the old op read a string as 0)
    // and, once the ops were declared, threw a TypeError. Decision: convert as
    // Node does, with `Number()`; what is not a number in range sends 0 as before.
    let status = scope.as_number(&node.call(scope, Slot::Number, &[scope.get(this, "statusCode")?])?).unwrap_or(0.0);
    let status = if (0.0..=999.0).contains(&status) { status as u16 } else { 0 };
    let fields = scope.get(this, "_fields")?;
    let (mut lines, mut close) = (String::new(), false);
    if scope.kind(&fields) == Kind::Object {
        let names = node.call(scope, Slot::Keys, std::slice::from_ref(&fields))?;
        for name in node.items(scope, &names)? {
            let name = node.text(scope, &name)?;
            let value = scope.get(&fields, &name)?;
            match name.as_str() {
                "connection" => close = node.text(scope, &value)?.eq_ignore_ascii_case("close"),
                "content-length" | "transfer-encoding" | "date" => {}
                _ if scope.as_bool(&node.call(scope, Slot::IsArray, std::slice::from_ref(&value))?).unwrap_or(false) => {
                    for item in node.items(scope, &value)? {
                        lines.push_str(&format!("{name}: {}\r\n", node.text(scope, &item)?));
                    }
                }
                _ => lines.push_str(&format!("{name}: {}\r\n", node.text(scope, &value)?)),
            }
        }
    }
    Ok((status, lines, close))
}

/// Builds `ServerResponse`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state: the reactor and the host's clock.
/// * `node` - the kept builtins.
///
/// # Returns
/// The class.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
fn response<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>) -> Outcome<V::Value<'s>> {
    let n = node.clone();
    let set_header = scope.function(SET_HEADER.name, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        if flag(&n, scope, this, "headersSent")? {
            let error = coded_error::<V>(scope, "Cannot set headers after they are sent to the client", Some("ERR_HTTP_HEADERS_SENT"))?;
            return Err(scope.throw(error));
        }
        let (name, value) = (arg::<V>(scope, args, 0), arg::<V>(scope, args, 1));
        let name = n.text(scope, &name)?;
        let shown = if scope.as_bool(&n.call(scope, Slot::IsArray, std::slice::from_ref(&value))?).unwrap_or(false) {
            n.items(scope, &value)?.iter().map(|item| n.text(scope, item)).collect::<Outcome<Vec<_>>>()?.join("")
        } else {
            n.text(scope, &value)?
        };
        if name.contains(['\r', '\n']) || shown.contains(['\r', '\n']) {
            return Err(scope.throw_error(ErrorKind::Type, &format!("Invalid character in header \"{name}\"")));
        }
        let mut fields = scope.get(this, "_fields")?;
        if scope.kind(&fields) != Kind::Object {
            fields = scope.object()?;
            scope.set(this, "_fields", fields.clone())?;
        }
        scope.set(&fields, &name.to_lowercase(), value)?;
        Ok(this.clone())
    })?;
    let n = node.clone();
    let get_header = scope.function(GET_HEADER.name, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        let fields = scope.get(this, "_fields")?;
        if scope.kind(&fields) != Kind::Object {
            return Ok(fields);
        }
        let name = n.text(scope, &arg::<V>(scope, call.args(), 0))?;
        scope.get(&fields, &name.to_lowercase())
    })?;
    let has_header = scope.function(HAS_HEADER.name, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let value = method::<V>(scope, call.this(), GET_HEADER.name, call.args())?;
        Ok(scope.boolean(scope.kind(&value) != Kind::Undefined))
    })?;
    let n = node.clone();
    let remove_header = scope.function(REMOVE_HEADER.name, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        let fields = scope.get(this, "_fields")?;
        if scope.kind(&fields) == Kind::Object {
            let name = n.text(scope, &arg::<V>(scope, call.args(), 0))?;
            n.call(scope, Slot::DeleteProperty, &[fields, scope.string(&name.to_lowercase())?])?;
        }
        Ok(scope.undefined())
    })?;
    let n = node.clone();
    let write_head = scope.function(WRITE_HEAD.name, move |call: &Call<'_, V>| {
        let (scope, this, args) = (call.scope(), call.this(), call.args());
        let (reason, mut fields) = (arg::<V>(scope, args, 1), arg::<V>(scope, args, 2));
        // `typeof reason === 'object'`, which null passes too.
        if matches!(scope.kind(&reason), Kind::Object | Kind::Null) {
            fields = reason;
        }
        scope.set(this, "statusCode", arg::<V>(scope, args, 0))?;
        if n.truthy(scope, &fields) {
            let names = n.call(scope, Slot::Keys, std::slice::from_ref(&fields))?;
            for name in n.items(scope, &names)? {
                let key = n.text(scope, &name)?;
                method::<V>(scope, this, SET_HEADER.name, &[name, scope.get(&fields, &key)?])?;
            }
        }
        Ok(this.clone())
    })?;
    let (s, n) = (shared.clone(), node.clone());
    let write = scope.function(WRITE.name, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        if flag(&n, scope, this, "finished")? {
            return Ok(scope.boolean(false));
        }
        let key = key::<V>(scope, this)?.unwrap_or(usize::MAX);
        if !flag(&n, scope, this, "headersSent")? {
            let (status, lines, close) = head(&n, scope, this)?;
            s.io.borrow_mut().http_head(key, status, &lines, close, &s.host);
        }
        let chunk = arg::<V>(scope, call.args(), 0);
        scope.with_bytes(&chunk, |bytes| s.io.borrow_mut().http_chunk(key, bytes))?;
        Ok(scope.boolean(true))
    })?;
    let (s, n) = (shared.clone(), node.clone());
    let end = scope.function(END.name, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        if flag(&n, scope, this, "finished")? {
            return Ok(this.clone());
        }
        scope.set(this, "finished", scope.boolean(true))?;
        let key = key::<V>(scope, this)?.unwrap_or(usize::MAX);
        let chunk = arg::<V>(scope, call.args(), 0);
        if flag(&n, scope, this, "headersSent")? {
            if scope.kind(&chunk) != Kind::Undefined {
                scope.with_bytes(&chunk, |bytes| s.io.borrow_mut().http_chunk(key, bytes))?;
            }
            s.io.borrow_mut().http_end(key);
        } else {
            let (status, lines, close) = head(&n, scope, this)?;
            scope.with_bytes(&chunk, |body| s.io.borrow_mut().http_respond(key, status, &lines, body, close, &s.host))?;
        }
        Ok(this.clone())
    })?;
    let spec = ClassSpec::new(SERVER_RESPONSE.name)
        .extends(node.get(scope, Slot::EventEmitter))
        .method(SET_HEADER.name, set_header)
        .method(GET_HEADER.name, get_header)
        .method(HAS_HEADER.name, has_header)
        .method(REMOVE_HEADER.name, remove_header)
        .method(WRITE_HEAD.name, write_head)
        .method(WRITE.name, write)
        .method(END.name, end);
    let class = scope.class::<(), _>(spec, move |call: &Call<'_, V>| {
        let (scope, this) = (call.scope(), call.this());
        scope.set(this, "_k", arg::<V>(scope, call.args(), 0))?;
        scope.set(this, "_fields", scope.undefined())?;
        scope.set(this, "statusCode", scope.int(200))?;
        scope.set(this, "headersSent", scope.boolean(false))?;
        scope.set(this, "finished", scope.boolean(false))?;
        Ok(())
    })?;
    node.keep(scope, Slot::ServerResponse, &class);
    Ok(class)
}

/// Builds `node:http`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state.
/// * `node` - the kept builtins.
/// * `net_server` - `net.Server`, the base of `http.Server`.
///
/// # Returns
/// The module object.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>, net_server: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    let incoming = incoming(scope, node)?;
    let response = response(scope, shared, node)?;
    // The `net.Server` constructor does the work for both (see `net`).
    let server = scope.class::<(), _>(ClassSpec::new(HTTP_SERVER.name).extends(net_server.clone()), |_: &Call<'_, V>| Ok(()))?;
    node.keep(scope, Slot::HttpServerProto, &scope.get(&server, "prototype")?);
    node.keep(scope, Slot::HttpServer, &server);
    let module = scope.object()?;
    scope.set(&module, HTTP_SERVER.name, server)?;
    scope.set(&module, INCOMING_MESSAGE.name, incoming)?;
    scope.set(&module, SERVER_RESPONSE.name, response)?;
    let methods = METHODS.iter().map(|name| scope.string(name)).collect::<Outcome<Vec<_>>>()?;
    scope.set(&module, "METHODS", scope.array(&methods)?)?;
    let n = node.clone();
    let create = scope.function(CREATE_SERVER.name, move |call: &Call<'_, V>| {
        let scope = call.scope();
        n.construct(scope, &n.get(scope, Slot::HttpServer), &[arg::<V>(scope, call.args(), 0), arg::<V>(scope, call.args(), 1)])
    })?;
    scope.set(&module, CREATE_SERVER.name, create)?;
    Ok(module)
}
