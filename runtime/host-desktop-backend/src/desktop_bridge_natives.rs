//! The two modules a desktop backend imports, as natives: `@desktop/bridge`
//! (`setProperty`, `serve`, `restored`) and `@desktop/api` (`native.<ns>.<method>`),
//! plus the two entry points the backend thread calls into them: [`dispatch`]
//! (an envelope from the desktop process) and [`settle`] (a native call finished).
//!
//! Decisions:
//! - Replaces the JS glue `js/desktop.js` (archived). No script runs for the
//!   desktop modules; the state the glue kept in closures is [`Desktop`], one per
//!   backend thread, in [`DESKTOP`].
//! - Property writes are collected per form and control and sent by a microtask
//!   queued at the first write. Everything a handler writes before it returns or
//!   awaits goes out as one envelope per form, and a write made from a timer or
//!   after an `await` is sent without the host having to ask for a flush.
//! - Closures given to the engine capture only plain Rust data (names, ids, the
//!   epoch). Engine handles live in [`DESKTOP`] only: a handle captured by a
//!   GC-managed function is a root the collector cannot see through.
//! - The `RefCell` of [`DESKTOP`] is borrowed only for short reads and writes and
//!   never while JS runs, because a handler may call back into the bridge.
//! - JSON crosses through the engine's own `JSON.parse`/`JSON.stringify`, captured
//!   at start, so the envelope text is exactly what the glue produced and this
//!   crate needs no JSON library.

use crate::*;
use lunarstorm_event_loop_runtime::declared;
use lunarstorm_event_loop_runtime::vm::{ErrorKind, Field, FnDecl, Item, Kind, ModuleDecl};
use std::collections::{HashMap, HashSet};

/// An engine value.
type Value<'s> = <Engine as Vm>::Value<'s>;
/// An engine handle that outlives a scope.
type Handle = <Engine as Vm>::Persistent;
/// The engine's scope.
type EngineScope<'s> = <Engine as Vm>::Scope<'s>;

/// Reads a string argument of a native function.
///
/// # Arguments
/// * `call` - the call.
/// * `index` - the argument's position.
///
/// # Returns
/// The text; empty when the argument is missing.
///
/// # Errors
/// The engine's exception when the argument is not convertible to a string.
pub(crate) fn text(call: &Call<'_, Engine>, index: usize) -> Outcome<String> {
    match call.args().get(index) {
        Some(value) => call.scope().with_str(value, str::to_owned),
        None => Ok(String::new()),
    }
}

/// Shorthand for a [`Sig`] of the backend natives: no rest parameter.
///
/// # Arguments
/// * `params` - the parameters.
/// * `returns` - the result.
/// * `about` - one line.
///
/// # Returns
/// The signature.
pub(crate) const fn sig(params: &'static [Param], returns: Ty, about: &'static str) -> Sig {
    Sig { params, rest: None, returns, about }
}

/// Specifier of the bridge module.
const BRIDGE_MODULE: &str = "@desktop/bridge";
/// Specifier of the native API module.
const API_MODULE: &str = "@desktop/api";

/// `setProperty(form, control, prop, value)`.
const SET_PROPERTY: FnDecl = FnDecl {
    name: "setProperty",
    sig: sig(
        &[
            Param { name: "form", ty: Ty::String, about: "the form's name" },
            Param { name: "control", ty: Ty::String, about: "the control's name" },
            Param { name: "prop", ty: Ty::String, about: "the property" },
            Param { name: "value", ty: Ty::Unknown, about: "the new value" },
        ],
        Ty::Undefined,
        "Writes one control property; it reaches the form's WebView once the running code returns or awaits.",
    ),
};
/// `serve(handler)`.
const SERVE: FnDecl = FnDecl {
    name: "serve",
    sig: sig(
        &[Param {
            name: "handler",
            ty: Ty::Function(&sig(
                &[Param {
                    name: "message",
                    ty: Ty::Object(&[
                        Field { name: "form", ty: Ty::String, about: "the form" },
                        Field { name: "target", ty: Ty::String, about: "the control" },
                        Field { name: "event", ty: Ty::String, about: "the event" },
                        Field { name: "payload", ty: Ty::Unknown, about: "the event's data" },
                        Field { name: "request", ty: Ty::Bool, about: "whether an answer is awaited" },
                    ]),
                    about: "",
                }],
                Ty::Unknown,
                "",
            )),
            about: "the form runtime's dispatch; its result (or promise) answers a request",
        }],
        Ty::Undefined,
        "Installs the one message handler.",
    ),
};
/// `restored(form)`.
const RESTORED: FnDecl = FnDecl {
    name: "restored",
    sig: sig(
        &[Param { name: "form", ty: Ty::String, about: "the form's name" }],
        Ty::Object(&[]),
        "Control values carried over from the previous backend instance (spec 6.2): control -> props.",
    ),
};

