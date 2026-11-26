# Hosts, bindings, extensions, IPC and workers

How a program the user starts owns a Runtime and gives it capabilities. The concepts doc
[embedding-a-host.md](embedding-a-host.md) and the extension design [extension-manifests.md](extension-manifests.md) stay the
long-form explanation. This file is how the code does it.

## The Host trait (`runtime/event-loop-runtime/src/host_services_trait.rs`)

`Runtime<V, H: Host>` calls up only through `H`, and it touches the process directly nowhere
else. `H` is a type parameter, so a host's answers are inlined into the loop.

| item | default | used for |
|---|---|---|
| `const PULSE` | `false` | compiles the two pulse writes per JS entry in or out |
| `const IPC` | `false` | whether `lunarstorm:ipc` resolves at all |
| `const WORKERS` | `false` | whether `lunarstorm:worker` resolves; needs `worker_host` |
| `type Extensions<V>` + `extensions()` | required | the extension tuple, built fresh per Runtime |
| `manifest()` | empty | which extensions and plugins are active |
| `args`, `environment`, `working_directory` | required | `process.argv`, `process.env` (read lazily), base for `[eval]` and relative paths |
| `write(Stream, bytes)` | required | stdout once per loop turn; stderr unbuffered |
| `exit(code) -> !` | required | must not return: `process.exit()` must not run another line of JS |
| `allow(Access)` | required | the fs/net policy: `Read`, `Write`, `Listen`, `Connect`; refusal → `EACCES` |
| `monotonic`, `wall`, `entropy` | clocks have defaults | timers, `performance.now`, HTTP `date`, `crypto.getRandomValues` |
| `log(Log)` | required | warnings, fatal errors, uncaught reports |
| `uncaught(in_promise, text)` | log + `exit(1)` | a host that survives script errors returns instead |
| `worker_host()` | `None` | factory, called on the new thread, for a worker's host |
| `ipc_send(bytes)` | no-op | receives `lunarstorm:ipc` `send` |

There is one fs and one net implementation, the Runtime's. A host does not replace them; it
only decides what they may touch.

## The two production hosts

| | CLI (`platform/cli/host`, `CliHost`) | desktop backend (`platform/desktop/host`, `DesktopHost`) |
|---|---|---|
| owner | `lunar run` / `lunar debug` / `lunar jbn build` (`command.rs`) | `Backend<S: Shell>` on one backend thread, driven by `platform/desktop/app` |
| loop | `runtime.run()` | `hold(true)`; events are posted tasks; reload/stop over a control channel |
| policy | allow everything | fs only under `Program::roots`; `Listen` and `Connect` refused |
| uncaught | framed report on stderr, `exit(1)` | `Shell::error(epoch, text)`, keeps going |
| `PULSE` | false | true: a `lunarstorm-budget` watchdog thread interrupts an entry that runs past `Program::budget` |
| extensions | `(HostApi<V, (Ui,)>,)` with the embedded `manifest.json` | `()` |
| host modules | `lunarstorm:ui` (alias of `lunarstorm:hostapi/ui`) | `@desktop/bridge`, `@desktop/api` (native, `src/desktop_bridge_natives.rs`) |
| IPC / workers | off | off |
| isolation | one Runtime per process | **one Runtime per epoch**: a reload is a new Runtime (new heap, jobs, timers) |

**No product host turns on `lunarstorm:ipc` or `lunarstorm:worker`.** Only the tests
(`runtime/event-loop-runtime/tests/{ipc,worker,extensions}.rs`) and `runtime/event-loop-runtime/examples/ipc-demo.rs` set
`IPC`/`WORKERS`. The desktop backend talks to its process through two native modules, `@desktop/bridge` and
`@desktop/api` (`src/desktop_bridge_natives.rs`); nothing is added to `ops`. Their state (handler,
outbox, calls in flight) is one `thread_local!` per backend thread, cleared before the instance goes,
and the backend thread calls `dispatch`/`settle` there directly.

## Bindings and `ops` (`host_bindings_registry.rs`, `internal_natives.rs`)

`ops` is one plain JS object holding:
- the Runtime natives (`ops::install`, internal_natives.rs): process, scheduling, text, tcp, http, fs;
- the constants `argv`, `pid`, `version`, `platform`, `arch`;
- everything registered through `Bindings`.

The native builtins read it at start. Host modules reach it through `lunarstorm:internal`.
Programs never reach it.

