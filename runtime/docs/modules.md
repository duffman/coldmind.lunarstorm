# Modules: what a specifier becomes

Source: `runtime/event-loop-runtime/src/modules.rs` (the VM callbacks), `runtime/module-sources` (`lunarstorm-module-sources`:
files, packages, TypeScript, CommonJS), `runtime/jbn-package-format` (`lunarstorm-jbn-package-format`: signed packages).

Every module is an ES module to the engine. `Loader<V, H>` (modules.rs:156) implements
`Callbacks::resolve`/`load`, and it holds only the shared loop state, never the Runtime.

## Resolution order (`Loader::resolve`, modules.rs:283)

```
1. base is a module of the loaded .jbn package?
      → the package's import table, then host modules/builtins; anything else is an error (no disk)
2. lunarstorm:worker-native, imported by lunarstorm:worker      (only when H::WORKERS)
3. lunarstorm:internal, imported by a host module               (a program file is refused)
4. native(specifier):
      lunarstorm:ipc     (only when H::IPC)
      lunarstorm:worker  (only when H::WORKERS)
      a host module      exact specifier from Bindings::module (lunarstorm:ui, @desktop/api, ...)
      a builtin          events net http fs fs/promises path process buffer timers, bare or node:
5. files and packages (lunarstorm-module-sources)
```

Host modules and builtins win over a `node_modules` package of the same name, as in Node.

## What `load` answers (modules.rs:320)

| canonical name | answer |
|---|---|
| a host module | `Ready(source)`, compiled on first import |
| `lunarstorm:ipc` | `Native`: `send` and `onMessage`, closures over this Runtime's state |
| `lunarstorm:worker` | `Ready(js/worker.js)`: the JS half |
| `lunarstorm:worker-native` | `Native`: `spawn`, `post`, `listen`, `listenParent`, `onExit`, `terminate`, `close`, `isWorker` |
| `lunarstorm:internal` | `Native`: `ops` and `live` from the glue |
| `node:<name>` | `Native`: `default` is the glue's object, each own key a named export |
| a module of the `.jbn` package | `Ready(image)`: bytecode, no source |
| a file | `Ready(source + import.meta + source map)` after `lunarstorm-module-sources` transforms it |

### Builtins have no global

`bootstrap.js` returns `{ modules, internal, exportsOf, ... }`, and the Runtime keeps it in
`Registry` (modules.rs:64). On first import of `node:fs`, `declare_builtin` reads `modules.fs`,
asks `exportsOf('fs')` for the key list and declares a native module. Nothing is published on the
global object; `builtins_are_native_modules_and_no_hidden_global_exists` (tests/runtime_embedding.rs) guards
this. The globals that do exist are web/Node conveniences set by the glue: `console`, `process`,
`Buffer`, `setTimeout` and friends, `TextEncoder`/`TextDecoder`, `crypto.getRandomValues`,
`performance.now`, `global`.

### `lunarstorm:internal`: the only door to `ops`

`ops` is the object of Runtime natives (ops.rs) plus everything hosts and extensions registered.
Only a **host module**, one whose canonical name is a specifier registered through
`Bindings::module`, may import `lunarstorm:internal`. A file importing it gets "module not
found". So the API a host exposes is always a JS module wrapping `ops.*` calls.

## Files and packages (`runtime/module-sources`)

| file | becomes |
|---|---|
| `.mjs`; `.js` under `"type": "module"` | itself |
| `.js`/`.ts` with no `type` | ES module if it has module syntax (Node's detection), else CommonJS |
| `.cjs`; `.js` under CommonJS | a **body** module (the file's own name, runs once) + a **facade** `<file>#esm` that `import` reaches, re-exporting `module.exports` as `default` plus statically found names |
| `.json` | `export default JSON.parse(...)` |
| `.ts`, `.mts`, `.cts` | types stripped by oxc (enums and namespaces lowered, never type-checked); **each output line keeps its original line number**, plus a source map for the debugger |

- **Resolution**: relative paths probe `""`, `.js`, `.mjs`, `.ts`, `.mts`, `/index.js` and
  `/index.ts`, and `./x.js` finds `./x.ts`. `require()` probes more (`.cjs`, `.json`, ...).
- **Packages**: `node_modules/<name>` is looked up in each ancestor directory. pnpm symlinks work
  because every path is canonicalized.
- **`package.json`**: `exports` and `imports` with subpath patterns. A condition is chosen by
  the mapping's own key order (as in Node), from the conditions `lunarstorm`, `node`,
  `import`/`require` and `default`.
- **`require()` inside a CommonJS body** becomes an import of `lunarstorm-require:<specifier>`,
  which the loader resolves with require rules (`REQUIRE` prefix). That is how the strip in
  `Loader::native` and `resolve` works.
- **Every read goes through `Host::allow(Access::Read)`**, including each `package.json`.
- **One `Modules` per Runtime** caches manifests and the *kind* of each file. A file that changes
  on disk keeps the kind it was first given.

## `.jbn` packages (`runtime/jbn-package-format`)

A signed container of compiled modules, resources and the import table that links them. The
layout is in `jbn/src/lib.rs`: the `JBNY` magic, a canonical-CBOR manifest, an Ed25519 signature
over `"lunarstorm-jbn-v1\0" || sha256(manifest)`, and a payload aligned to 8.

**Build** (`Runtime::package`, runtime.rs:726; CLI `lunar jbn build`, `lunar project compile`):
1. Start a real Runtime with the same host bindings `run` uses.
2. Walk the graph from the entry. Each module is resolved, transformed and compiled exactly as
   `load` would, through `Scope::compile`: no linking, source stripped, positions kept.
3. Record every `(specifier → target)` pair.

Only static imports, re-exports and `import()` of a string literal are followed; a computed
`import()` can reach host modules and builtins only. Module names stay the build machine's
absolute paths, so stack traces name the source files.

**Run** (`lunar run --trust trust.json app.jbn`):
1. `Package::open` checks the header, the manifest digest, the signature against the
   `TrustStore` (key, allowed package names, `minimum_release`), the manifest's consistency and
   every blob digest.
2. `Runtime::load(Source::Package)` refuses a package built for another `engine::<V>()` string.
3. From then on the package is a **closed world**: nothing is read from disk.

## Where this is hand-mirrored

- `BUILTINS` (modules.rs:44) and the keys of `modules` in bootstrap.js must agree.
  `declare_builtin` fails with "builtin is unavailable" for a name listed in Rust but missing in
  JS.
- `ipc`, `worker` and `internal` are reserved extension names (`lunar-extension::RESERVED`), so no
  extension can claim `lunarstorm:ipc`, `lunarstorm:worker` or `lunarstorm:internal`.
  `lunarstorm:worker-native` is safe because extension names cannot contain `-`.
