//! Node's builtin modules and globals, as natives.
//!
//! One file per module (D4): each has `declare()`, the declaration its exports
//! are installed from, and `install(...)`, which builds the module's object.
//! [`install`] runs them at `start`, after the `ops` object and the host's
//! bindings, and returns the result: `{ modules, ...internals }`.
//!
//! Decisions:
//! - [`install`] returns the object the loop and the loader read (D3): the
//!   hooks `dispatch`, `ticks`, `describe`, `exitCode` and `invoke`, and the
//!   registry parts `modules`, `internal` and `exportsOf`. It replaces what the
//!   removed JS glue returned; no script runs at `start`. Scripts never see that object.
//! - Values the natives keep between calls (builtins such as `Map.prototype.get`,
//!   the `EventEmitter` class, `Buffer.prototype`, `process`) are persistent
//!   handles in [`Node`], which the Runtime owns and clears before the VM
//!   instance goes. Closures capture the `Rc<Node>`, never a handle of their own:
//!   a handle inside a GC-managed function is a root the collector cannot see
//!   through, so a cycle through it would never be freed.
//! - Builtins are captured once, at install, instead of looked up on the global
//!   object per call: a script that replaces `Object.keys` does not change how
//!   `console.log` prints.

pub(crate) mod buffer_and_text_codecs;
pub(crate) mod console_and_value_inspector;
pub(crate) mod event_emitter;
pub(crate) mod fs_module;
pub(crate) mod http_server;
pub(crate) mod tcp_sockets_and_servers;
pub(crate) mod posix_path;
pub(crate) mod process_and_globals;
pub(crate) mod timers_and_ticks;
pub(crate) mod kept_intrinsics;
pub(crate) mod loop_event_dispatch;

pub(crate) use kept_intrinsics::{Node, Slot, INTRINSICS};

use crate::host_services_trait::Host;
use crate::reactor_io_state::Shared;
use lunarstorm_engine_contract::{FnDecl, Kind, ModuleDecl, Outcome, Param, Scope, Sig, Ty, Vm};
use std::rc::Rc;

/// The specifier of the declarations of names installed on the global object
/// (`console`, `TextEncoder`, `crypto`); not importable.
pub const GLOBAL: &str = "globalThis";

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
    global.items.extend(timers_and_ticks::globals());
    global.items.extend(buffer_and_text_codecs::globals());
    global.items.extend(console_and_value_inspector::globals());
    global.items.extend(process_and_globals::globals());
    let mut modules = vec![posix_path::declare(), event_emitter::declare(), timers_and_ticks::declare(), buffer_and_text_codecs::declare(), process_and_globals::declare()];
    modules.extend(fs_module::declare());
    modules.push(tcp_sockets_and_servers::declare());
    modules.push(http_server::declare());
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
/// The object the builtins share: `modules` (the builtin registry so far) and the
/// helpers the other builtins use.
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
    scope.set(&modules, "path", posix_path::install::<V, H>(scope, shared)?)?;
    scope.set(&modules, "events", event_emitter::install(scope, node, &native)?)?;
    scope.set(&modules, "timers", timers_and_ticks::install(scope, shared, node, &native)?)?;
    scope.set(&modules, "buffer", buffer_and_text_codecs::install(scope, node, &native)?)?;
    console_and_value_inspector::install(scope, shared, node, ops, &native)?;
    scope.set(&modules, "process", process_and_globals::install(scope, shared, node, &native)?)?;
    let (fs, promises) = fs_module::install(scope, shared, node)?;
    scope.set(&modules, "fs", fs)?;
    scope.set(&modules, "fs/promises", promises)?;
    node.keep(scope, Slot::QueueMicrotask, &scope.get(&global, "queueMicrotask")?);
    let (net, net_server) = tcp_sockets_and_servers::install(scope, shared, node)?;
    scope.set(&modules, "net", net)?;
    scope.set(&modules, "http", http_server::install(scope, shared, node, &net_server)?)?;
    loop_event_dispatch::hooks(scope, shared, node, ops, &native, &modules)?;
    Ok(native)
}

// ---------------------------------------------------------------- helpers

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

/// Builds an `Error` carrying a Node-style `code`, as Node does.
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
