//! `node:process` (the `process` object, an `EventEmitter`) and the `global`,
//! `crypto` and `performance` globals.
//!
//! Decisions:
//! - `process` is an instance of the native `EventEmitter` with plain
//!   properties, in the glue's key order, so `node:process`'s named exports
//!   (its own keys) are unchanged.
//! - `process.env` is copied from the host on first read: a getter replaces
//!   itself with the object, as the glue's did; most scripts never read it.
//! - `crypto` and `performance` are installed only when the engine has none.

use super::{arg, f, p, Node, Slot};
use crate::bindings::declared;
use crate::host::Host;
use crate::io::Shared;
use crate::ops::{arch, platform, print, ENTROPY_MAX};
use crate::node::timers::NEXT_TICK;
use lunarstorm_engine_contract::{Call, ErrorKind, Field, FnDecl, Item, Kind, ModuleDecl, Outcome, Scope, Ty, Vm};
use std::rc::Rc;

const CWD: FnDecl = f("cwd", &[], None, Ty::String, "The host's working directory.");
const EXIT: FnDecl = f(
    "exit",
    &[p("code", Ty::Any, "exit status, as `code | 0`; defaults to process.exitCode, then 0")],
    None,
    Ty::Named("never"),
    "Ends the process now.",
);
const UPTIME: FnDecl = f("uptime", &[], None, Ty::Number, "Seconds since the process started.");
const BIGINT: FnDecl = f("bigint", &[], None, Ty::Named("bigint"), "Nanoseconds since the process started.");
const WRITE: FnDecl = f("write", &[p("text", Ty::Any, "converted with String()")], None, Ty::Bool, "Writes text; always true.");
const NOW: FnDecl = f("now", &[], None, Ty::Number, "Milliseconds since the process started, from the host's monotonic clock.");
const GET_RANDOM_VALUES: FnDecl = f(
    "getRandomValues",
    &[p("array", Ty::Object(&[]), "an integer typed array of at most 65536 bytes")],
    None,
    Ty::Named("ArrayBufferView"),
    "Fills a typed array with random bytes from the host's entropy source.",
);

/// A writable stream's members.
const STREAM: [Field; 1] = [Field { name: "write", ty: Ty::Function(&WRITE.sig), about: "Writes text; always true." }];
/// `process.hrtime`'s members.
const HRTIME: [Field; 1] = [Field { name: "bigint", ty: Ty::Function(&BIGINT.sig), about: "Nanoseconds since the process started." }];

/// The members of `process`, in its key order (the `EventEmitter` methods come
/// from its class).
const MEMBERS: [Item; 16] = [
    Item::Value(Field { name: "argv", ty: Ty::Array(&Ty::String), about: "The program's arguments, from the host." }),
    Item::Value(Field { name: "pid", ty: Ty::Number, about: "The process id." }),
    Item::Value(Field { name: "platform", ty: Ty::String, about: "The OS, spelled as Node spells it." }),
    Item::Value(Field { name: "arch", ty: Ty::String, about: "The CPU, spelled as Node spells it." }),
    Item::Value(Field { name: "version", ty: Ty::String, about: "'v' and the Runtime's version." }),
    Item::Value(Field { name: "versions", ty: Ty::Object(&[Field { name: "lunarstorm", ty: Ty::String, about: "The Runtime's version." }]), about: "Component versions." }),
    Item::Value(Field { name: "exitCode", ty: Ty::Optional(&Ty::Any), about: "The exit status when the loop ends, as `exitCode | 0`." }),
    Item::Function(CWD),
    Item::Function(NEXT_TICK),
    Item::Function(EXIT),
    Item::Function(UPTIME),
    Item::Value(Field { name: "hrtime", ty: Ty::Object(&HRTIME), about: "High-resolution time." }),
    Item::Value(Field { name: "stdout", ty: Ty::Object(&STREAM), about: "Standard output." }),
    Item::Value(Field { name: "stderr", ty: Ty::Object(&STREAM), about: "Standard error." }),
    Item::Value(Field { name: "env", ty: Ty::Object(&[]), about: "The environment, copied from the host on first read." }),
    Item::Class(super::events::EVENT_EMITTER),
];

/// # Returns
/// The declaration of `node:process`; its last item is the class `process` is
/// an instance of.
pub(crate) fn declare() -> ModuleDecl {
    ModuleDecl { specifier: "node:process", items: MEMBERS.to_vec(), about: "The current process, an EventEmitter." }
}

