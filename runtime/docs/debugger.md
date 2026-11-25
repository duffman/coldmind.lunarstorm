# The debugger (`runtime/debug`, crate `lunarstorm-debug`)

This is jetbunny's debugger, copied whole and marked `lunarstorm patch` where it was changed. It
has four parts:
- a DAP translator;
- an owner-thread coordinator;
- a source registry with Source Map v3;
- a QuickJS-ng backend over a private C bridge (`jbq_*`, ABI 14) compiled into `rquickjs-sys`.

**The one production caller is `lunar debug <script>`** (`runtime/hosts/cli/src/cli.rs`). The
desktop binary also called `lunarstorm-debug` (`platform/desktop`) is a different thing: the dev
runtime.

There is no debugger VM. The debugger uses the same `Runtime` and the same loop through four VM
contract hooks:

| hook | called by the Runtime | does |
|---|---|---|
| `Instance::debug(ctx, wake, options)` | `Runtime::debug`, after `start`, before `load` | attaches a session to the context's bridge wrapper; returns `VmDebugHandle` |
| `debug_entry` | before every JS entry (`Turn::entry`) | holds the first entry until the client's `configurationDone` |
| `debug_service` | loop phase 0 | serves requests; `true` = client asked to stop |
| `debug_checkpoint` | after each microtask drain (`settle`) | qualifies rejections as unhandled |

`lunarstorm-vm` re-exports the crate (`pub use lunarstorm_debug`), which is why hosts write
`lunarstorm_runtime::vm::lunarstorm_debug::...`.

## Threads

```
main thread   DAP loop: adapter.poll() → recv_timeout(2 ms) on input → adapter.receive() → frames to writer
stdin reader  4 KiB reads → sync_channel(8)
stdout writer sync_channel(32)    (full → session fails "DAP output queue is full")
owner thread  the Runtime and the script; blocks inside the interpreter while paused
                ▲
Session (Mutex + Condvar) is the only channel; a request also wakes the Runtime's mio waker
and sets the native Signal flag the statement hook checks.
```

## A session, end to end

1. **Start**: `debug::host::run` spawns the owner closure. That closure builds
   `Runtime<QuickJs, CliHost::for_debugger>` (script stdout goes to stderr, because stdout is the
   protocol), calls `start()`, then `runtime.debug(break_on_start: true)`, and sends an
   `OwnerSession`.
2. **The client must send `attach`.** `launch` is unsupported, and a conformance test asserts
   that. `DapAdapter::new` refuses a session without break-on-start.
3. **Entry gate**: `load` → `entry()` → `before_entry` blocks the owner on the Condvar, serving
   only debug requests. `configurationDone` opens the gate once earlier setup replies have
   landed. The entry pause is resumed automatically, without a `stopped` event, so there is no
   stopOnEntry.
4. **Breakpoints**: `setBreakpoints` turns the path into a `file://` URI. `SourceRegistry`
   resolves it, through the source map if there is one, to the nearest statement marker at or
   after the position. `jbq_debug_breakpoint` then sets one bit per marker in the function's
   `debug_bp`.
5. **Pause**: `OP_debug_stmt` (patched engine) → `js_debug_marker` → the bridge → Rust `hook` →
   `on_hook` picks a reason (requested, `debugger` statement, exception, breakpoint, step) →
   `pause_at` → `wait_pause` blocks on the Session Condvar. **The whole loop is frozen**: no
   timers, I/O or microtasks while paused.
6. **Inspect**: `stackTrace`, `scopes` and `variables` are answered inside `wait_pause` through
   `jbq_debug_frame_at`, `jbq_debug_variables` and the paged `jbq_debug_inspect`. References are
   invalidated at every pause change and every evaluate.
7. **Evaluate**: a REPL evaluate requires a `frameId`. It runs as a direct eval bound to that
   frame (`jbq_debug_eval_verified`), compiled by the Rust compiler. Hover and watch may only
   read binding or property paths, without executing. Breakpoints do not fire inside an evaluate.
