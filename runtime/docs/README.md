# Lunarstorm runtime

A Node-compatible JavaScript runtime for embedding: QuickJS-ng behind a static VM contract, a
mio event loop with HTTP/1.1 parsed in Rust, Node-style modules (ESM, CommonJS, TypeScript,
`node_modules`), and signed bytecode packages (`.jbn`). It runs `lunar run` and `lunar debug`
on the command line, and the backend of every Lunarstorm desktop app. It serves I/O-bound work
at node's throughput with 2–3 MiB of RSS instead of 88. The price is 8–10x slower microtask
churn, because it is an interpreter.

| doc | read it when |
|---|---|
| [architecture.md](architecture.md) | first: crates, dependency direction, the five flows end to end |
| [event-loop.md](event-loop.md) | touching timers, I/O, HTTP, the pool, `settle`, or embedding the loop |
| [vm-and-engine.md](vm-and-engine.md) | touching the VM contract, the QuickJS adapter or the vendored C engine |
| [compiler.md](compiler.md) | anything that compiles JS: it is the only compiler there is |
| [modules.md](modules.md) | specifier resolution, builtins, CommonJS/TS, `.jbn` packages |
| [hosts.md](hosts.md) | writing a host, bindings, extensions, `lunarstorm:ipc`, `lunarstorm:worker` |
| [debugger.md](debugger.md) | `lunar debug`, DAP, breakpoints, source maps |
| [decisions-and-hazards.md](decisions-and-hazards.md) | before any change that "looks safe"; also lists docs that contradict the code |
| [glossary.md](glossary.md) | a word in the code means something specific here |

The concept docs `runtime/core/doc/hosts.md` and `runtime/core/doc/extensions.md` remain the
design rationale for hosts and extensions. Package READMEs cover build and test commands, and
several are stale (see decisions-and-hazards.md).

## Five minutes with a new hire

Start in `runtime/core/src/runtime.rs`. Everything else hangs off `Runtime<V: Vm, H: Host>`.
`V` is the engine and `H` is the program that owns the runtime. Both are type parameters, never
trait objects. In the whole repo there are only two places that choose them: the CLI
(`Runtime<QuickJs, CliHost>`) and the desktop backend (`type Engine = QuickJs`).

`start()` builds a VM instance and then runs one JavaScript function, `bootstrap.js`. That
50 KB file *is* Node: `console`, `process`, `Buffer`, timers, `events`, `net`, `http` and `fs`
are all JS built on a hidden object of Rust natives called `ops`. Programs never see `ops`.
Builtins reach them as native modules (`import fs from 'node:fs'`), and host APIs reach them as
host modules that may import `lunarstorm:internal`. Builtins have no global.

The loop has one turn per `step`: timers, poll, I/O, posted work, immediates. Every call into
JS is an *entry*. After each entry, `settle()` drains nextTicks and promise jobs, the same as
Node. JS sees every event through one `dispatch(kind, …)` function with integer codes. Those
codes are mirrored by hand between `runtime.rs` and `bootstrap.js`, and that is the first thing
you will break.

Below the contract, QuickJS is not stock. The C parser has been **removed**. Every source
string, from your app to `eval` to the debugger console, is compiled by a Rust compiler
(`runtime/compiler`, OXC-based) and then verified in Rust. Only then does C load the bytecode,
and C trusts it. So any edit to the C engine is three edits: C, the Rust verifier tables, and
the `+lunarstorm.N` engine version that `.jbn` packages check. On top of those, a provenance
test makes you record every patch site in `PATCHES.md`.

What will bite you:
- Turning on workers or IPC in a real host. Both are built and tested, but no product host has
  run with them.
- Naming an extension op `now`. It silently replaces the core one.
- Keeping a `Persistent` past `stop()`.
- Expecting `lunar debug` to accept DAP `launch`. Clients must `attach`.
- Trusting the package READMEs over the code.

## Delete test

**Delete `rquickjs_core::compiler` (with the `qjs-*` crates behind it), and the most breaks with
the least chance of a quick repair.** Every rquickjs context installs it as its base compiler.
Without it, no JavaScript is compiled anywhere: not the glue, not a CLI script, not a desktop
backend, not a debugger evaluate. Nothing falls back, because the C parser is gone (its source
only survives in `.redundant-legacy/`). Restoring the C parser would also break the debugger's
statement markers and the verifier-based trust model `Image` relies on.

Runner-up is `runtime/vm`. Its removal breaks compilation of core, the adapter and both hosts,
but that is a compile error with an obvious shape, not a silent loss of the language.