/// # Returns
/// The declaration of `@desktop/bridge`.
fn bridge_decl() -> ModuleDecl {
    ModuleDecl {
        specifier: BRIDGE_MODULE,
        items: vec![
            Item::Function(SET_PROPERTY),
            Item::Function(SERVE),
            Item::Function(RESTORED),
            Item::Value(Field { name: "default", ty: Ty::Object(&[]), about: "the bridge: setProperty, serve, restored" }),
        ],
        about: "The form runtime's link to the desktop process.",
    }
}

/// # Returns
/// The declaration of `@desktop/api`.
fn api_decl() -> ModuleDecl {
    ModuleDecl {
        specifier: API_MODULE,
        items: vec![
            Item::Value(Field { name: "native", ty: Ty::Any, about: "native.<namespace>.<method>(...args) -> Promise" }),
            Item::Value(Field { name: "default", ty: Ty::Any, about: "the same `native`" }),
        ],
        about: "The desktop process's native services.",
    }
}

/// The desktop modules' state of the instance running on this thread.
pub(crate) struct Desktop {
    /// The desktop process.
    shell: Arc<dyn Shell>,
    /// The instance's epoch.
    epoch: u64,
    /// `JSON.parse`.
    parse: Handle,
    /// `JSON.stringify`.
    stringify: Handle,
    /// `String`, for `String(value)`.
    string: Handle,
    /// `Object.freeze`.
    freeze: Handle,
    /// `Reflect.construct`.
    construct: Handle,
    /// `Proxy`.
    proxy: Handle,
    /// `Promise`.
    promise: Handle,
    /// `Promise.resolve`.
    resolve: Handle,
    /// `Error.prototype`.
    error_prototype: Handle,
    /// `Object.prototype.isPrototypeOf`.
    is_prototype_of: Handle,
    /// `queueMicrotask`.
    queue_microtask: Handle,
    /// The native function that sends the outbox.
    flush: Handle,
    /// Carried-over control values: form -> control -> props.
    restore: Handle,
    /// The bridge object, once a module built it.
    bridge: Option<Handle>,
    /// The `native` proxy, once a module built it.
    native: Option<Handle>,
    /// The form runtime's dispatch, once `serve` was called.
    handler: Option<Handle>,
    /// Native calls in flight: id -> resolver.
    pending: HashMap<u64, Handle>,
    /// Property writes not sent yet: form -> `{control: {prop: value}}`, in first-write order.
    outbox: Vec<(String, Handle)>,
    /// Request ids (as `f64` bits) the desktop process cancelled.
    cancelled: HashSet<u64>,
    /// Whether a flush is already queued.
    flushing: bool,
    /// Id of the last native call.
    last_call: u64,
}

thread_local! {
    /// The desktop modules' state of the backend thread's current instance.
    ///
    /// Decision: a thread-local, because the tasks that reach it are posted from
    /// other threads and so cannot own engine handles, yet always run on the one
    /// backend thread. It is cleared ([`clear`]) before the instance is destroyed.
    static DESKTOP: RefCell<Option<Desktop>> = const { RefCell::new(None) };
}

/// Drops the current instance's state; its handles must go before the instance.
pub(crate) fn clear() {
    DESKTOP.with(|slot| slot.borrow_mut().take());
}

/// Runs `f` on the state, borrowed for the call only.
///
/// # Arguments
/// * `f` - reads or changes the state; must not call into JS.
///
/// # Returns
/// `f`'s result; `None` when no instance runs on this thread.
fn with<R>(f: impl FnOnce(&mut Desktop) -> R) -> Option<R> {
    DESKTOP.with(|slot| slot.borrow_mut().as_mut().map(f))
}

