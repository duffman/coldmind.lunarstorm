//! The hooks the loop and the loader call: `dispatch` (one native entry for
//! every loop event, by the codes in [`crate::runtime_public_types::ev`]),
//! `invoke` (a host call's export), `describe`, `exitCode`, and the builtin
//! registry parts `modules`, `internal` and `exportsOf`.

use crate::host_services_trait::Host;
use crate::reactor_io_state::Shared;
use lunarstorm_engine_contract::{Kind, Outcome, Scope, Vm};
use std::rc::Rc;
use super::*;

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
pub(crate) fn hooks<'s, V: Vm, H: Host>(
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
        scope.string(&console_and_value_inspector::inspect(&n, scope, &arg::<V>(scope, call.args(), 0))?)
    })?;
    scope.set(native, "describe", describe)?;
    let n = node.clone();
    let exit_code = scope.function("exitCode", move |call: &lunarstorm_engine_contract::Call<'_, V>| {
        let scope = call.scope();
        let code = scope.get(&n.get(scope, Slot::Process), "exitCode")?;
        let code = scope.as_number(&n.call(scope, Slot::Number, &[code])?).unwrap_or(0.0);
        Ok(scope.int(process_and_globals::to_int32(code)))
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
pub(crate) fn plain<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, value: &V::Value<'s>) -> crate::runtime_public_types::Arg {
    use crate::runtime_public_types::Arg;
    match scope.kind(value) {
        Kind::Undefined | Kind::Null => Arg::Null,
        Kind::Bool | Kind::Number | Kind::String | Kind::Bytes => crate::internal_natives::neutral::<V>(scope, value),
        _ => {
            let json = node.call(scope, Slot::Stringify, std::slice::from_ref(value));
            match json {
                Ok(json) if scope.kind(&json) == Kind::String => crate::internal_natives::neutral::<V>(scope, &json),
                Ok(_) => Arg::Null,
                // A cycle or a throwing `toJSON`: the inspector's text instead.
                Err(_) => console_and_value_inspector::inspect(node, scope, value).map(Arg::Text).unwrap_or(Arg::Null),
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
pub(crate) fn invoke<'s, V: Vm, H: Host>(node: &Rc<Node<V>>, shared: &Rc<Shared<H>>, scope: &V::Scope<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
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
        let text = console_and_value_inspector::inspect(&n, scope, &arg::<V>(scope, call.args(), 0)).unwrap_or_default();
        s.calls.borrow_mut().push((id, Err(text)));
        Ok(scope.undefined())
    })?;
    method::<V>(scope, &promise, "then", &[fulfilled, rejected])?;
    Ok(scope.undefined())
}

/// The single entry point for loop events (D3). Codes are
/// [`crate::runtime_public_types::ev`].
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
pub(crate) fn dispatch<'s, V: Vm>(node: &Rc<Node<V>>, scope: &V::Scope<'s>, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
    use crate::runtime_public_types::ev;
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
                Some(index) => scope.string(http_server::METHODS.get(index as usize).copied().unwrap_or(""))?,
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
                Some(server) => event_emitter::fire(node, scope, &server, &event("request")?, &[request, response.clone()])?,
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
                let chunk = buffer_and_text_codecs::as_buffer(node, scope, a())?;
                event_emitter::fire(node, scope, &socket, &event("data")?, &[chunk])?;
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
            event_emitter::fire(node, scope, &server, &event("connection")?, &[socket])?;
        }
        ev::END => {
            let Some(socket) = handle() else { return Ok(scope.undefined()) };
            event_emitter::fire(node, scope, &socket, &event("end")?, &[])?;
            if !tcp_sockets_and_servers::flag(node, scope, &socket, "allowHalfOpen")? {
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
            event_emitter::fire(node, scope, &socket, &event("close")?, &[scope.boolean(had)])?;
        }
        ev::DRAIN => {
            if let Some(socket) = handle() {
                event_emitter::fire(node, scope, &socket, &event("drain")?, &[])?;
            }
        }
        ev::CONNECT => {
            let Some(socket) = handle() else { return Ok(scope.undefined()) };
            scope.set(&socket, "connecting", scope.boolean(false))?;
            let queue = scope.get(&socket, "_queue")?;
            scope.set(&socket, "_queue", scope.undefined())?;
            // Replay what was written while connecting, then the deferred end().
            let ended = tcp_sockets_and_servers::flag(node, scope, &socket, "_ended")?;
            scope.set(&socket, "_ended", scope.boolean(false))?;
            if scope.kind(&queue) == Kind::Object {
                for entry in node.items(scope, &queue)? {
                    method::<V>(scope, &socket, "write", &[node.at(scope, &entry, 0)?, node.at(scope, &entry, 1)?])?;
                }
            }
            if ended {
                method::<V>(scope, &socket, "end", &[])?;
            }
            event_emitter::fire(node, scope, &socket, &event("connect")?, &[])?;
        }
        ev::FS => {
            let message = arg::<V>(scope, args, 3);
            let result = if scope.kind(&message) == Kind::Undefined {
                Ok(a())
            } else {
                Err((node.text(scope, &message)?, node.text(scope, &arg::<V>(scope, args, 4))?))
            };
            fs_module::done(node, scope, id as i32, result)?;
        }
        _ => {}
    }
    Ok(scope.undefined())
}
