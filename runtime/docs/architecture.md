# Architecture: the layers and the flows that matter

## Layers and crates

```
                        platform/cli/app (lunar)     platform/desktop/app (lunarstorm-debug/-release)
                               │                                 │
HOSTS        platform/cli/host  lunarstorm-host-cli      platform/desktop/host  lunarstorm-host-desktop
                 │  CliHost, command.rs (run, debug, jbn)      │  DesktopHost, Backend<S: Shell>, watchdog
                 └───────────────┬─────────────────────────────┘
RUNTIME      runtime/core  lunarstorm-runtime     Runtime<V, H>: loop, ops, glue, loader, ipc, workers
                 │ uses                  │ uses                      │ uses
             runtime/modules         runtime/jbn                 extensions/core (lunar-extension)
             lunarstorm-modules      lunarstorm-jbn              crates/http1 (lunarstorm-http1)
                 │ (oxc)
VM CONTRACT  runtime/vm  lunarstorm-vm  ──depends on──▶ runtime/debug (protocol types), runtime/jbn (Image)
ENGINE       runtime/engines/quickjs  lunarstorm-vm-quickjs
             runtime/engines/rquickjs  rquickjs + rquickjs-sys (patched quickjs-ng 0.17.0, C)
COMPILER     runtime/compiler/crates/qjs-*   the only JS → bytecode compiler, called from C through rquickjs_core::compiler
DEBUGGER     runtime/debug  lunarstorm-debug  DAP adapter + engine bridge (QuickJS-ng FFI)
BENCH        runtime/bench   http/tcp/loop benchmarks against node and bun
```

The rule at both of `runtime/core`'s boundaries: it names no engine type and no host type.
`Runtime<V: Vm, H: Host>` is monomorphized, so a call into the engine or the host is a direct
call. Engines and hosts are chosen in exactly two places: `Runtime::<QuickJs, CliHost>` in
`platform/cli/host/src/command.rs`, and `pub type Engine = QuickJs` in `platform/desktop/host/src/lib.rs`.

Much of this was ported from an earlier project, *jetbunny*, and the name survives in the
engine patches (`jetbunny-*-v1`), in `JS_Jetbunny*` symbols, in `debug/tests/jetbunny_*`, and
in comments that say "ported from jetbunny". Read "jetbunny" as "the previous runtime this code
came from".

## Flow 1: `lunar run app.ts`

```
platform/cli/app main → cli-host parses argv → package lunarstorm-host-cli, command run (command.rs:124)
  execute(Source::File, argv)
    Runtime::<QuickJs, CliHost>::new(host)       mio Poll + Waker, Shared{io, host, refs...}, Ipc, Workers,
                                                 Modules (reads gated by Host::allow)
    register(host::bindings())                   ops.color, ops.tty, module lunarstorm:ui
    start()                                      runtime.rs:583
      extensions: check manifest → merge HostApi bindings
      QuickInstance::new(limits, Loader)         loader/rejection/interrupt hooks share one Rc<Loader>
      instance.context()                         rquickjs installs the Rust compiler (base); Debugger::wrap adds its observer
      ops::install → installers → glue()         glue: compile bootstrap.js once per process, cache Image
      bootstrap(ops) → {dispatch, ticks, describe, exitCode, invoke, modules, internal, exportsOf}
    load(Source::File)                           runtime.rs:652
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
platform/desktop/app UI thread ─ Backend::load(Program{epoch, code, roots, budget, ...}) ─▶ backend thread
  start(): DesktopHost{shell, epoch, roots} → Runtime → register(desktop bindings) → start
           hold(true) → watchdog(pulse, interrupt, budget) → load(Source::Text "backend.js")
  envelope from a WebView ─ Backend::dispatch(epoch, json) ─ poster.post(task) ─▶ glue dispatch(json)
  backend calls native.ns.method() → ops.desktopNative → Shell::native(epoch, id, ...)
  platform answers ─ Settler::settle(epoch, id, result) ─ post ─▶ glue settle → promise resolves
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
command.rs debug → lunarstorm_debug::debug::host::run(owner closure on its own thread)
  Runtime::new(CliHost::for_debugger)       script stdout → stderr (stdout is the DAP channel)
  start → runtime.debug(break_on_start: true) → VmDebugHandle to the DAP adapter
  load → loop poll(Block): phase 0 serves debugger requests; entry() holds the first JS entry
         until the client's configurationDone; DAP thread wakes the loop through the waker
```

Details of the debugger side are in debugger.md.