/// Reads one handle of the state as a value of this scope.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `pick` - selects the handle.
///
/// # Returns
/// The value.
///
/// # Errors
/// An `Error` when no instance runs on this thread.
fn local<'s>(scope: &EngineScope<'s>, pick: impl FnOnce(&Desktop) -> &Handle) -> Outcome<Value<'s>> {
    with(|desktop| scope.local(pick(desktop))).ok_or_else(|| scope.throw_error(ErrorKind::Error, "the desktop backend is not running"))
}

/// `JSON.parse(text)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `text` - JSON text.
///
/// # Returns
/// The value.
///
/// # Errors
/// The `SyntaxError` `JSON.parse` throws.
fn parse<'s>(scope: &EngineScope<'s>, text: &str) -> Outcome<Value<'s>> {
    let parse = local(scope, |d| &d.parse)?;
    scope.call(&parse, &[scope.string(text)?])
}

/// `JSON.stringify(value)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - the value.
///
/// # Returns
/// The JSON text; `undefined` for a value JSON cannot express.
///
/// # Errors
/// What `JSON.stringify` throws (a cycle, a `BigInt`).
fn stringify<'s>(scope: &EngineScope<'s>, value: &Value<'s>) -> Outcome<String> {
    let stringify = local(scope, |d| &d.stringify)?;
    let text = scope.call(&stringify, std::slice::from_ref(value))?;
    scope.with_str(&text, str::to_owned)
}

/// `String(value)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - any value, symbols included.
///
/// # Returns
/// The text.
///
/// # Errors
/// What a `toString` of the value throws.
fn to_text<'s>(scope: &EngineScope<'s>, value: &Value<'s>) -> Outcome<String> {
    let string = local(scope, |d| &d.string)?;
    let text = scope.call(&string, std::slice::from_ref(value))?;
    scope.with_str(&text, str::to_owned)
}

/// Describes a thrown value for the error report, as the glue's `format` did.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - what was thrown.
///
/// # Returns
/// `name: message\nstack` for an error, the text of a string, JSON otherwise.
///
/// Fix: the glue passed `JSON.stringify(undefined)` (which is `undefined`) to a
/// native declared to take a string, so reporting `throw undefined` itself threw.
/// A value JSON cannot express is now described with `String(value)`.
fn describe<'s>(scope: &EngineScope<'s>, value: &Value<'s>) -> String {
    let is_error = (|| {
        let check = local(scope, |d| &d.is_prototype_of)?;
        let prototype = local(scope, |d| &d.error_prototype)?;
        scope.call_with(&check, &prototype, std::slice::from_ref(value))
    })()
    .ok()
    .and_then(|answer| scope.as_bool(&answer))
    .unwrap_or(false);
    if is_error {
        let field = |key: &str| scope.get(value, key).and_then(|field| to_text(scope, &field)).unwrap_or_default();
        let stack = match scope.get(value, "stack") {
            Ok(stack) if !matches!(scope.kind(&stack), Kind::Undefined | Kind::Null) => to_text(scope, &stack).unwrap_or_default(),
            _ => String::new(),
        };
        return format!("{}: {}\n{stack}", field("name"), field("message"));
    }
    if scope.kind(value) == Kind::String {
        return scope.with_str(value, str::to_owned).unwrap_or_default();
    }
    let json = stringify(scope, value).ok().filter(|_| {
        // `JSON.stringify` answers `undefined` for undefined and functions.
        !matches!(scope.kind(value), Kind::Undefined | Kind::Function)
    });
    match json {
        Some(json) => json,
        None => {
            let _ = scope.exception();
            to_text(scope, value).unwrap_or_else(|_| String::from("<unprintable>"))
        }
    }
}

/// Reports an error from a handler or a rejected request to the shell.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `error` - what was thrown.
fn report<'s>(scope: &EngineScope<'s>, error: &Value<'s>) {
    let text = describe(scope, error);
    if let Some((shell, epoch)) = with(|d| (d.shell.clone(), d.epoch)) {
        shell.error(epoch, &text);
    }
}