| `Bindings` call | effect |
|---|---|
| `function(name, f)` | `ops.<name>` = a native closure `Fn(&Call<V>) -> Outcome<Value>` |
| `values(f)` | arbitrary writes on `ops` at start (the CLI's `ops.color`, `ops.tty`) |
| `module(specifier, source)` | a host module, compiled on first import |
| `merge(owner, other)` | adds an extension's set; modules must lie in `lunarstorm:<owner>[/...]`; refuses a module or function already bound |

Installers run once, at `start`, *before* the builtins: `console` reads `ops.color` while it is
built.

The call path from a program into host Rust is: program →
`import { f } from 'lunarstorm:x'` (a JS module) → `ops.x_f(...)` (a property load on a hidden
object) → `Native<F>` adapter (args copied to a stack array) → closure. Core native modules
(`lunarstorm:ipc`, `lunarstorm:worker`, builtins) skip the JS wrapper module: their exports
*are* the natives. An extension cannot declare a native module today.

**Hazard:** `merge` checks names only against other `Bindings` functions. It does not see the
Runtime's own `ops` names (`print`, `now`, `fs`, ...) or names written through `values`.
`ops::install` runs first and the installers run after it, so an extension function named `now`
silently replaces `ops.now`. Prefix extension ops with the extension name.

## Extensions (`extension_manifest_check.rs`, `extensions/core`, `extensions/hostapi`)

```
Runtime::start
  host.extensions::<V>()                       fresh tuple for this Runtime (workers get their own)
  check(host.manifest(), offers)               every manifest error, joined; no VM exists yet
  ExtensionSet::install                        for each member the manifest uses, in tuple order:
      member.bindings(&Use) → merge(NAME, ..)  errors prefixed "extension <NAME>:"
  bindings.take() → ops installers + host module table
  V::Instance::new(limits, Loader) → context → ops::install → installers → node_builtins::install
```

- `Extension<V>`: `const NAME` (`[a-z][a-z0-9]{0,31}`, not `ipc`/`worker`/`internal`),
  `plugin_hosts()`, and `bindings(&Use)`.
- `ExtensionSet` is implemented for `()` and tuples up to 8, so it is static like everything else.
- An unused extension installs nothing, so its specifiers do not resolve.
- HostApi (`extensions/hostapi`) is the extension through which a host exposes its own API as
  plugins ("areas"), fenced to `lunarstorm:hostapi/<area>`. The CLI's terminal UI is area `ui`
  (`platform/cli/host/src/terminal_ui_area.rs`).
- `Runtime::register` bindings are **not** installed in workers. A worker gets only what its
  host's `extensions()` returns.

## `lunarstorm:ipc` (`ipc_module.rs`, `cross_thread_message_channel.rs`)

```js
import { send, onMessage } from 'lunarstorm:ipc';
onMessage((bytes) => send(bytes));   // Uint8Array both ways
```

- `send(Uint8Array)` calls `Host::ipc_send` synchronously on the loop thread. Anything else
  throws a `TypeError`.
- The host posts with `runtime.inbox().post(Vec<u8>)` from any thread. A `true` return means the
  message will be delivered unless the program ends first.
- Bounds per channel: 4096 unread messages, 64 MiB unread, 8 MiB per message. A post past a
  bound is refused and nothing is queued; the refusal is the back-pressure.
- The receiver is a `Persistent` in Rust, called directly by the loop in phase 4 (no `dispatch`
  code). It holds one `refs`. `onMessage(null)` or `Inbox::close()` (once drained) releases it.
- A message posted before `onMessage` waits. It is delivered in a later turn, never from inside
  `onMessage`.
- Dropping the Runtime closes the channel, so `post` then returns `false`.

## `lunarstorm:worker` (`worker_threads_module.rs`)

```js
import { spawn } from 'lunarstorm:worker';
const w = spawn('./job.js', { maxMemory: 64 << 20 });   // path relative to the entry module's directory
w.onMessage((data) => ...); w.postMessage({ n: 1 }); await w.exited;   // {state:'fulfilled'|'terminated'} or rejects WorkerError
// in job.js: import { onMessage, postMessage, close } from 'lunarstorm:worker';
```

- A worker is a full `Runtime<V, H>` on its own thread, built from `Host::worker_host()`, with a
  heap limit (default 256 MiB). Its entry path needs `Access::Read`.
- Messages cross as bytes: tag `0` + JSON text, or tag `1` + raw `Uint8Array`. There is no
  structured clone; a nested `Uint8Array` arrives as a JSON object.
- Each direction is a `Channel` with the IPC bounds, per worker. At most 64 active workers per
  Runtime.
- `exited` holds a `refs` on the parent until delivered, so the parent outlives its workers.
  All of a worker's messages are delivered before its exit.
- `terminate()` sets a flag the worker's loop checks every step, requests the engine `Interrupt`
  and wakes it. The resulting uncaught error is suppressed. Children end with their parent
  (`Workers::shutdown` in `Drop`/`stop`).
- `process.exit()` in a worker ends whatever its host's `exit` ends. A worker host should end
  only the thread.

## Writing a host (checklist)

1. Implement `Host`. Every method takes `&self`, so use interior mutability for state. State
   `type Extensions<V: Vm> = ();` explicitly, because associated type defaults are unstable.
2. `Runtime::<QuickJs, MyHost>::new(host)?`, then optionally `limits(...)` and
   `register(bindings)`, then `start()`, then `load(Source::File | Text | Package)`.
3. Drive it: `run()`, or `poll(Wait)` from your loop, or `hold(true)` + `post`.
4. If the host must survive script errors, override `uncaught` to return. To survive
   `process.exit`, run the Runtime on a thread `exit` can end.
5. Drop order: the Runtime drops its persistent handles, workers and IPC receiver before the VM.
   Keep any `Persistent` you hold (desktop: `HOOKS`) out of the instance's lifetime, or clear it
   first.
