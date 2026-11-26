# Build plan: HostApi, the first host extension

Status: **built** on branch `runtime-extensions`. Concepts: `runtime/event-loop-runtime/doc/hosts.md`, `runtime/event-loop-runtime/doc/extensions.md`.

## Goal

HostApi is the extension through which a host gives its programs the host's own API. It opens one plugin
host, `area`. Each API area the host offers is a plugin there, and the manifest selects which areas are
active:

| module | exports |
|---|---|
| `lunarstorm:hostapi` | `areas`: the names of the active areas, in manifest order |
| `lunarstorm:hostapi/<area>` | the API of one active area |

The plan builds the extension machinery HostApi needs and proves it on an existing feature. The CLI host's
terminal API (`lunarstorm:ui`, today registered by `bindings()` in `lunarstorm-host-command-line`) becomes the
HostApi area `ui`. The existing end-to-end tests of that feature, unchanged, are the acceptance test.

## Result

| package | path | contents |
|---|---|---|
| `lunar-extension` | `extensions/core/` | names, manifest, plugin host rules, load-time checks. No VM dependency, shared with the platform level |
| `lunarstorm-event-loop-runtime` | `runtime/event-loop-runtime/` | `Extension<V>`, `ExtensionSet<V>`, `Host::Extensions`, extension setup in `Runtime::start` |
| `lunar-extension-hostapi` | `extensions/hostapi/` | `HostApi<V, A>`, the `Area<V>` contract, `lunarstorm:hostapi` |
| `lunarstorm-host-command-line` | `runtime/host-command-line/` | the area `ui`, the host's manifest, HostApi installed |

```
lunarstorm-host-command-line ──> lunar-extension-hostapi ──> lunarstorm-event-loop-runtime ──> lunarstorm-engine-contract
        │                        │                         │
        └────────────────────────┴─────────────────────────┴──> lunar-extension
```

## Design

### What the Runtime already provides

An extension is built on `Bindings<V>` (`runtime/event-loop-runtime/src/bindings.rs`), the mechanism hosts use today:

- `function(name, f)` and `values(install)` put natives on `ops`;
- `module(specifier, source)` adds a source module, compiled on first import;
- a module added this way is a host module, and only host modules may import `lunarstorm:internal`
  (`modules.rs:303`). That gives them `ops` and the console's `live` hook. `ui.js` depends on this.

The loader, the module privilege and the on-demand compilation stay as they are; `modules.rs` does not
change.

### Runtime level

```rust
/// A named package of functionality a host installs in each of its Runtimes.
pub trait Extension<V: Vm>: 'static {
    /// `[a-z][a-z0-9]{0,31}`. The extension's modules are `lunarstorm:<NAME>` and `lunarstorm:<NAME>/...`.
    const NAME: &'static str;
    /// The plugin hosts it opens, each with the plugins this build offers there.
    fn plugin_hosts(&self) -> Vec<PluginHost> { Vec::new() }
    /// Called at `Runtime::start` when the manifest uses the extension. `using` holds the active plugins,
    /// in manifest order, and the extension's configuration.
    fn bindings(&self, using: &Use<'_>) -> Result<Bindings<V>, String>;
}

/// A host's extensions as one static type: `()`, `(A,)`, `(A, B)`, ... up to 8, in setup order.
pub trait ExtensionSet<V: Vm>: 'static {
    fn offers(&self) -> Vec<Offer>;
    fn bindings(&self, manifest: &Manifest) -> Result<Bindings<V>, String>;
}

pub trait Host: 'static {
    // ...existing members...
    type Extensions<V: Vm>: ExtensionSet<V>;
    fn extensions<V: Vm>(&self) -> Self::Extensions<V>;
    fn manifest(&self) -> &Manifest { Manifest::empty() }
}
```

`Runtime::start`, before it installs the registered bindings:

1. `lunar_extension::check(host.manifest(), set.offers())`. A failure stops `start` with the messages of
   `extensions.md` ("Load-time checks").
2. For each extension the manifest uses, in the set's order: `bindings(using)`. An `Err` stops `start` as
   `extension <name>: <message>`.
3. Each result is merged into the Runtime's bindings. The merge refuses:
   - a module outside the extension's namespace: `extension <name>: module <specifier> is outside lunarstorm:<name>`;
   - a module specifier or `ops` function name that is already bound: `extension <name>: <item> is already bound`.

Properties that follow:

- **Per Runtime.** The set comes from the host, and a worker's Runtime is created from its own host
  (`Host::worker_host`). Each Runtime, the main one and every worker, calls `bindings` itself and holds
  its own state, captured by the binding closures.
- **Workers get extensions.** Today a worker's Runtime installs no host bindings (`worker.rs:494-497`
  calls `new` and `start`, never `register`). Extensions are installed by `start`, so a worker of a host
  has the same extensions as its main Runtime.