/// Sends one envelope to the desktop process, which stamps version and epoch.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `envelope` - kind, windowId, method, payload and, for a response, id.
///
/// # Errors
/// What `JSON.stringify` throws for the payload.
fn send<'s>(scope: &EngineScope<'s>, envelope: &Value<'s>) -> Outcome<()> {
    let text = stringify(scope, envelope)?;
    if let Some((shell, epoch)) = with(|d| (d.shell.clone(), d.epoch)) {
        shell.send(epoch, &text);
    }
    Ok(())
}

/// Builds an envelope object.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `fields` - its properties, in order.
///
/// # Returns
/// The object.
///
/// # Errors
/// [`Thrown`] when allocation fails.
fn envelope<'s>(scope: &EngineScope<'s>, fields: &[(&str, Value<'s>)]) -> Outcome<Value<'s>> {
    let object = scope.object()?;
    for (key, value) in fields {
        scope.set(&object, key, value.clone())?;
    }
    Ok(object)
}

/// Sends the property writes collected since the last flush, one envelope per form.
///
/// # Arguments
/// * `scope` - the context handle.
///
/// # Errors
/// What `JSON.stringify` throws for a written value.
fn flush<'s>(scope: &EngineScope<'s>) -> Outcome<()> {
    let outbox = with(|d| {
        d.flushing = false;
        std::mem::take(&mut d.outbox)
    })
    .unwrap_or_default();
    for (form, controls) in outbox {
        let controls = scope.local(&controls);
        let payload = envelope(scope, &[("controls", controls)])?;
        let update = scope.string("update")?;
        let message = envelope(scope, &[("kind", update.clone()), ("windowId", scope.string(&form)?), ("method", update), ("payload", payload)])?;
        send(scope, &message)?;
    }
    Ok(())
}

/// The bridge's `setProperty`.
///
/// # Arguments
/// * `call` - `(form, control, prop, value)`, checked against [`SET_PROPERTY`].
///
/// # Returns
/// `undefined`.
///
/// # Errors
/// [`Thrown`] when allocation fails or no instance runs.
fn set_property<'a>(call: &Call<'a, Engine>) -> Outcome<Value<'a>> {
    let scope = call.scope();
    let (form, control, prop) = (text(call, 0)?, text(call, 1)?, text(call, 2)?);
    let value = call.args().get(3).cloned().unwrap_or_else(|| scope.undefined());
    let controls = match with(|d| d.outbox.iter().find(|(name, _)| *name == form).map(|(_, controls)| scope.local(controls))).flatten() {
        Some(controls) => controls,
        None => {
            let controls = scope.object()?;
            let handle = scope.persist(&controls);
            with(|d| d.outbox.push((form, handle)));
            controls
        }
    };
    let mut props = scope.get(&controls, &control)?;
    if scope.kind(&props) != Kind::Object {
        props = scope.object()?;
        scope.set(&controls, &control, props.clone())?;
    }
    scope.set(&props, &prop, value)?;
    if with(|d| !std::mem::replace(&mut d.flushing, true)).unwrap_or(false) {
        let queue = local(scope, |d| &d.queue_microtask)?;
        let flush = local(scope, |d| &d.flush)?;
        scope.call(&queue, &[flush])?;
    }
    Ok(scope.undefined())
}

/// The bridge's `serve`: keeps the one message handler.
///
/// # Arguments
/// * `call` - `(handler)`, checked against [`SERVE`].
///
/// # Returns
/// `undefined`.
fn serve<'a>(call: &Call<'a, Engine>) -> Outcome<Value<'a>> {
    let scope = call.scope();
    if let Some(handler) = call.args().first() {
        let handle = scope.persist(handler);
        with(|d| d.handler = Some(handle));
    }
    Ok(scope.undefined())
}

/// The bridge's `restored`.
///
/// # Arguments
/// * `call` - `(form)`, checked against [`RESTORED`].
///
/// # Returns
/// The form's carried-over control values; an empty object when there are none.
///
/// # Errors
/// [`Thrown`] when allocation fails or no instance runs.
fn restored<'a>(call: &Call<'a, Engine>) -> Outcome<Value<'a>> {
    let scope = call.scope();
    let restore = local(scope, |d| &d.restore)?;
    let values = scope.get(&restore, &text(call, 0)?)?;
    if matches!(scope.kind(&values), Kind::Undefined | Kind::Null) {
        return scope.object();
    }
    Ok(values)
}