8. **Disconnect** detaches and resumes; the script keeps running. stdin EOF requests a stop.
   `terminate`, `restart` and `suspend` are unsupported.

## Source maps and TypeScript

```
runtime/modules strip_types_mapped → map (sources: file name relative to <file>.js)
Modules::source_map TAKES it (once) → ModuleSource.source_map
quickjs adapter declare(): session open → Debugger::stage(text, name, module, map)  (one-slot staging)
Rust compiler CompileObserver → register_source: takes the staged entry only if the SHA of the exact text matches
→ markers collected (jbq_debug_markers) → breakpoints can bind
```

## What is debuggable

Only code compiled after `Runtime::debug()`. The glue image, `.jbn` bytecode and anything
compiled before attach have no statement markers.

## Hazards

- **ABI lockstep.** `JBQ_ABI` must agree in `bridge.h:30`, `ffi.rs:13` and `build.rs`. Bump it
  on any signature or struct-layout change; adding functions does not need a bump.
- **`build.rs` identity is hand-written**: the patch list, `ABI=14`, `BYTECODE=94`, the revision.
  It hashes `../engines/rquickjs/sys/quickjs` and `../compiler/crates/qjs-*` by relative path,
  so moving the crate breaks the build. `tests/compiler_build_identity.rs` asserts
  `;bytecode=94;`.
- **Stage then compile, one slot.** The staged text must be byte-identical to the compiled text.
  A transform between `stage` and `Module::declare`, or a second compile in between, breaks
  registration (`InvalidSource`).
- **A source map's `file` must resolve to `<uri>.js`**, or the source fails closed.
- **Every statement calls into Rust** while attached (the flags are always ATTACHED|STEP). Debug
  runs are slow by design.
- **Never print to stdout** on the owner thread under `lunar debug`.
- **The CLI's stop handle** does not use `coordinator.stop_signal()`, though the coordinator
  says an ordinary stop should go there. It relies on `end_input` detaching to free a paused
  owner. Its `VmHandle` epoch is hard-coded to 1, which matters only with a target broker.
- **`autotests = false`.** A new `tests/x.rs` runs only if it is `#[path]`-mounted from `lib.rs`
  or given a `[[test]]` entry.
- **There is no end-to-end test of the Lunarstorm path.** Nothing in core, quickjs, the CLI host
  or vm tests calls `Runtime::debug`, `Debugger::wrap` or `DapAdapter` together. The about 40
  jetbunny test files mounted in `lib.rs` drive jetbunny's own VM, which is compiled only under
  `#[cfg(test)]` (`engine/bridge.rs`, `runner.rs`, `vm/mod.rs`, `transport/`, `host.rs`,
  `driver.rs`, ...). Test-only `src/host.rs` is not production `src/debug/host.rs`.

## Code that is compiled but unused by Lunarstorm

- `debug/targets.rs` and `target_dap.rs`: a multi-target broker for worker sessions; the CLI
  passes `targets: None`.
- `QuickJsNgEngine` and `EngineRegistry`.

## README drift

- `runtime/debug/README.md` describes `Debugger::EVAL_FLAG`, which does not exist; the real flow
  is stage, then the compiler observer.
- It says jetbunny's VM and its acceptance tests were archived. They are compiled under
  `cfg(test)` and mounted, and `.archive/2026-11-24/runtime/debug/` does not exist. The same
  missing archive is cited in `quickjs_ng/mod.rs:690` and `compiler.rs:14`.
- The `quickjs_ng/mod.rs` header still says stock QuickJS-ng has no interactive backend.
- `PATCHES.md` (debugger-v1) says `BC_VERSION` is `29|0x40`; the engine has `30|0x40`.

## Test

```sh
cargo test -q -p lunarstorm-debug      # lib tests (~40 jetbunny files mounted as modules) + jetbunny_copy_alloc
```