/// # Returns
/// The names this module puts on the global object.
pub(crate) fn globals() -> Vec<Item> {
    vec![
        Item::Value(Field { name: "process", ty: Ty::Named("typeof import('node:process')"), about: "The current process." }),
        Item::Value(Field { name: "global", ty: Ty::Named("typeof globalThis"), about: "The global object." }),
        Item::Value(Field { name: "crypto", ty: Ty::Object(&[Field { name: "getRandomValues", ty: Ty::Function(&GET_RANDOM_VALUES.sig), about: "Random bytes." }]), about: "Web Crypto's random source." }),
        Item::Value(Field { name: "performance", ty: Ty::Object(&[Field { name: "now", ty: Ty::Function(&NOW.sig), about: "Milliseconds since start." }]), about: "High-resolution time." }),
    ]
}

/// `x | 0` of a number: JS's ToInt32.
///
/// # Arguments
/// * `number` - the number.
///
/// # Returns
/// It truncated and wrapped to 32 bits; NaN and infinities give 0.
pub(crate) fn to_int32(number: f64) -> i32 {
    if !number.is_finite() {
        return 0;
    }
    (number.trunc().rem_euclid(4_294_967_296.0) as u32) as i32
}

/// # Arguments
/// * `shared` - loop state.
///
/// # Returns
/// Milliseconds since the Runtime started, from the host's monotonic clock.
fn now<H: Host>(shared: &Shared<H>) -> f64 {
    shared.host.monotonic().duration_since(shared.start).as_secs_f64() * 1000.0
}

/// Builds a `{ write }` stream object.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state: the output streams.
/// * `node` - the kept builtins.
/// * `fd` - 1 for stdout, 2 for stderr.
///
/// # Returns
/// The stream object.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
fn stream<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>, fd: i32) -> Outcome<V::Value<'s>> {
    let (s, n) = (shared.clone(), node.clone());
    let write = declared(scope, WRITE.name, WRITE.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let text = n.text(scope, &call.args()[0])?;
        print(&s, fd, text.as_bytes());
        Ok(scope.boolean(true))
    })?;
    let stream = scope.object()?;
    scope.set(&stream, WRITE.name, write)?;
    Ok(stream)
}

