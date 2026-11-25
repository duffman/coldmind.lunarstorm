# Extensions and plugins

Status: **design.** The concepts below apply to every level of Lunarstorm; the platform level is specified
in detail in `platform/doc/lunar-extensions/`. Read `hosts.md` first.

## Concepts

| concept | what it is | example |
|---|---|---|
| **module** | a unit of code a script imports | `node:fs`, `lunarstorm:ipc`, `./util.js` |
| **core module** | a module the Runtime itself provides to every host that enables it | `node:*`, `lunarstorm:ipc`, `lunarstorm:worker` |
| **lunar-extension** | a named package of functionality a host adds to its programs: modules, native code, state | `ui` (terminal output), `store` (persistent key/value) |
| **plugin host** | a named slot an extension opens, with a contract other code implements | `store` opens the slot `backend` |
| **plugin** | an implementation of a plugin host's contract | `sqlite` and `file`, both implementing `backend` |
| **manifest** | the declaration of which extensions and plugins an application uses, and the permissions it holds | `"store": { "plugins": ["sqlite"] }` |
| **permission** | a rule in the manifest: what is active, which calls are granted, which data they may touch | `store.get`, a scope of keys |

The Runtime supplies the core modules. Each host supplies its own lunar-extensions, chosen for its
environment; plugins extend those extensions; the manifest selects and limits all of it.

```
host
 +-- Runtime: core modules
 +-- lunar-extension "store"
 |     +-- plugin host "backend"   (contract: trait Backend)
 |           +-- plugin "sqlite"
 |           +-- plugin "file"
 +-- lunar-extension "ui"
 +-- manifest: uses store + sqlite; grants store.get, store.set; scope keys "ui.*"
```

## One concept at every level

Extensions exist at two levels, with one shape:

| level | lives in | instance | a call is | example |
|---|---|---|---|---|
| **runtime** | inside a `Runtime`, added by its host | one per Runtime: the main program and each worker | a synchronous function call inside the VM | the CLI host's `ui`; the desktop host's `@desktop/api` |
| **platform** | in the desktop process (`lunarstorm-platform`) | one per process | an asynchronous `native.<ns>.<cmd>(...)` with JSON arguments, run on a declared thread | `window`, `dialog`, a third-party `store` |

What both levels share:

- the vocabulary: extension, plugin host, plugin, manifest, permission;
- the name rules: `[a-z][a-z0-9]{0,31}`, unique per host;
- the manifest entry and its checks;
- the permission model: uses, grants, scopes;
- the lifecycle: `setup` when the extension becomes active, release when its owner ends.

The shared part is one crate without a VM dependency (working name `lunar-extension`). Each level adds its
own trait on top: the runtime level is generic over `V: Vm` and registered statically, like every
Runtime/VM/Host call; the platform level is an object-safe trait registered when the process starts.

## A runtime-level extension

A runtime-level extension is built on the mechanism the core modules already use. `lunarstorm:ipc` is a
native module: on its first import the loader builds its exports as closures over this Runtime's state and
declares them with `Scope::native_module` (`src/modules.rs`). An extension does the same for its own
modules, and adds state, configuration and plugins:

```rust
pub trait Extension<V: Vm>: 'static {
    /// The extension's name: `[a-z][a-z0-9]{0,31}`.
    const NAME: &'static str;
    /// State for one Runtime; created by `setup`, dropped with the Runtime.
    type State: 'static;
    /// The module specifiers it serves, e.g. `lunarstorm:store`.
    fn modules(&self) -> &'static [&'static str];
    /// Called once per Runtime when the manifest uses the extension: reads its configuration and
    /// active plugins, builds the state.
    fn setup(&self, cx: &Setup<'_>) -> Result<Self::State, String>;
    /// Builds the exports of one of its modules, on that module's first import.
    fn exports<'s>(&self, state: &Rc<Self::State>, scope: &V::Scope<'s>, module: &str)
        -> Outcome<Vec<(&'static str, V::Value<'s>)>>;
}
```

- **Per Runtime.** `setup` runs in every Runtime that uses the extension, so a worker gets an instance of
  its own and state stays local to its VM.
- **On demand.** A module's exports are built on its first import. A program pays for the extensions it
  imports: the startup cost of the Runtime stays the cost of the core.
- **Static.** A host lists its extensions as a type (`(Ui, Store<(Sqlite, File)>)`), so registration and
  every call are resolved at compile time.

