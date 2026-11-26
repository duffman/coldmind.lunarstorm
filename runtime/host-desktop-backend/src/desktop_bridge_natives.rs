//! The natives the desktop backend registers (`send`, `native`, `error`,
//! `restore`, `hooks`), their declarations, and the `lunarstorm:desktop`
//! modules built on them.

use crate::*;

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

/// `desktopSend(json)`.
pub(crate) const SEND: Sig = sig(&[Param { name: "json", ty: Ty::String, about: "the envelope as JSON" }], Ty::Undefined, "Sends an envelope to the desktop process.");
/// `desktopNative(id, namespace, method, args)`.
pub(crate) const NATIVE: Sig = sig(
    &[
        Param { name: "id", ty: Ty::Number, about: "the call id the result settles" },
        Param { name: "namespace", ty: Ty::String, about: "the native API's namespace" },
        Param { name: "method", ty: Ty::String, about: "the method" },
        Param { name: "args", ty: Ty::String, about: "the arguments as a JSON array" },
    ],
    Ty::Undefined,
    "Starts a native call; the result arrives through the settle hook.",
);
/// `desktopError(text)`.
pub(crate) const ERROR: Sig = sig(&[Param { name: "text", ty: Ty::String, about: "the formatted error" }], Ty::Undefined, "Reports a handler error.");
/// `desktopRestore()`.
pub(crate) const RESTORE: Sig = sig(&[], Ty::String, "The control values carried over from the previous instance, as JSON.");
/// `desktopHooks(dispatch, settle)`. Only the hooks' callability is checked at
/// the call; their parameters are declared for `lunarstorm types`.
///
/// Fix: the hooks were declared without parameters, which typed them as `() =>
/// void`, a type the glue's real hooks (they take arguments) are not assignable
/// to. They now declare the arguments the host passes.
pub(crate) const HOOKS_SIG: Sig = sig(
    &[
        Param {
            name: "dispatch",
            ty: Ty::Function(&sig(&[Param { name: "json", ty: Ty::String, about: "" }], Ty::Undefined, "")),
            about: "delivers one envelope (JSON) from the desktop process",
        },
        Param {
            name: "settle",
            ty: Ty::Function(&sig(
                &[
                    Param { name: "id", ty: Ty::Number, about: "" },
                    Param { name: "ok", ty: Ty::Bool, about: "" },
                    Param { name: "json", ty: Ty::String, about: "" },
                ],
                Ty::Undefined,
                "",
            )),
            about: "settles a native call: (id, ok, json)",
        },
    ],
    Ty::Undefined,
    "Hands the host the glue's two hooks.",
);

/// The bindings of one backend instance: the natives behind the glue, the glue
/// and the two modules a backend imports.
///
/// | native                               | does                                   |
/// |--------------------------------------|----------------------------------------|
/// | `desktopSend(json)`                  | an envelope to the desktop process     |
/// | `desktopNative(id, ns, method, args)`| starts a native call                   |
/// | `desktopError(text)`                 | reports a handler error                |
/// | `desktopRestore()`                   | the carried-over control values (JSON) |
/// | `desktopHooks(dispatch, settle)`     | hands the host the glue's hooks        |
///
/// # Arguments
/// * `shell` - the desktop process.
/// * `epoch` - the instance's epoch.
/// * `restore` - the carried-over control values as JSON.
///
/// # Returns
/// The bindings to register before `start`.
pub(crate) fn bindings<S: Shell>(shell: &Arc<S>, epoch: u64, restore: String) -> Bindings<Engine> {
    let (send, native, error) = (shell.clone(), shell.clone(), shell.clone());
    Bindings::new()
        .function("desktopSend", SEND, move |call: &Call<'_, Engine>| {
            send.send(epoch, &text(call, 0)?);
            Ok(call.scope().undefined())
        })
        .function("desktopNative", NATIVE, move |call: &Call<'_, Engine>| {
            let scope = call.scope();
            let id = call.args().first().and_then(|id| scope.as_number(id)).unwrap_or(0.0) as u64;
            native.native(epoch, id, &text(call, 1)?, &text(call, 2)?, &text(call, 3)?);
            Ok(scope.undefined())
        })
        .function("desktopError", ERROR, move |call: &Call<'_, Engine>| {
            error.error(epoch, &text(call, 0)?);
            Ok(call.scope().undefined())
        })
        .function("desktopRestore", RESTORE, move |call: &Call<'_, Engine>| call.scope().string(&restore))
        .function("desktopHooks", HOOKS_SIG, |call: &Call<'_, Engine>| {
            let scope = call.scope();
            if let [dispatch, settle, ..] = call.args() {
                let hooks = Hooks { dispatch: scope.persist(dispatch), settle: scope.persist(settle) };
                HOOKS.with(|slot| *slot.borrow_mut() = Some(hooks));
            }
            Ok(scope.undefined())
        })
        .module(GLUE_MODULE, GLUE)
        .module("@desktop/bridge", BRIDGE_MODULE)
        .module("@desktop/api", API_MODULE)
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

/// Describes the pending exception of a failed hook call.
///
/// # Arguments
/// * `scope` - the context handle; its pending exception is taken.
///
/// # Returns
/// The stack or message of an error, or the thrown text.
pub(crate) fn thrown<'s>(scope: &<Engine as Vm>::Scope<'s>) -> String {
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

/// Calls one of the glue's hooks on the backend thread.
///
/// # Arguments
/// * `scope` - the context handle of the posted task.
/// * `shell` - receives the error if the hook throws.
/// * `epoch` - the instance's epoch.
/// * `call` - picks the hook and builds its arguments, then calls it.
pub(crate) fn hook<'s, S: Shell>(
    scope: &<Engine as Vm>::Scope<'s>,
    shell: &S,
    epoch: u64,
    call: impl FnOnce(&Hooks) -> Outcome<()>,
) {
    // The borrow is shared: the glue only replaces the hooks while a module loads.
    let outcome = HOOKS.with(|slot| slot.borrow().as_ref().map(call));
    if let Some(Err(Thrown)) = outcome {
        shell.error(epoch, &thrown(scope));
    }
}