/// Builds `process` and the `global`, `crypto` and `performance` globals.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state and host.
/// * `node` - the kept builtins; `process` is kept.
/// * `native` - the object the glue receives; its `nextTick` becomes `process.nextTick`.
///
/// # Returns
/// `process`, which is also the `node:process` module object.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>, native: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    let global = scope.global();
    let process = node.construct(scope, &node.get(scope, Slot::EventEmitter), &[])?;
    node.keep(scope, Slot::Process, &process);

    let argv = shared.host.args().iter().map(|arg| scope.string(arg)).collect::<Outcome<Vec<_>>>()?;
    scope.set(&process, "argv", scope.array(&argv)?)?;
    scope.set(&process, "pid", scope.number(f64::from(std::process::id())))?;
    scope.set(&process, "platform", scope.string(platform())?)?;
    scope.set(&process, "arch", scope.string(arch())?)?;
    let version = env!("CARGO_PKG_VERSION");
    scope.set(&process, "version", scope.string(&format!("v{version}"))?)?;
    let versions = scope.object()?;
    scope.set(&versions, "lunarstorm", scope.string(version)?)?;
    scope.set(&process, "versions", versions)?;
    scope.set(&process, "exitCode", scope.undefined())?;

    let s = shared.clone();
    let cwd = declared(scope, CWD.name, CWD.sig, move |call: &Call<'_, V>| call.scope().string(&s.host.working_directory()))?;
    scope.set(&process, CWD.name, cwd)?;
    scope.set(&process, NEXT_TICK.name, scope.get(native, NEXT_TICK.name)?)?;

    let (s, n) = (shared.clone(), node.clone());
    let exit = declared(scope, EXIT.name, EXIT.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let mut code = arg::<V>(scope, call.args(), 0);
        if scope.kind(&code) == Kind::Undefined {
            code = scope.get(&n.get(scope, Slot::Process), "exitCode")?;
        }
        let code = match scope.as_number(&code) {
            Some(code) => code,
            None => scope.as_number(&n.call(scope, Slot::Number, &[code])?).unwrap_or(0.0),
        };
        s.io.borrow_mut().flush_stdout(&s.host);
        s.host.exit(to_int32(code))
    })?;
    scope.set(&process, EXIT.name, exit)?;

    let s = shared.clone();
    let uptime = declared(scope, UPTIME.name, UPTIME.sig, move |call: &Call<'_, V>| Ok(call.scope().number(now(&s) / 1000.0)))?;
    scope.set(&process, UPTIME.name, uptime)?;

    let (s, n) = (shared.clone(), node.clone());
    let bigint = declared(scope, BIGINT.name, BIGINT.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        // `BigInt(Math.round(now * 1e6))`; the clock never runs backwards, so
        // rounding half up is `floor(x + 0.5)`.
        let nanos = (now(&s) * 1e6 + 0.5).floor();
        n.call(scope, Slot::BigInt, &[scope.number(nanos)])
    })?;
    let hrtime = scope.object()?;
    scope.set(&hrtime, BIGINT.name, bigint)?;
    scope.set(&process, "hrtime", hrtime)?;
    scope.set(&process, "stdout", stream(scope, shared, node, 1)?)?;
    scope.set(&process, "stderr", stream(scope, shared, node, 2)?)?;

    // The environment, copied from the host on first read; the getter replaces
    // itself with a plain data property holding the copy.
    let (s, n) = (shared.clone(), node.clone());
    let env_getter = scope.function("env", move |call: &Call<'_, V>| {
        let scope = call.scope();
        let env = scope.object()?;
        for (name, value) in s.host.environment() {
            scope.set(&env, &name, scope.string(&value)?)?;
        }
        let descriptor = scope.object()?;
        scope.set(&descriptor, "value", env.clone())?;
        scope.set(&descriptor, "writable", scope.boolean(true))?;
        scope.set(&descriptor, "enumerable", scope.boolean(true))?;
        n.call(scope, Slot::DefineProperty, &[n.get(scope, Slot::Process), scope.string("env")?, descriptor])?;
        Ok(env)
    })?;
    let descriptor = scope.object()?;
    scope.set(&descriptor, "enumerable", scope.boolean(true))?;
    scope.set(&descriptor, "configurable", scope.boolean(true))?;
    scope.set(&descriptor, "get", env_getter)?;
    node.call(scope, Slot::DefineProperty, &[process.clone(), scope.string("env")?, descriptor])?;

    scope.set(&global, "process", process.clone())?;

    // `typeof x !== 'object'`: `null` counts as an object, as in the glue.
    let missing = |name: &str| -> Outcome<bool> { Ok(!matches!(scope.kind(&scope.get(&global, name)?), Kind::Object | Kind::Bytes | Kind::Null)) };
    if missing("crypto")? {
        let (s, n) = (shared.clone(), node.clone());
        let random = declared(scope, GET_RANDOM_VALUES.name, GET_RANDOM_VALUES.sig, move |call: &Call<'_, V>| {
            let scope = call.scope();
            let array = call.args()[0].clone();
            let parts = [scope.get(&array, "buffer")?, scope.get(&array, "byteOffset")?, scope.get(&array, "byteLength")?];
            let view = n.construct(scope, &n.get(scope, Slot::Uint8Array), &parts)?;
            let length = n.length(scope, &view)?;
            if length > ENTROPY_MAX {
                return Err(scope.throw_error(ErrorKind::Range, "getRandomValues: at most 65536 bytes per call"));
            }
            let mut bytes = vec![0u8; length];
            s.host.entropy(&mut bytes);
            scope.call_with(&scope.get(&view, "set")?, &view, &[scope.bytes(bytes)?])?;
            Ok(array)
        })?;
        let crypto = scope.object()?;
        scope.set(&crypto, GET_RANDOM_VALUES.name, random)?;
        scope.set(&global, "crypto", crypto)?;
    }
    scope.set(&global, "global", global.clone())?;
    if missing("performance")? {
        let s = shared.clone();
        let now_fn = declared(scope, NOW.name, NOW.sig, move |call: &Call<'_, V>| Ok(call.scope().number(now(&s))))?;
        let performance = scope.object()?;
        scope.set(&performance, NOW.name, now_fn)?;
        scope.set(&global, "performance", performance)?;
    }
    Ok(process)
}