/// The bridge object, built at the first import of `@desktop/bridge` and the
/// same for every later one.
///
/// # Arguments
/// * `scope` - the context handle.
///
/// # Returns
/// The frozen `{setProperty, serve, restored}`.
///
/// # Errors
/// [`Thrown`] when allocation fails or no instance runs.
fn bridge<'s>(scope: &EngineScope<'s>) -> Outcome<Value<'s>> {
    if let Some(bridge) = with(|d| d.bridge.as_ref().map(|bridge| scope.local(bridge))).flatten() {
        return Ok(bridge);
    }
    let object = scope.object()?;
    scope.set(&object, SET_PROPERTY.name, declared::<Engine, _>(scope, SET_PROPERTY.name, SET_PROPERTY.sig, set_property)?)?;
    scope.set(&object, SERVE.name, declared::<Engine, _>(scope, SERVE.name, SERVE.sig, serve)?)?;
    scope.set(&object, RESTORED.name, declared::<Engine, _>(scope, RESTORED.name, RESTORED.sig, restored)?)?;
    let freeze = local(scope, |d| &d.freeze)?;
    scope.call(&freeze, std::slice::from_ref(&object))?;
    let handle = scope.persist(&object);
    with(|d| d.bridge = Some(handle));
    Ok(object)
}

/// Calls `new Proxy({}, {get})`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `get` - the `get` trap: `(target, key)`.
///
/// # Returns
/// The proxy.
///
/// # Errors
/// [`Thrown`] when allocation fails or no instance runs.
fn proxy<'s>(scope: &EngineScope<'s>, get: Value<'s>) -> Outcome<Value<'s>> {
    let handler = envelope(scope, &[("get", get)])?;
    let target = scope.object()?;
    let construct = local(scope, |d| &d.construct)?;
    let proxy = local(scope, |d| &d.proxy)?;
    let arguments = scope.array(&[target, handler])?;
    scope.call(&construct, &[proxy, arguments])
}

/// Starts one native call: `native.<namespace>.<method>(...args)`.
///
/// # Arguments
/// * `call` - the call; its arguments travel as a JSON array.
/// * `namespace` - the service.
/// * `method` - the method.
///
/// # Returns
/// A promise that [`settle`] settles.
///
/// # Errors
/// [`Thrown`] when allocation fails or no instance runs. Arguments JSON cannot
/// carry reject the promise instead, as the glue's executor did.
fn native_call<'a>(call: &Call<'a, Engine>, namespace: &str, method: &str) -> Outcome<Value<'a>> {
    let scope = call.scope();
    let (promise, resolver) = Scope::promise(scope)?;
    let arguments = scope.array(call.args())?;
    match stringify(scope, &arguments) {
        Ok(json) => {
            let Some((shell, epoch, id)) = with(|d| {
                d.last_call += 1;
                d.pending.insert(d.last_call, resolver);
                (d.shell.clone(), d.epoch, d.last_call)
            }) else {
                return Err(scope.throw_error(ErrorKind::Error, "the desktop backend is not running"));
            };
            shell.native(epoch, id, namespace, method, &json);
        }
        Err(Thrown) => {
            let error = scope.exception();
            scope.settle(&resolver, Err(error))?;
        }
    }
    Ok(promise)
}

/// The `native` proxy, built at the first import of `@desktop/api`.
///
/// # Arguments
/// * `scope` - the context handle.
///
/// # Returns
/// `native`: any property is a namespace, any property of that a method.
///
/// # Errors
/// [`Thrown`] when allocation fails or no instance runs.
fn native<'s>(scope: &EngineScope<'s>) -> Outcome<Value<'s>> {
    if let Some(native) = with(|d| d.native.as_ref().map(|native| scope.local(native))).flatten() {
        return Ok(native);
    }
    let namespaces = scope.function("get", |call: &Call<'_, Engine>| {
        let scope = call.scope();
        let key = call.args().get(1).cloned().unwrap_or_else(|| scope.undefined());
        let namespace: Arc<str> = to_text(scope, &key)?.into();
        let methods = scope.function("get", move |call: &Call<'_, Engine>| {
            let scope = call.scope();
            let key = call.args().get(1).cloned().unwrap_or_else(|| scope.undefined());
            let method: Arc<str> = to_text(scope, &key)?.into();
            let namespace = namespace.clone();
            scope.function(&method.clone(), move |call: &Call<'_, Engine>| native_call(call, &namespace, &method))
        })?;
        proxy(scope, methods)
    })?;
    let native = proxy(scope, namespaces)?;
    let handle = scope.persist(&native);
    with(|d| d.native = Some(handle));
    Ok(native)
}

