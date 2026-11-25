# Runtime and hosts

Lunarstorm is built from two kinds of parts:

- the **Runtime** executes JavaScript;
- a **host** is the program the user starts. It adapts the Runtime to one environment (a terminal, a
  desktop application, a device) and owns it.

There is one Runtime and one host per environment. This document describes both, the contract between
them, and how a host is put together. Extensions and plugins, the way a host adds capabilities, are in
`extensions.md`.

```
+------------------------------------ host process -------------------------------------+
|                                                                                       |
|  Host (impl Host)                                     manifest                        |
|   - environment: argv, env, cwd, stdio, exit           - extensions and plugins used  |
|   - policy: allow(Access)                              - permissions granted          |
|   - clocks, entropy                                                                    |
|   - extensions --+-- plugins                                                           |
|                                                                                       |
|  Runtime<V, H>  (one thread)                                                          |
|   - VM instance (engine V)        - event loop: timers, immediates, microtasks, I/O   |
|   - module loader                 - core modules: node:*, lunarstorm:ipc, ...worker   |
|   - worker Runtimes, each on its own thread with its own VM, host and extensions      |
+---------------------------------------------------------------------------------------+
```

## The Runtime

The Runtime is the execution engine shared by every Lunarstorm program:

| part | crate / path | does |
|---|---|---|
| VM contract | `lunarstorm-vm` (`runtime/vm`) | the engine-neutral interface: scopes, values, modules, bytecode images |
| engines | `runtime/engines/*` | implementations of the contract (QuickJS-ng today) |
| compiler, debugger | `runtime/compiler`, `runtime/debug` | bytecode compilation and verification, the debug adapter |
| Runtime | `lunarstorm-runtime` (`runtime/core`) | one VM instance, its event loop, the module loader, fs and net, the core modules |

The core modules are part of the Runtime and available to every program the host enables them for:
`node:*` (`events`, `fs`, `net`, `http`, `path`, `process`, `buffer`, `timers`, ...), `lunarstorm:ipc`
(a byte channel to the embedding application) and `lunarstorm:worker` (worker threads).

`Runtime<V, H>` is generic over the engine `V: Vm` and the host `H: Host`. The host binds both at compile
time, so every call between host, Runtime and VM is static.

## A host

A host is a program built around one `Runtime<V, H>`. It supplies what depends on the environment:

- the process environment, the output streams, clocks and random bytes;
- the policy: what a script may read, write, listen on and connect to;
- the capabilities of its environment, as extensions and plugins, and the manifest that declares them
  (`extensions.md`);
- the thread that drives the event loop.

| host | crate | environment | supplies |
|---|---|---|---|
| CLI | `lunarstorm-host-cli` | terminal programs (`lunarstorm app.js`) | the process's argv, stdio and exit code; the terminal module `lunarstorm:ui`; the user's own rights as policy |
| desktop | `lunarstorm-host-desktop` | the backend of a desktop application | the `@desktop/bridge` and `@desktop/api` modules; backend reload by epoch; an execution budget enforced by a watchdog; fs limited to the application's roots; native calls, served by the desktop process `lunarstorm-platform` |

## The host contract

`trait Host` (`src/host.rs`) is the interface between a host and the Runtime. The host implements it; the
Runtime calls it, always on the loop thread.

| member | supplies |
|---|---|
| `args`, `environment`, `working_directory` | `process.argv`, `process.env`, the base of relative paths |
| `write(Stream, bytes)` | stdout and stderr; stdout arrives batched, once per loop turn |
| `log(Log)` | diagnostics |
| `exit(code) -> !` | the end of the program; `process.exit()` arrives here without running another line of JS |
| `uncaught(in_promise, description)` | the handling of an uncaught error; the default logs it and calls `exit(1)` |
| `allow(Access) -> bool` | the fs and net policy: `Read`, `Write`, `Listen`, `Connect`; a refusal reaches the script as `EACCES` |
| `monotonic`, `wall`, `entropy` | clocks and random bytes |
| `const PULSE` | the pulse counter, for a host that enforces an execution budget |
| `const IPC`, `ipc_send(bytes)` | `lunarstorm:ipc` and the receiving end of its `send` |
| `const WORKERS`, `worker_host()` | `lunarstorm:worker`, and a factory for the host of each new worker |
| `type Extensions<V>`, `extensions()`, `manifest()` | the host's extension set and the manifest that selects from it (`extensions.md`) |