- **Inert when unused.** An extension absent from the manifest contributes no bindings, and its
  specifiers do not resolve.
- **No collisions by construction.** Names are unique per host and each extension serves only its own
  namespace. The core names `ipc`, `worker` and `internal` are reserved.
- **Every `impl Host` states its set.** Associated type defaults are unstable, so the 7 existing impls
  each gain two lines: `type Extensions<V: Vm> = ();` and `fn extensions<V: Vm>(&self) {}`. These are
  `CliHost`, `DesktopHost`, the test hosts in `tests/{modules,ipc,embed,worker}.rs`, and
  `examples/ipc-demo.rs`. The workspace's `rust-version` is 1.99, which supports the GAT.

### Manifest

```json
{ "extensions": { "hostapi": { "plugins": ["ui"], "config": {} } } }
```

At the runtime level a permission is given per plugin: a used plugin is granted, and its module exports
everything it defines. Grants per export and data scopes arrive with the first host whose manifest the
application writes (the desktop host). The CLI host's manifest is written by the host's own authors.

### HostApi

| item | value |
|---|---|
| name | `hostapi` |
| plugin host | `area`: cardinality `Many`, `Optional` |
| contract | `trait Area<V: Vm> { const NAME; fn bindings(&self, using: &Use<'_>) -> Result<Bindings<V>, String>; }` |
| the areas as a type | `HostApi<V, A: AreaSet<V>>`, with `A` a tuple such as `(Ui,)` |
| `lunarstorm:hostapi` | a source module: `export const areas = Object.freeze([...ops.hostapi_areas]);`. `hostapi_areas` is set on `ops` by HostApi's `values` |
| area modules | `lunarstorm:hostapi/<area>`. HostApi refuses an area whose module lies outside that path: `extension hostapi: area <name> serves <specifier>` |

### The area `ui`

`runtime/host-command-line/src/ui.rs` holds the CLI host's terminal API, since that API is specific to the host.

| binding | owner after the change | reason |
|---|---|---|
| `ops.color`, `ops.tty` | the CLI host's `bindings()` | the core console reads `ops.color` for its own colouring (`bootstrap.js:524`), whether or not `ui` is used |
| `ops.columns`, `ops.live` | the area `ui` | used only by `ui.js` |
| module, source `lunarstorm_cliui::UI_SOURCE` | the area `ui`, at `lunarstorm:hostapi/ui` | |
| `lunarstorm:ui` | the CLI host's `bindings()`, as `export * from 'lunarstorm:hostapi/ui'; export { default } from 'lunarstorm:hostapi/ui';` | existing scripts and tests keep working and share one module instance with `lunarstorm:hostapi/ui` |

### The CLI host's manifest

`runtime/host-command-line/manifest.json` holds `{ "extensions": { "hostapi": { "plugins": ["ui"] } } }`. It is
embedded with `include_str!`, parsed once into a `OnceLock`, and returned by `CliHost::manifest`. A unit
test in the crate parses it, so a malformed manifest fails the build's tests, not a user's run.

## Steps

Each step starts by writing its tests and seeing them fail on an assertion. Then the code is written, the
same tests pass, and the documentation follows. Sizes are added lines, tests included.

| # | step | files | tests | size |
|---|---|---|---|---|
| 1 | `lunar-extension`: `Name` (`[a-z][a-z0-9]{0,31}`, reserved `ipc`, `worker`, `internal`), `Manifest` (serde, `Manifest::empty()`), `PluginHost`, `Cardinality`, `Requirement`, `Offer`, `Use`, `check` | `extensions/core/{Cargo.toml,src/lib.rs,README.md}`; root `Cargo.toml` members and header | unit tests: valid, invalid and reserved names; each load-time message; the empty manifest passes against any offer | 250 |
| 2 | Runtime: `Extension<V>`, `ExtensionSet<V>` for tuples up to 8, the `Host` members, the checked merge in `Bindings`, extension setup in `start`, two lines in each of the 7 `impl Host` | `runtime/event-loop-runtime/src/{extension.rs,host.rs,bindings.rs,runtime.rs,lib.rs}`, the 4 test hosts, `examples/ipc-demo.rs`, `runtime/hosts/{cli,desktop}/src/*` | `runtime/event-loop-runtime/tests/extensions.rs` (see below) | 280 |
| 3 | HostApi: `HostApi<V, A>`, `Area<V>`, `AreaSet<V>` for tuples, `lunarstorm:hostapi` | `extensions/hostapi/{Cargo.toml,src/lib.rs,js/hostapi.js,README.md}` | `extensions/hostapi/tests/hostapi.rs` (see below) | 220 |
| 4 | The CLI host adopts HostApi: `src/ui.rs`, `manifest.json`, `type Extensions<V: Vm> = (HostApi<V, (Ui,)>,)`, the alias, `lunarstorm:hostapi` in the help's module list (`cli.rs:104`) | `runtime/host-command-line/{Cargo.toml,manifest.json,src/ui.rs,src/host.rs,src/cli.rs}` | `apps/lunarstorm/tests/ui.rs` unchanged and green, plus two new cases (see below); manifest parse test in `host.rs` | 110 |
| 5 | Documentation | READMEs of the four packages; `runtime/event-loop-runtime/doc/extensions.md` (built parts marked built, the open module-name and CLI-manifest questions answered); `runtime/event-loop-runtime/doc/hosts.md` (register step and worker note); root README table | | 120 |