/// Answers a request, unless it was cancelled meanwhile.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `id` - the request's id.
/// * `window` - its `windowId`.
/// * `method` - its method.
/// * `payload` - the handler's answer; `undefined` and `null` answer `{}`.
///
/// # Errors
/// What `JSON.stringify` throws for the payload.
fn answer<'s>(scope: &EngineScope<'s>, id: f64, window: &str, method: &str, payload: Value<'s>) -> Outcome<()> {
    if with(|d| d.cancelled.remove(&id.to_bits())).unwrap_or(false) {
        return Ok(());
    }
    let payload = if matches!(scope.kind(&payload), Kind::Undefined | Kind::Null) { scope.object()? } else { payload };
    let message = envelope(
        scope,
        &[("kind", scope.string("response")?), ("id", scope.number(id)), ("windowId", scope.string(window)?), ("method", scope.string(method)?), ("payload", payload)],
    )?;
    send(scope, &message)
}

/// Delivers one envelope from the desktop process to the backend.
///
/// # Arguments
/// * `scope` - the context handle of the posted task.
/// * `json` - the envelope: `event`, `request` or `cancel`.
///
/// # Errors
/// The envelope is not JSON, or sending the answer throws. A throwing handler is
/// reported to the shell, not returned.
pub(crate) fn dispatch<'s>(scope: &EngineScope<'s>, json: &str) -> Outcome<()> {
    let envelope_value = parse(scope, json)?;
    let field = |key: &str| scope.get(&envelope_value, key);
    let kind = to_text(scope, &field("kind")?)?;
    let id = scope.as_number(&field("id")?).unwrap_or(f64::NAN);
    if kind == "cancel" {
        with(|d| d.cancelled.insert(id.to_bits()));
        return Ok(());
    }
    let window = field("windowId")?;
    let method = to_text(scope, &field("method")?)?;
    // As the glue's `indexOf`/`slice`: without a dot, the target is the method
    // less its last character and the event the whole method.
    let (target, event) = match method.find('.') {
        Some(dot) => (method[..dot].to_owned(), method[dot + 1..].to_owned()),
        None => {
            let mut target = method.clone();
            target.pop();
            (target, method.clone())
        }
    };
    let request = kind == "request";
    let message = envelope(
        scope,
        &[
            ("form", window.clone()),
            ("target", scope.string(&target)?),
            ("event", scope.string(&event)?),
            ("payload", field("payload")?),
            ("request", scope.boolean(request)),
        ],
    )?;
    let window: Arc<str> = to_text(scope, &window)?.into();
    let method: Arc<str> = method.into();
    let handler = with(|d| d.handler.as_ref().map(|handler| scope.local(handler))).flatten();
    let result = match handler {
        Some(handler) => scope.call(&handler, &[message]),
        None => Ok(scope.undefined()),
    };
    let result = match result {
        Ok(result) => result,
        Err(Thrown) => {
            let error = scope.exception();
            report(scope, &error);
            if request {
                answer(scope, id, &window, &method, scope.object()?)?;
            }
            return Ok(());
        }
    };
    if request {
        let (window_ok, method_ok) = (window.clone(), method.clone());
        let on_answer = scope.function("answer", move |call: &Call<'_, Engine>| {
            let scope = call.scope();
            let payload = call.args().first().cloned().unwrap_or_else(|| scope.undefined());
            answer(scope, id, &window_ok, &method_ok, payload)?;
            Ok(scope.undefined())
        })?;
        let on_error = scope.function("fail", move |call: &Call<'_, Engine>| {
            let scope = call.scope();
            let error = call.args().first().cloned().unwrap_or_else(|| scope.undefined());
            report(scope, &error);
            answer(scope, id, &window, &method, scope.object()?)?;
            Ok(scope.undefined())
        })?;
        let (promise, resolve) = (local(scope, |d| &d.promise)?, local(scope, |d| &d.resolve)?);
        let resolved = scope.call_with(&resolve, &promise, &[result])?;
        call_method(scope, &resolved, "then", &[on_answer, on_error])?;
    } else if matches!(scope.kind(&result), Kind::Object | Kind::Function) {
        let then = scope.get(&result, "then")?;
        if scope.kind(&then) == Kind::Function {
            let on_error = scope.function("report", |call: &Call<'_, Engine>| {
                let scope = call.scope();
                let error = call.args().first().cloned().unwrap_or_else(|| scope.undefined());
                report(scope, &error);
                Ok(scope.undefined())
            })?;
            scope.call_with(&then, &result, &[scope.undefined(), on_error])?;
        }
    }
    Ok(())
}