The capability switches are associated constants: each host enables exactly the capabilities it uses, and
the ones it leaves off cost nothing at run time.

## How a host runs a program

The CLI host's `run` command (`runtime/hosts/cli/src/command.rs`) shows the full sequence:

```rust
let mut runtime = Runtime::<QuickJs, CliHost>::new(CliHost::new(argv))?;  // 1. host + Runtime
runtime.register(host::bindings());                                       // 2. host's own modules and natives
runtime.start()?;                                                         // 3. manifest, extensions, VM, glue
runtime.load(source)?;                                                    // 4. the program
runtime.run()                                                             // 5. the loop, until idle or exit
```

1. **Create.** `Runtime::new` takes the host by value; from then on the Runtime owns it.
2. **Register.** The host adds its own modules and native functions with `Bindings`. These are installed
   in this Runtime only.
3. **Start.** The Runtime checks the host's manifest against `Host::extensions`, merges the bindings of
   each used extension, creates the VM instance and installs its JS glue. The glue is compiled once per
   process and loaded from that image by every later Runtime, worker Runtimes included.
4. **Load.** The program's entry module is loaded and evaluated.
5. **Drive.** Two ways:
   - `run()`: the host gives its thread to the Runtime until the program is idle or exits (CLI host);
   - `poll(Wait)`: the host keeps its own loop and steps the Runtime; `hold(true)` keeps an idle Runtime
     alive for work posted later through `poster()` (desktop host, whose events and native results arrive
     from other threads).

## Dependencies and layout

Hosts depend on the Runtime; the Runtime's crates depend on the VM contract and the engines. This keeps
one Runtime usable by every host, including hosts written outside this repository. The hosts themselves
are used by `apps/lunarstorm` (CLI host) and `platform/desktop` (desktop host).

Target layout, with the hosts beside the Runtime:

```
runtime/          VM contract, engines, compiler, debugger, lunarstorm-runtime
hosts/cli/        lunarstorm-host-cli
hosts/desktop/    lunarstorm-host-desktop
```

The host crates are at `runtime/hosts/cli` and `runtime/hosts/desktop` today. Moving them updates the root
`Cargo.toml` (workspace members and header comment), the dependency paths in `apps/lunarstorm/Cargo.toml`
and `platform/desktop/Cargo.toml`, the relative paths inside both host crates, and the documents that name
`runtime/hosts/`.

## Isolation and workers

One `Runtime` is one VM instance with one event loop on one thread. A worker is a further `Runtime` with
its own VM, loop and thread, the model of Node's `worker_threads`:

1. A script calls `spawn` from `lunarstorm:worker`.
2. The Runtime asks its host for `worker_host()`, a factory, and calls it on the new thread; the worker's
   host can therefore hold values that are not `Send`.
3. The worker's Runtime starts with that host and runs the worker's entry module.
4. The two Runtimes exchange messages (JSON values and `Uint8Array`) through bounded channels.

Each Runtime holds its own state. A worker's Runtime starts with the worker's host, so the host's
extensions are installed again in it, as instances of their own. Bindings the host registers with
`register` belong to the Runtime they were registered on; a worker does not get them.

## Writing a host

1. Implement `Host` for the environment: its services, its policy, the capability constants it uses, and
   `worker_host` when it enables workers.
2. Bind the engine: `Runtime<QuickJs, MyHost>`.
3. State the extension set (`type Extensions<V: Vm> = ();` for none) and register the host's own modules
   before `Runtime::start`.
4. Ship the host's manifest and return it from `Host::manifest` (`extensions.md`).
5. Drive the loop with `run()`, or with `poll(Wait)` from the host's own loop.
