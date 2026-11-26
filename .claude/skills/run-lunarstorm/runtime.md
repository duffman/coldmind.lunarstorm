# Runtime (`runtime/`): the Node.js-like JavaScript runtime

A Rust event loop (mio) around one JS engine (QuickJS-NG). Scripts get Node-style `node:*`
builtins plus `lunarstorm:*` modules. The runtime is a library; a **host** owns it, and decides
argv, stdio, file access, which extensions exist and whether workers/IPC exist.

## Crates (names as in Cargo.toml)

| dir | crate | role |
|---|---|---|
| `runtime/engine-contract` | `lunarstorm-engine-contract` | engine contract: `Vm`, `Instance`, `Context`, `Scope`, `Callbacks`, `Call`, `Load`, `Limits`, `Image`; conformance suite (15 checks, `src/conformance.rs`) |
| `runtime/engine-quickjs-adapter` | `lunarstorm-engine-quickjs-adapter` | the only adapter: quickjs-ng `0.17.0+lunarstorm.1`; capabilities memory_limits, termination, bytecode_cache, debug_hooks |
| `runtime/vendor/rquickjs-binding` | `rquickjs`, `rquickjs-core`, `-sys`, `-macro` | the engine binding, owned in-repo (from upstream 0.14.0) |
| `runtime/event-loop-runtime` | `lunarstorm-event-loop-runtime` | the Runtime: loop, timers, ops, loader, fs/net/http, ipc, worker, extensions, `js/bootstrap.js` glue |
| `runtime/module-sources` | `lunarstorm-module-sources` | TS/JSON/CJS → ESM (oxc), `node_modules` + `package.json` `exports`/`imports` |
| `runtime/jbn-package-format` | `lunarstorm-jbn-package-format` | signed `.jbn` packages (Ed25519, CBOR, zstd), trust store |
| `runtime/debugger` | `lunarstorm-debugger` | DAP debugger (from jetbunny) |
| `runtime/compiler/{js-parser-frontend,scalar-ssa-ir,bytecode-emitter,bytecode-image-verifier,reference-engine-bridge}` | `qjs-js-parser-frontend`, `qjs-scalar-ssa-ir`, `qjs-bytecode-emitter`, `qjs-bytecode-image-verifier`, `qjs-reference-engine-bridge` | the Rust JS compiler: OXC frontend, scalar SSA IR, bytecode emitter, image verifier, reference-engine bridge for tests |
| `runtime/host-command-line` | `lunarstorm-host-command-line` | library: the `run`, `debug`, `jbn` commands and `CliHost`; mounted by `lunar` |
| `runtime/host-desktop-backend` | `lunarstorm-host-desktop-backend` (+ bin `lunarstorm-desktop`) | a desktop app's backend on a thread, new Runtime per epoch |

## How `lunar run file.js` runs

1. `runtime/host-command-line/src/lunar_runtime_commands.rs` `run`: argv is `["lunar", script, ...]` (`ARGV0`); a `.jbn` needs `--trust` or `LUNARSTORM_TRUST`.
2. `Runtime::<QuickJs, CliHost>::new` (mio poll, waker), `register(command_line_host_services::bindings())` (`ops.color`, `ops.tty`, `lunarstorm:ui`).
3. `Runtime::start` (`runtime/event-loop-runtime/src/runtime_instance.rs` ~583): check the host's manifest, install extensions, create Loader + engine instance, `ops::install`, run binding installers, `node_builtins::install` (the Node surface, native; no script is compiled at start).
4. `load(Source::File)`: read through `Host::allow(Access::Read)`, transform TS/CJS/JSON, evaluate, `settle`.
5. `run()`: `Turn::step` until idle, then exit with `process.exitCode`.

Loop turn (`Turn::step`): debugger service → timers → stdout flush → mio poll (gated by `POLL_GAP`
250 µs while work is queued) → I/O events → posted tasks (IPC messages, worker events) →
immediates. After every JS entry `settle` drains nextTicks, then promise jobs, until both are
empty, then checks out-of-memory and reports unhandled rejections. Idle = no immediates, tasks,
timers, live I/O, holds or refs.

## Module resolution (`runtime/event-loop-runtime/src/module_loader_callbacks.rs`, in order)

1. `.jbn` package import table (closed world: a package never reads the filesystem).
2. `lunarstorm:worker-native`: only from `lunarstorm:worker`.
3. `lunarstorm:internal` (`{ops, live}`): only from **host modules** (their canonical name is their specifier, never a path), so program files get "module not found".
4. Host modules: exact specifier, registered via `Bindings::module` (extension modules are these).
5. `lunarstorm:ipc` / `lunarstorm:worker`: only if `Host::IPC` / `Host::WORKERS`.
6. Builtins: `events net http fs fs/promises path process buffer timers`, as `node:x` or bare `x`. Native modules declared from the glue's registry.
7. Files and packages via `lunarstorm-module-sources`: probes `"" .js .mjs .ts .mts /index.js /index.ts`; `./x.js` finds `x.ts`; conditions `lunarstorm, node, import|require, default`. `.tsx`/`.jsx` are not transformed.