The CLI host's terminal support is the first candidate: today `bindings()` in `lunarstorm-host-cli`
registers the natives `color`, `tty`, `columns`, `live` and the `lunarstorm:ui` module; as an extension it
becomes `ui` with the module `lunarstorm:ui`.

## Plugin hosts and plugins

An extension that can be extended opens one or more plugin hosts. A plugin host has:

| part | meaning |
|---|---|
| name | `backend`, unique within the extension |
| contract | a Rust trait the extension defines and calls, e.g. `trait Backend { fn get(..); fn set(..); }` |
| cardinality | `one` (exactly one active plugin) or `many` (any number, in manifest order) |
| requirement | `required` (the extension cannot work without a plugin here) or `optional` |

A plugin is a crate that implements the contract. It depends on the extension's crate, so Cargo's version
resolution checks that plugin and extension agree on the contract. The host compiles in the plugins it
offers; the manifest selects the active ones.

Plugins appear to the program under their extension: the module `lunarstorm:store/sqlite` at the runtime
level, the namespace `native.store.sqlite` at the platform level. A plugin's commands are granted and
scoped like any other command (`store.sqlite.vacuum`).

## The manifest

The manifest declares what an application uses and what it may do. For the CLI host the manifest lives
with the host; a desktop application carries its manifest in its host configuration (`zebra.json`).

```json
{
  "extensions": {
    "store": { "plugins": ["sqlite"], "config": { "file": "state.db" } },
    "ui": {}
  },
  "permissions": {
    "native": ["store.get", "store.set"],
    "scopes": { "store": { "allow": [{ "key": "ui.*" }], "deny": [{ "key": "ui.secret" }] } }
  }
}
```

### Permissions

Three questions, answered at three times:

| permission | answers | written as | checked |
|---|---|---|---|
| **use** | is this extension or plugin active for the application | its entry under `extensions` | when the host loads the manifest |
| **grant** | may the program call this command | `<namespace>.<command>`, `<namespace>.*` | at each call (platform), at link time (runtime: an ungranted export is absent from the module) |
| **scope** | which data may a granted command touch | typed entries under `scopes.<namespace>`, `allow` and `deny` | by the command, through `call.scope::<T>()` |

Evaluation of one call:

1. The extension is active (use). Otherwise its namespace and modules are unknown to the program.
2. The command is granted. Otherwise: `native method <ns>.<cmd> is not permitted by the host configuration`.
3. The data is in scope: every `allow` level present admits it, and no `deny` entry matches.

### Load-time checks

When a host loads a manifest, before any script runs, it reports:

| condition | message |
|---|---|
| an extension the host does not contain | `manifest: this host has no extension <name>` |
| a plugin the extension does not contain | `manifest: extension <name> has no plugin <plugin>` |
| a required plugin host left empty | `manifest: extension <name>: plugin host <slot> requires a plugin` |
| two plugins in a `one` plugin host | `manifest: extension <name>: plugin host <slot> takes one plugin` |
| a grant for an inactive extension or plugin | `manifest: <ns>.<cmd> is granted but <ns> is not used` |
| a scope entry that does not match the extension's schema | `manifest: scopes.<ns>: <detail>` |
| a failing `setup` | `extension <name>: <message>` |

### Trust

The manifest decides what the program can reach and what the host starts. An active extension or plugin
is Rust code running in the host process with the process's rights; choosing it for a host is a decision
to trust it.

## Lifecycle

| step | runtime level | platform level |
|---|---|---|
| compile | the host lists its extensions and plugins as types | the application binary registers them with `Builder` |
| load | the host reads the manifest and runs the load-time checks | the same |
| setup | per Runtime, at `Runtime::start`, for each used extension, plugins after their extension | once per process, before the first window |
| use | modules built on first import; calls run inside the VM | `native.*` calls routed to the declared thread |
| release | the state drops with its Runtime (program end, worker end) | `release_epoch` on backend reload; `Drop` at process end |

## Open questions

- **Module names for extensions.** Proposed: `lunarstorm:<extension>` and `lunarstorm:<extension>/<plugin>`
  at both levels, with the names already in use reserved: `ipc` and `worker` (core), `ui` (CLI host). The platform documents currently
  use `@extension/<name>`.
- **The CLI host's manifest.** Its format and its file inside the host crate.
