# lunarstorm-debug

jetbunny's debugger, taken over whole: the Debug Adapter Protocol adapter
(`debug::DapAdapter`), the stdio transport (`debug::host::run`), the coordinator
(`debug::DebugCoordinator`, breakpoints, stepping, pause, inspection, evaluation
in frame), source registration and Source Map v3 (`debug::sources`,
`debug/source_map.rs`), and the QuickJS-NG backend (`engines::quickjs_ng`). The code
is jetbunny's, unchanged except where marked `lunarstorm patch`.

## How Lunarstorm uses it

`engines::quickjs_ng::Debugger` (`attach.rs`) puts jetbunny's backend around a
context rquickjs created (`jbq_context_wrap` in the engine's `bridge.c`). The owner
calls in where jetbunny's own engine hooks did:

| owner | calls |
|---|---|
| interrupt handler (`lunarstorm-engine-quickjs-adapter`) | `Debugger::poll` |
| rejection tracker | `Debugger::rejection` |
| module compile (`declare`) | `register` → compile with `Debugger::EVAL_FLAG` → `loaded` |
| loop (`lunarstorm-event-loop-runtime`) | `before_entry` before each JS entry, `service` each step, `checkpoint` after each microtask drain |

Through the VM contract: `Instance::debug`, `debug_entry`, `debug_service`,
`debug_checkpoint` (`lunarstorm-engine-contract`), `Runtime::debug` (`lunarstorm-event-loop-runtime`), and
`lunarstorm debug <script>` (`lunarstorm-host-command-line`).

Sources are registered by `file://` URI, the form DAP client paths become. A
TypeScript module is registered as `<file URI>.js` with the map from
`lunarstorm-module-sources` (`strip_types_mapped`) back to the `.ts` file.

## Not taken

jetbunny's own VM (`VirtualMachine`, program, scheduler, module provider) and its
Rust compiler callback: Lunarstorm has its own loop and loader. The acceptance tests
that drive that VM (`quickjs_ng_data`, `_vm`, `_module_vm`, `_modules`, `_jbn`) are
in `.archive/2026-11-24/runtime/debugger/tests/`; the compiler is in
`.archive/2026-11-24/runtime/debugger/src/engines/quickjs_ng/compiler.rs`.

## Tests

The files in `tests/` are jetbunny's unit tests, compiled into the library through
`#[path]` as in jetbunny (`autotests = false`):

    cargo test -p lunarstorm-debugger

`build.rs` sets the build identity jetbunny's images carry: the engine sources'
hash, target and ABI.
