# lunarstorm-event-loop-runtime (runtime/event-loop-runtime)

The Lunarstorm Runtime: a single-threaded mio reactor that owns every socket and one VM instance, with timers, immediates, `process.nextTick` and promise-job draining, an ES module loader, fs and net under host policy, and a Node-shaped JS glue (`src/js/bootstrap.js`: EventEmitter, net, http, fs, path, process, buffer, timers, console, TextEncoder/Decoder). It is the middle layer of doc/plans/main.md §5. It is generic over `V: Vm` and `H: Host`, both bound at compile time, and names no engine and no host type.

## Public surface

- `Runtime<V, H>`: `new(host)`, `limits`, `register(Bindings)`, `start`, `load(Source)`, `call(export, &[Arg]) -> CallId`, `result(CallId)`, `post(task)`, `poll(Wait) -> State`, `run() -> i32`, `stop() -> i32`, plus `poster()`, `hold(bool)`, `pulse()`, `interrupt()`, `polls()`. `Source::{File(PathBuf), Text { name, code }}`; `Arg::{Null, Bool, Number, Text, Bytes}`; `Wait::{Block, NoBlock}`; `State::{Running, Idle}`.
- `Poster<V>`: the one `Send` part, posts tasks to the loop from any thread (a burst costs one wake syscall).
- `trait Host`: `args`, `environment`, `working_directory`, `write(Stream, bytes)`, `exit(code) -> !`, `allow(Access) -> bool`, `monotonic`, `wall`, `entropy`, `log(Log)`, `uncaught(in_promise, description)`, `worker_host`, `ipc_send`, the constants `PULSE`, `IPC`, `WORKERS`, and the extension members `type Extensions<V: Vm>`, `extensions()`, `manifest()`. `Access::{Read, Write, Listen, Connect}`.
- Extensions (`src/extension.rs`, design in `doc/extensions.md`): `trait Extension<V>` (`NAME`, `plugin_hosts()`, `bindings(&Use) -> Result<Bindings<V>, String>`), `trait ExtensionSet<V>` for `()` and tuples up to 8, and from `lunar-extension`: `Manifest`, `Use`, `Offer`, `PluginHost`, `Cardinality`, `Requirement`, `check`, `valid_name`, `RESERVED`.
- `Bindings<V>`: `function(name, f)`, `values(install)`, `module(specifier, source)`: how a host or an extension adds natives and ES modules before `start`; `merge(owner, other)` adds a set confined to `lunarstorm:<owner>` and refuses a module or `ops` function already bound. Re-exports `lunarstorm_engine_contract` as `vm`.
- Scripts see: `node:events|net|http|fs|fs/promises|path|process|buffer|timers` (bare `fs` also works), host modules by exact specifier, relative files (`./x`, `../x`, `/x`: tries exact path, `.js`, `.mjs`, `/index.js`).

## How it fits

- Depends on `lunarstorm-engine-contract`, `lunar-extension` (`extensions/core`), `lunarstorm-http1`, `mio`, `crossbeam-channel`, `bytes`, `slab`, `libc`. Dev-depends on `lunarstorm-engine-quickjs-adapter` (the library itself names no engine).
- Used by `runtime/host-command-line` (the `run` command) and `runtime/host-desktop-backend` (backend thread), which bind the engine, and by extensions (`extensions/hostapi`).
- Concepts: `doc/hosts.md` (the Runtime, hosts, the host contract), `doc/extensions.md` (extensions, plugin hosts, plugins, manifest, permissions).
- Spec: doc/plans/main.md §5, "RUNTIME INTERFACE" (start/load/stop, call, post, poll, register), "RUNTIME", "Bindings", "Isolation".

## Build and test

```text
cargo build -p lunarstorm-event-loop-runtime
cargo test -p lunarstorm-event-loop-runtime      # tests/{embed,extensions,ipc,modules,worker}.rs, against QuickJS
```

`tests/embed.rs` (12 tests) covers: services come from the host, `call` settling with values and errors, `post` from another thread, host bindings and modules, host fs/net refusal, `poll` for hosts with their own loop, lifecycle errors as `Err` not panics, no kernel poll per queued immediate, hosts that survive uncaught errors and failed loads, a held loop sleeping until posted work, and a watchdog ending an overrunning script via `pulse()` plus `interrupt()`.

## Decisions and hazards

- Loop step order (Node's): expired timers, kernel `poll`, I/O events and worker results, posted tasks, `setImmediate`. Idle = no open handle, no armed timer, no outstanding pool job, no queued task.
- Fix (`runtime.rs`): with an immediate or posted task queued, every step used to make a zero-timeout kernel poll, one syscall per `setImmediate`. Now it polls only when `POLL_GAP` (250 us; a poll costs about 13 us on macOS) has passed. Dropped alternative: poll every Nth step, which bounds callbacks but not time.
- Extensions (`Runtime::start`): the host's manifest is checked against its extension set (`lunar-extension::check`, all messages joined), then each used extension's `bindings` are merged after the host's own, before the VM exists. The set comes from `Host::extensions`, so a worker's Runtime, created from its own host, gets instances of its own; host bindings registered with `register` are not installed in workers. An extension's modules are host modules (they may import `lunarstorm:internal`) and lie in `lunarstorm:<name>[/...]`; `ipc`, `worker` and `internal` are reserved names.
- A Runtime starts once; reload or isolation means a new Runtime (new VM instance). Field order of `Engine` is drop order: persistent handles, then context, then instance.
- `Host::exit` never returns, because `process.exit()` must not run another line of JS and not every engine can unwind out of a native call. A host that must survive runs the Runtime on a thread it can abandon, or overrides `uncaught` to return (default: log and `exit(1)`).
- `Host::PULSE` is an associated const so the two counter writes per JS entry compile out for hosts without a budget (the CLI host).
- `Arg` carries no engine handles: objects an export returns arrive as JSON `Arg::Text`.
- fs policy: `read`, `stat`, `readdir`, `exists` need `Access::Read`; everything else `Access::Write`; a rename needs write on both ends. One fs implementation (`fs::exec`) serves the `*Sync` API inline and the async API on the worker pool.
- Pool (`pool.rs`): threads start lazily, up to core count; a full bounded queue parks jobs in a reactor-side overflow queue, never blocking and never dropping.
- I/O (`io.rs`): one slab for all handles (key = mio token = JS handle id); sockets registered once for `READABLE | WRITABLE` (edge-triggered); closed slots stay tombstoned (`Entry::Closed`) until the close event is delivered so stale events cannot hit a reused key; stdout is buffered once per loop turn.
- Modules: bare package specifiers (`left-pad`) are rejected, there is no `node_modules` lookup. Module files load synchronously (`Load::Ready` always).
- Ops read arguments leniently (a missing or mistyped number is 0): the only caller is the glue.
- `src/js/bootstrap.js` mirrors Rust tables by hand: event codes in `runtime.rs` (`mod ev`), `FS_*` in `fs.rs`, `METHODS` in `crates/http1`. Change both sides together; its comment still points at `src/http.rs`.