/// Calls `receiver[name](...args)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `receiver` - the object.
/// * `name` - the method's name.
/// * `args` - the arguments.
///
/// # Returns
/// The method's result.
///
/// # Errors
/// What the lookup or the method throws; a `TypeError` when it is not a function.
fn call_method<'s>(scope: &EngineScope<'s>, receiver: &Value<'s>, name: &str, args: &[Value<'s>]) -> Outcome<Value<'s>> {
    let function = scope.get(receiver, name)?;
    if scope.kind(&function) != Kind::Function {
        return Err(scope.throw_error(ErrorKind::Type, &format!("{name} is not a function")));
    }
    scope.call_with(&function, receiver, args)
}

/// Settles the promise of a native call. An unknown id (already settled, or of a
/// retired instance) is ignored.
///
/// # Arguments
/// * `scope` - the context handle of the posted task.
/// * `call` - the call's id.
/// * `result` - the value as JSON (empty for `undefined`), or the error message.
///
/// # Errors
/// [`Thrown`] when allocation fails.
///
/// Fix: the glue parsed a result that was not JSON inside its settle hook, which
/// threw and left the call's promise pending forever. Such a result now rejects
/// the promise with the `SyntaxError`.
pub(crate) fn settle<'s>(scope: &EngineScope<'s>, call: u64, result: Result<String, String>) -> Outcome<()> {
    let Some(resolver) = with(|d| d.pending.remove(&call)).flatten() else { return Ok(()) };
    let outcome = match result {
        Ok(json) if json.is_empty() => Ok(scope.undefined()),
        Ok(json) => parse(scope, &json).map_err(|Thrown| scope.exception()),
        Err(message) => Err(scope.error(ErrorKind::Error, &message)?),
    };
    scope.settle(&resolver, outcome)
}

/// Runs one entry from the backend thread into the desktop modules, reporting
/// an exception to the shell.
///
/// # Arguments
/// * `scope` - the context handle of the posted task.
/// * `shell` - receives the error.
/// * `epoch` - the instance's epoch.
/// * `entry` - [`dispatch`] or [`settle`], bound to its arguments.
pub(crate) fn hook<'s, S: Shell>(scope: &EngineScope<'s>, shell: &S, epoch: u64, entry: impl FnOnce() -> Outcome<()>) {
    if let Err(Thrown) = entry() {
        shell.error(epoch, &thrown(scope));
    }
}