## What a script has (checked with `lunar -e`)

- Globals: `console`, timers, `setImmediate`, `queueMicrotask`, `TextEncoder`/`TextDecoder`, `crypto.getRandomValues`, `performance.now`, `Buffer`, `process`.
- `fs`: read/write/append/stat/readdir/mkdir/unlink/rmdir/rename/rm in promise, callback and Sync forms. No streams, watch, fds.
- `net`: server and client sockets. `http`: **server only**.
- Missing: `fetch`, `URL`, `structuredClone`, `http.request`, `https`, `node:os`, `node:crypto`, `node:child_process`; `Buffer` is UTF-8 only (`toString("base64")` throws `Unsupported encoding: base64`); `process` never emits `'exit'`.
- In the CLI host: `lunarstorm:ui` (= `lunarstorm:hostapi/ui`) and `lunarstorm:hostapi` work; `lunarstorm:worker`/`ipc`/`internal` are "module not found".

## The Host trait (`runtime/event-loop-runtime/src/host_services_trait.rs`)

Required: `type Extensions<V>` + `extensions()` (write `()` for none), `args`, `environment`,
`working_directory`, `write(Stream, &[u8])` (stdout batched per turn), `exit(code) -> !`,
`allow(Access::{Read, Write, Listen, Connect})` (refusal = `EACCES` in JS), `entropy`, `log`.
Defaulted: `PULSE`, `IPC`, `WORKERS` (all `false`), `manifest()` (empty), `monotonic`/`wall`,
`worker_host()` (`None`; needed when `WORKERS`), `ipc_send` (no-op), `uncaught` (log + `exit(1)`).

| host | IPC/WORKERS | extensions | file access |
|---|---|---|---|
| `CliHost` (`runtime/host-command-line/src/command_line_host_services.rs`) | off | `(HostApi<V, (Ui,)>,)` + `manifest.json` | everything |
| desktop backend (`runtime/host-desktop-backend/src/lib.rs`) | off, `PULSE` on | `()` | read/write under `roots` only; listen/connect refused |
| `ipc-demo` (`runtime/event-loop-runtime/examples/ipc-demo.rs`) | on | `()` | example |

Workers: each is a new Runtime on its own thread from `Host::worker_host`; at most 4096 messages,
64 MiB and 8 MiB per message per direction; messages are a tag byte + JSON or a raw `Uint8Array`.

## Operate

```sh
.claude/skills/run-lunarstorm/driver.sh cli           # --version, -e, a script using node:fs
.claude/skills/run-lunarstorm/driver.sh runtime-ipc   # workers + lunarstorm:ipc (ipc-demo host)
.claude/skills/run-lunarstorm/driver.sh project       # signed .jbn: init -> compile -> run --trust
~/.cargo-target/debug/lunar -e 'console.log(typeof fetch)'
cargo test -q -p lunarstorm-engine-quickjs-adapter                # conformance of the adapter
cargo test -q -p lunarstorm-event-loop-runtime --test modules --test worker --test ipc
cargo test -q -p lunarstorm --test commands jbn       # jbn key/build/run --trust end to end
```

`runtime/benchmarks/run.sh [secs]` compares `_build/lunar` with node and bun (needs `oha`, node, bun,
about a minute per runtime) and appends to `runtime/benchmarks/results.txt`.

## Hazards

- **Hand-mirrored tables:** change both sides together.
  - `ev` codes (`runtime.rs` `mod ev`) ↔ `dispatch` in `bootstrap.js`.
  - `FS_*` codes in `bootstrap.js` ↔ `fs.rs`.
  - `METHODS` in `bootstrap.js` ↔ `crates/http1`.
- **`Host::exit` never returns.** A host that must outlive its script runs the Runtime on a thread it can abandon.
- **Out-of-memory is fatal even when caught.** The engine's mark is checked after every entry.
- **Bump the engine identity suffix** (`engines/quickjs/src/lib.rs` ~89) whenever the vendored engine's bytecode changes: `.jbn` packages check it exactly.
- **Long test build:** `lunarstorm-debug` compiles jetbunny's whole VM test corpus, so `cargo test -p lunarstorm-debugger` is slow.
- **Stale READMEs:**
  - `runtime/engine-contract` and `runtime/engine-quickjs-adapter` (check counts, capabilities, `Load::Native`).
  - `runtime/event-loop-runtime` (says bare packages are rejected; they resolve).
  - `runtime/host-command-line` (argv[0] and the command list).

  Trust the code.
