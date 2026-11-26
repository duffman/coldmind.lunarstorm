# Lunar extensions

Status: **design, not implemented.** This directory is the contract; the code follows it.
Spec context: `doc/plans/main.md` §2.6 (permissions), §6 (native desktop process), §6.1 (dev changes), §6.2 (backend reload).

An extension adds a **native namespace** to the desktop platform. The backend script calls it the way it
calls the built-ins:

```js
import { native } from "@desktop/api";
await native.store.set("theme", "dark");     // extension `store`, command `set`
```

An extension is a Rust crate (the native side) with an optional JS module (typed wrappers). It is compiled
into the platform binary; it is never loaded at run time.

## Documents

| file | covers |
|---|---|
| `README.md` | model, crate layout, the `Extension` trait, registration, naming, config |
| `commands.md` | how a call travels, threads, arguments and results, channels, the JS module |
| `permissions.md` | grants, scopes, the gate, errors |
| `lifecycle.md` | hooks, state, the Rust API extension trait, backend reload |
| `groups.md` | grouping extensions under one namespace: `native.<group>.<extension>.<command>` |

## What exists today

| piece | where | role for extensions |
|---|---|---|
| `native.<ns>.<method>(...)` | `platform/desktop/host/src/desktop_bridge_natives.rs`, a `Proxy` over any namespace and method | no change for a flat namespace: it already resolves. Groups need a deeper path (`groups.md`) |
| the gate | `HostConfig::allows_native` (`platform/desktop/app/src/manifest.rs`), called from `platform/desktop/app/src/runtime/lunarstorm.rs` before routing | decides `ns.method` or `ns.*` |
| the router | `NativeServices::call` (`platform/desktop/app/src/native.rs`), a `match call.ns` over the built-ins | gains an extension map after the built-ins |
| `NativeRouter::release_epoch` | `platform/desktop/app/src/runtime/mod.rs` | called when a backend is retired (§6.2) |
| `NativeCall`, `Completer` | `platform/desktop/app/src/runtime/mod.rs` | what a command receives and answers with |

## Crate layout

| crate | path | contents |
|---|---|---|
| `lunar-extension` | `platform/extension/` | `Extension`, `Group`, `Command`, `Thread`, `Context`, `NativeCall`, `Completer`, `Channel`, `Scope`, `AppEvent`, `schema`, and a `testing` module (`Harness`) |
| `lunarstorm-platform` | `platform/desktop/app/` | depends on `lunar-extension`; owns the registry and the router |
| `lunar-extension-<name>` | any | one extension; depends on `lunar-extension` only |

Decision: the trait lives in its own crate (self-contained parts are their own package), so an extension
does not build `tao`, `wry` or the runtime. `NativeCall` and `Completer` move there from
`platform/desktop/app/src/runtime/mod.rs`; `lunarstorm-platform` re-exports them.

## The `Extension` trait

```rust
pub trait Extension: Send + Sync + 'static {
    /// Its own name. The namespace is `native.<name>`, or `native.<group>.<name>` inside a group
    /// (`groups.md`); `Context::namespace()` returns the full one. See Naming.
    fn name(&self) -> &'static str;
    /// Every command, with the thread it runs on. A call to anything else is refused by the router.
    fn commands(&self) -> &'static [Command];
    /// Once per process, after the host configuration is read and before the first window.
    fn setup(&self, _cx: &Context, _config: serde_json::Value) -> Result<(), String> { Ok(()) }
    /// Runs one command, already on the thread its `Command` names. Answers through `call.done`.
    fn call(&self, cx: &Context, call: NativeCall);
    /// Source of the JS module served as `@extension/<name>`.
    fn guest(&self) -> Option<&'static str> { None }
    /// A form's window and WebView exist.
    fn on_window_ready(&self, _cx: &Context, _form: &str) {}
    /// A WebView is about to navigate; `false` cancels. It can only refuse more than the host does.
    fn on_navigation(&self, _cx: &Context, _form: &str, _url: &str) -> bool { true }
    /// A host event, observed. It cannot veto.
    fn on_event(&self, _cx: &Context, _event: &AppEvent) {}
    /// The backend of `epoch` is retired: drop everything it created.
    fn release_epoch(&self, _cx: &Context, _epoch: u64) {}
}

pub struct Command { pub name: &'static str, pub thread: Thread }
pub enum Thread { Ui, Pool, Inline }
```

There is no `on_drop`: the extension value is dropped when the process ends, so `Drop` is the hook.
Lifecycle detail is in `lifecycle.md`.

## Registration

```rust
lunarstorm_platform::Builder::new()
    .extension(lunar_extension_store::init())
    .group(lunar_extension_media::group())      // several extensions under one name, see groups.md
    .run()
```

- `Builder` is new; `src/bin/debug.rs` and `src/bin/release.rs` become `Builder::new().run()` with no
  extensions. An application with extensions is its own binary (`fn main` in its own crate).
- `lunarstorm-pack --runtime <binary>` already takes the host binary as input, so it packages that binary
  unchanged.
- Startup errors, reported before any window opens: a name used twice, a name that is a built-in
  namespace, an invalid name, a failing `setup` (`extension <name>: <error>`).

## Naming

- Namespace: `[a-z][a-z0-9]{0,31}`. Decision: no hyphens, because the namespace is a JS property
  (`native.store`); `native["my-store"]` is the case this rule removes.
- In a group the full namespace is `<group>.<name>`; the group name follows the same rule.
- Reserved (built-ins): `app window screen menu tray dialog path os clipboard notification shell process`.
- Crate: `lunar-extension-<name>`. JS module: `@extension/<name>`.

## Configuration

`zebra.json` gains an optional map; an extension receives its own entry in `setup`:

```json
{ "schema": 2, "extensions": { "store": { "file": "state.json" } } }
```

- `HOST_SCHEMA` goes from 1 to 2 when this lands; a host rejects a newer schema (existing check in
  `manifest.rs`).
- No entry means `setup` receives `null`.
- Changing `extensions` is a `dev.restart` (§6.1, same row as permissions): an extension's `setup` runs once and
  its state is not rebuilt in place.

## Decisions

- Compile-time only. A dynamic library breaks the sandbox and signing story, and Rust has no stable ABI.
- Built-ins first in the router: an extension cannot replace `process`, `window` or any other built-in.
- The built-in namespaces stay as they are. They can become extensions later with this same trait; nothing in
  this design requires it.
- Arguments and results stay JSON, as for the built-ins.