/// The bindings of one backend instance: the state the desktop modules keep,
/// set up at `start`, and the two modules.
///
/// # Arguments
/// * `shell` - the desktop process.
/// * `epoch` - the instance's epoch.
/// * `restore` - the carried-over control values as JSON; empty for none.
///
/// # Returns
/// The bindings to register before `start`.
pub(crate) fn bindings<S: Shell>(shell: &Arc<S>, epoch: u64, restore: String) -> Bindings<Engine> {
    let shell: Arc<dyn Shell> = shell.clone();
    Bindings::new()
        .values(move |scope: &EngineScope<'_>, _ops: &Value<'_>| {
            let global = scope.global();
            let json = scope.get(&global, "JSON")?;
            let object = scope.get(&global, "Object")?;
            let reflect = scope.get(&global, "Reflect")?;
            let promise = scope.get(&global, "Promise")?;
            let error = scope.get(&global, "Error")?;
            let parse = scope.get(&json, "parse")?;
            let restore = scope.call(&parse, &[scope.string(if restore.is_empty() { "{}" } else { &restore })?])?;
            let flush = scope.function("flush", |call: &Call<'_, Engine>| {
                flush(call.scope())?;
                Ok(call.scope().undefined())
            })?;
            let state = Desktop {
                shell,
                epoch,
                parse: scope.persist(&parse),
                stringify: scope.persist(&scope.get(&json, "stringify")?),
                string: scope.persist(&scope.get(&global, "String")?),
                freeze: scope.persist(&scope.get(&object, "freeze")?),
                construct: scope.persist(&scope.get(&reflect, "construct")?),
                proxy: scope.persist(&scope.get(&global, "Proxy")?),
                resolve: scope.persist(&scope.get(&promise, "resolve")?),
                promise: scope.persist(&promise),
                error_prototype: scope.persist(&scope.get(&error, "prototype")?),
                is_prototype_of: scope.persist(&scope.get(&scope.get(&object, "prototype")?, "isPrototypeOf")?),
                queue_microtask: scope.persist(&scope.get(&global, "queueMicrotask")?),
                flush: scope.persist(&flush),
                restore: scope.persist(&restore),
                bridge: None,
                native: None,
                handler: None,
                pending: HashMap::new(),
                outbox: Vec::new(),
                cancelled: HashSet::new(),
                flushing: false,
                last_call: 0,
            };
            DESKTOP.with(|slot| *slot.borrow_mut() = Some(state));
            Ok(())
        })
        .native_module(bridge_decl(), |scope, exports| {
            let bridge = bridge(scope)?;
            for name in [SET_PROPERTY.name, SERVE.name, RESTORED.name] {
                exports.push(scope.get(&bridge, name)?);
            }
            exports.push(bridge);
            Ok(())
        })
        .native_module(api_decl(), |scope, exports| {
            let native = native(scope)?;
            exports.push(native.clone());
            exports.push(native);
            Ok(())
        })
}

/// A desktop process that ignores everything, for [`declarations`]: its Runtime
/// never starts, so none of these is called.
pub(crate) struct Silent;

impl Shell for Silent {
    /// Ignored.
    fn loaded(&self, _: u64, _: Option<&str>) {}
    /// Ignored.
    fn send(&self, _: u64, _: &str) {}
    /// Ignored.
    fn native(&self, _: u64, _: u64, _: &str, _: &str, _: &str) {}
    /// Ignored.
    fn error(&self, _: u64, _: &str) {}
    /// Ignored.
    fn log(&self, _: &str, _: &str) {}
    /// Ignored.
    fn retired(&self, _: u64) {}
}

/// Lists everything a backend on this host can reach, for `lunarstorm types`.
///
/// Decision: built the way [`start`] builds a backend's Runtime (same host type,
/// same [`bindings`]), over a [`Silent`] shell and without starting it, so the
/// declarations cannot drift from what a backend gets.
///
/// # Returns
/// The Runtime's merged declarations (`Runtime::declarations`).
///
/// # Errors
/// The event loop cannot be created, or the Runtime's own error.
pub fn declarations() -> Result<Vec<lunarstorm_event_loop_runtime::vm::ModuleDecl>, String> {
    let shell = Arc::new(Silent);
    let host = DesktopHost {
        shell: shell.clone(),
        epoch: 0,
        name: String::new(),
        roots: Vec::new(),
        environment: Vec::new(),
        loading: Cell::new(false),
        failure: RefCell::new(None),
    };
    let mut runtime = Runtime::<Engine, _>::new(host).map_err(|error| format!("cannot create the event loop: {error}"))?;
    runtime.register(bindings(&shell, 0, String::new()));
    runtime.declarations()
}

/// Describes the pending exception of a failed entry.
///
/// # Arguments
/// * `scope` - the context handle; its pending exception is taken.
///
/// # Returns
/// The stack or message of an error, or the thrown text.
pub(crate) fn thrown<'s>(scope: &EngineScope<'s>) -> String {
    let value = scope.exception();
    let field = |key: &str| scope.get(&value, key).ok().and_then(|field| scope.with_str(&field, str::to_owned).ok());
    let message = field("message").unwrap_or_default();
    let described = match field("stack") {
        Some(stack) if !stack.is_empty() => format!("{message}\n{stack}"),
        _ if !message.is_empty() => message,
        _ => scope.with_str(&value, str::to_owned).unwrap_or_else(|Thrown| String::from("<unprintable>")),
    };
    explain(&described)
}
