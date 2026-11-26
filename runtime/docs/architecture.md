# Architecture: the layers and the flows that matter

## Layers and crates

```
                        apps/lunarstorm (lunar)     platform/desktop (lunarstorm-debug/-release)
                               │                                 │
HOSTS        runtime/host-command-line  lunarstorm-host-command-line      runtime/host-desktop-backend  lunarstorm-host-desktop-backend
                 │  CliHost, lunar_runtime_commands.rs (run, debug, jbn)          │  DesktopHost, Backend<S: Shell>, watchdog
                 └───────────────┬─────────────────────────────┘
RUNTIME      runtime/event-loop-runtime  lunarstorm-event-loop-runtime     Runtime<V, H>: loop, ops, Node builtins, loader, ipc, workers
                 │ uses                  │ uses                      │ uses
             runtime/module-sources         runtime/jbn-package-format                 extensions/core (lunar-extension)
             lunarstorm-module-sources      lunarstorm-jbn-package-format              crates/http1 (lunarstorm-http1)
                 │ (oxc)
VM CONTRACT  runtime/engine-contract  lunarstorm-engine-contract  ──depends on──▶ runtime/debugger (protocol types), runtime/jbn-package-format (Image)
ENGINE       runtime/engine-quickjs-adapter  lunarstorm-engine-quickjs-adapter
             runtime/vendor/rquickjs-binding  rquickjs + rquickjs-sys (patched quickjs-ng 0.17.0, C)
COMPILER     runtime/compiler/*             the only JS → bytecode compiler, called from C through rquickjs_core::compiler
DEBUGGER     runtime/debugger  lunarstorm-debug  DAP adapter + engine bridge (QuickJS-ng FFI)
BENCH        runtime/benchmarks   http/tcp/loop benchmarks against node and bun
```

The rule at both of `runtime/event-loop-runtime`'s boundaries: it names no engine type and no host type.
`Runtime<V: Vm, H: Host>` is monomorphized, so a call into the engine or the host is a direct
call. Engines and hosts are chosen in exactly two places: `Runtime::<QuickJs, CliHost>` in
`host-command-line/src/lunar_runtime_commands.rs`, and `pub type Engine = QuickJs` in `host-desktop-backend/src/lib.rs`.

Much of this was ported from an earlier project, *jetbunny*, and the name survives in the
engine patches (`jetbunny-*-v1`), in `JS_Jetbunny*` symbols, in `debugger/tests/jetbunny_*`, and
in comments that say "ported from jetbunny". Read "jetbunny" as "the previous runtime this code
came from".

## Flow 1: `lunar run app.ts`

```
apps/lunarstorm main → cli-host parses argv → package lunarstorm-host-command-line, command run (cli.rs:128)
  execute(Source::File, argv)
    Runtime::<QuickJs, CliHost>::new(host)       mio Poll + Waker, Shared{io, host, refs...}, Ipc, Workers,
                                                 Modules (reads gated by Host::allow)
    register(host::bindings())                   ops.color, ops.tty, module lunarstorm:ui
    start()                                      runtime_instance.rs
      extensions: check manifest → merge HostApi bindings
      QuickInstance::new(limits, Loader)         loader/rejection/interrupt hooks share one Rc<Loader>
      instance.context()                         rquickjs installs the Rust compiler (base); Debugger::wrap adds its observer
      ops::install → installers                  host natives on `ops`
      node_builtins::install → {dispatch, ticks, describe, exitCode, invoke, modules, internal, exportsOf}
    load(Source::File)                           runtime_instance.rs
      read under Access::Read → Modules::entry (TS stripped, CJS wrapped, source map kept)
      scope.module(name, source) → engine resolves imports through Loader::resolve/load (recursively, synchronously)
      settle() → drain_deferred()
    run()                                        loop Turn::step until Idle (event-loop.md)
    stop()                                       process.exitCode; flush stdout; drop workers, ipc, VM
```

The exit status is `process.exitCode`, 1 when the entry cannot be read, and 70 when the Runtime
cannot start. An uncaught error or `process.exit()` ends the process inside the host
(`CliHost::exit`) and never returns.

## Flow 2: an HTTP request

```
socket readable (edge) → on_event → http_readable → http_pump
  read into scratch (64 KiB) → http1::advance parses in place → flags BUSY (+CORKED if more follow)
  fire(HTTP, listener, conn, method idx|token, url, fields text, body)
    → dispatch case 8: IncomingMessage + ServerResponse → server.emit('request')
    → res.end(body) → ops.httpRespond(key, status, fields, body, close) → Io::http_respond
        one writev of head+body; tail buffered; BUSY cleared; keep-alive or ENDING
  settle (nextTick, promise jobs); next pipelined request from the same buffer
  async handler: socket left unread while BUSY; Deferred::Resume restarts the pump when it answers
```

## Flow 3: the desktop backend

```
platform/desktop UI thread ─ Backend::load(Program{epoch, code, roots, budget, ...}) ─▶ backend thread
  start(): DesktopHost{shell, epoch, roots} → Runtime → register(desktop bindings) → start
           hold(true) → watchdog(pulse, interrupt, budget) → load(Source::Text "backend.js")
  envelope from a WebView ─ Backend::dispatch(epoch, json) ─ poster.post(task) ─▶ desktop glue dispatch(json)
  backend calls native.ns.method() → ops.desktopNative → Shell::native(epoch, id, ...)
  platform answers ─ Settler::settle(epoch, id, result) ─ post ─▶ desktop glue settle → promise resolves
  property writes batched per promise job → ops.desktopSend → Shell::send(epoch, envelope)
  reload = control message: retire old Runtime (Shell::retired) → start a new one with epoch+1
```

## Flow 4: build and run a signed package

```
lunar jbn build app.js -o app.jbn --key k.der    (or lunar project compile)
  same Runtime + bindings as run → Runtime::package: walk imports, Scope::compile each module
  → jbn::Builder (bytecode + import table) → sign (Ed25519 over the manifest digest)
lunar run --trust trust.json app.jbn
  Package::open verifies everything → load(Source::Package): engine identity must match
  → entry Image → imports answered only from the package's table (closed world)
```

## Flow 5: `lunar debug app.js` (DAP over stdio)

```
cli.rs debug → lunarstorm_debugger::debug::host::run(owner closure on its own thread)
  Runtime::new(CliHost::for_debugger)       script stdout → stderr (stdout is the DAP channel)
  start → runtime.debug(break_on_start: true) → VmDebugHandle to the DAP adapter
  load → loop poll(Block): phase 0 serves debugger requests; entry() holds the first JS entry
         until the client's configurationDone; DAP thread wakes the loop through the waker
```

Details of the debugger side are in debugger.md.