Total about 980 lines.

### Tests in step 2: `runtime/event-loop-runtime/tests/extensions.rs`

Uses a test host with `WORKERS = true` and a test extension `probe`. `probe` counts its `bindings` calls,
binds an `ops` function, and serves `lunarstorm:probe` as a source module reading it.

| test | asserts |
|---|---|
| `used_extension_serves_its_module` | `import { value } from 'lunarstorm:probe'` gives the native's result |
| `unused_extension_is_inert` | absent from the manifest: `bindings` is not called and the import fails with the loader's not-found error |
| `bindings_run_once_per_runtime` | two imports in one program cause one `bindings` call |
| `a_worker_gets_its_own_instance` | a worker importing `lunarstorm:probe` causes a second call; the two states differ |
| `load_time_error_stops_start` | a manifest naming `nosuch`: `start` returns `manifest: this host has no extension nosuch` |
| `module_outside_namespace_is_refused` | `probe` serving `lunarstorm:other`: `start` fails with the namespace message |
| `duplicate_binding_is_refused` | `probe` binding an `ops` name the host already bound: `start` fails with the already-bound message |
| `extension_module_reaches_internal` | `lunarstorm:probe` imports `lunarstorm:internal`; a program file that does the same is still refused |

### Tests in step 3: `extensions/hostapi/tests/hostapi.rs`

Two test areas, `alpha` and `beta`.

| test | asserts |
|---|---|
| `areas_lists_active_areas_in_manifest_order` | manifest `["beta", "alpha"]` gives `areas` = `["beta", "alpha"]` |
| `inactive_area_does_not_resolve` | manifest `["alpha"]`: `lunarstorm:hostapi/beta` fails to import |
| `area_module_works` | `lunarstorm:hostapi/alpha` exports what `alpha` binds |
| `no_areas_is_valid` | `hostapi` used with no plugins: `areas` is empty |
| `unknown_area_is_a_load_error` | manifest `["gamma"]`: `manifest: extension hostapi has no plugin gamma` |

### Tests in step 4: `apps/lunarstorm/tests/ui.rs`

| test | asserts |
|---|---|
| `hostapi_lists_ui` | `import { areas } from 'lunarstorm:hostapi'; console.log(areas.join())` prints `ui` |
| `ui_alias_is_the_same_module` | `lunarstorm:ui` and `lunarstorm:hostapi/ui` give the identical `style` object, and the same `default` |

## Verification

These are the focused suites, run on the final tree. Each test count is read and must be non-zero.

```text
cargo test -p lunar-extension
cargo test -p lunarstorm-event-loop-runtime --test extensions --test embed --test modules --test worker --test ipc
cargo test -p lunar-extension-hostapi
cargo test -p lunarstorm-host-command-line
cargo test -p lunarstorm --test ui --test cli --test commands
cargo test -p lunarstorm-host-desktop-backend --test backend
cargo clippy -p lunar-extension -p lunarstorm-event-loop-runtime -p lunar-extension-hostapi -p lunarstorm-host-command-line
```

`lunarstorm-host-desktop-backend` is in the list because its `impl Host` changes, even though its set stays `()`.

Startup cost: run `console.log(1)` through the release CLI three times before step 2 and three times after
step 4. The CLI now runs the manifest check and HostApi's `bindings` on every start. The source modules
still compile only on first import.

## Decisions taken in this plan

| decision | taken as |
|---|---|
| how an extension contributes | a `Bindings<V>` per Runtime, the mechanism hosts already use; no new loader path |
| extension packages location | `extensions/` at the repository root |
| module names | `lunarstorm:<extension>` and `lunarstorm:<extension>/...`, enforced at `start`; `ipc`, `worker`, `internal` reserved |
| setup order | the order of the host's extension tuple |
| runtime-level permissions | per plugin (use); per-export grants and scopes come with the desktop host's adoption |
| `ops.color`, `ops.tty` | stay host bindings, because the core console uses them |
| `lunarstorm:ui` | a re-export of `lunarstorm:hostapi/ui`, so both specifiers share one instance |
| CLI host manifest | `runtime/host-command-line/manifest.json`, embedded and parsed once |
| the desktop host | builds with the empty set `()`; adopting HostApi there is a separate plan |
