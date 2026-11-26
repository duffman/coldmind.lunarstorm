# Decisions, hazards and stale docs

## Decisions nobody wrote in one place (reconstructed from the code)

| they chose | over | because |
|---|---|---|
| generics everywhere: `Runtime<V: Vm, H: Host>`, `ExtensionSet` tuples, `Host::PULSE`/`IPC`/`WORKERS` consts | trait objects, runtime flags | every call across a boundary inlines; features a host does not use compile out |
| a hidden `ops` object + the Node surface as native classes (`node_builtins`) | a JS glue script, many small native modules or globals | nothing is compiled at start; `ops` natives stay tiny and lenient (only the builtins and host modules call them) |
| builtins as native modules declared from the builtins' registry | a `__lunarstorm` global, generated ES module text | nothing reachable except through `import`; the export list lives with each builtin |
| one `dispatch(kind, …)` entry with integer codes | a closure per event | no allocation or handle lookup per event (HTTP benchmark parity with node at 3 MiB RSS) |
| HTTP/1.1 parsed in Rust inside the reactor | `http` on top of `net` in JS | in-place parsing out of the scratch buffer, pipelining, corking |
| a new Runtime (new VM instance) per reload, worker and epoch | resetting state in place | isolation is the engine's heap boundary; nothing can leak across |
| `Host::exit` returns `!` | unwinding from a native call | not every engine can unwind out of a native; `process.exit` must not run another line |
| the Rust compiler as the only compiler, with a verifier | QuickJS's C parser | a verified image model shared with the debugger (statement markers, frame eval) and with `.jbn` packaging; inherited from jetbunny |
| `Image` with private bytes and two constructors | a `bytecode: bool` on `ModuleSource` | the engine does not validate bytecode |
| a `.jbn` as a closed world with an import table | reading `node_modules` at run time | a signed app cannot be redirected by files on disk |
| worker messages as tag byte + JSON or raw bytes | structured clone | the VM contract has no structured clone |
| bounded channels that refuse (`post` → `false`) | blocking or growing | the refusal is the back-pressure signal; memory is bounded per channel |
| CLI policy "allow everything" | sandboxing the CLI | a command-line program runs with the user's rights, as in Node; the desktop host is the sandboxed one |

## Hazard map: looks safe, isn't

| change | why it bites |
|---|---|
| reorder loop phases or move `settle` | Node's ordering is the contract; tests in `embed.rs` pin immediates/ticks order |
| add an HTTP method | `METHODS` in `crates/http1` and in `node_builtins/http_server.rs` must agree |
| add a builtin to `BUILTINS` without adding it to `modules` in `node_builtins.rs`, or the reverse | "builtin is unavailable" on import / the builtin can't be imported |
| name an extension op `now`, `print`, `fs`, ... | `Bindings::merge` doesn't see core ops or `values` names; the installer silently replaces the core native |
| keep a `Persistent` past `Runtime::stop` | handles must drop before the instance (`Engine` field order is drop order); the desktop `HOOKS` thread-local is cleared by hand |
| call into the engine inside `with_str`/`with_bytes` | the lent slice aliases the JS heap; the engine may move or detach it |
| edit the vendored C engine | needs a `PATCHES.md` section + `Sites` entry + `REVISION` (provenance test); if bytecode or opcodes change: bump `engine_version` `+lunarstorm.N` (old `.jbn` would be misread) **and** the Rust verifier tables |
| raise an `ImageLimits`/`EmissionLimits` value | limits may only be tightened; raising returns a `capacity:` error |
| upgrade OXC | the frontend is pinned `=0.157` and matches diagnostic strings |
| turn on `H::IPC`/`H::WORKERS` in a product host | no product host has run with them; `worker_host` must make `exit` end only the thread |
| `Runtime::register` bindings in a host that uses workers | they are not installed in workers; only `extensions()` reaches workers |
| change a file's `type` in `package.json` while running | `Modules` caches each file's kind for the Runtime's life |
| add a third way to build an `Image` | it would let unverified bytes reach `JS_ReadObject` |
| run `lunarstorm-debug` tests expecting statement markers in `.jbn` code | packaged bytecode is compiled without markers; it is not debuggable |
| a CommonJS stack trace showing `#esm` | the facade carries the suffix; the body keeps the file's own name on purpose |

## Docs that contradict the code (as of this writing)

| doc | says | code |
|---|---|---|
| `runtime/event-loop-runtime/README.md` | bare package specifiers rejected, no `node_modules` lookup | `runtime/module-sources` resolves `node_modules`, `exports`/`imports` |
| `runtime/event-loop-runtime/README.md` | `Source::{File, Text}`; `embed.rs` has 12 tests | `Source::Package` exists; `embed.rs` has 17 tests |
| `runtime/engine-contract/README.md` | "no dependencies"; `Load::{Ready, Later}`; `Limits { memory, stack }`; 13 checks | depends on `lunarstorm-debug` and `lunarstorm-jbn-package-format`; `Load::Native`; `gc_threshold`; 15 checks |
| `runtime/engine-quickjs-adapter/README.md` | capabilities: memory limits and termination only; "no debug hooks"; 13 checks | also `bytecode_cache` and `debug_hooks`; 15 checks |
| `worker_threads_module.rs` | "this Runtime has no packages" | `.jbn` packages and `node_modules` both exist |
| `quickjs/src/lib.rs:120` | the debugger wrapper makes the Rust compiler the context's compiler | rquickjs installs it on every context (`install_base`) |
| `runtime/compiler` docs and constants | see compiler.md, "Stale references" | |
| `runtime/debugger/README.md` and module headers | see debugger.md, "README drift" | |