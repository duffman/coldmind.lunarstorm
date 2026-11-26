# Glossary: the words the runtime code uses

| term | means here | lives in |
|---|---|---|
| **Runtime** | one VM instance + its event loop + module loader + fs/net, on one thread; `Runtime<V, H>` | `core/src/runtime.rs` |
| **host** | the program that owns a Runtime and answers `Host` (stdio, exit, policy, clocks) | `core/src/host.rs`, `hosts/*` |
| **VM** / **engine** | the contract (`Vm` marker type) / the thing behind it (quickjs-ng) | `vm/src/lib.rs` |
| **adapter** | the crate implementing the contract for one engine | `engines/quickjs` |
| **Instance** | engine heap + GC + job queue + limits + interrupt (`JSRuntime`) | contract |
| **Context** | one realm (`JSContext`) | contract |
| **Scope** | the entered context; every value operation goes through it (`Ctx<'s>`) | contract |
| **Persistent** | a `'static` handle; must drop before the Instance | contract |
| **Thrown** / **Outcome** | marker "an exception is pending" / `Result<T, Thrown>` | contract |
| **Call** | one native invocation: scope + args slice, two raw pointers | `vm/src/lib.rs:275` |
| **Image** | trusted module bytecode: from a verified `.jbn` or from this engine's `compile` | `vm/src/lib.rs:198` |
| **Descriptor** / **engine identity** | `adapter engine version contract N`; a `.jbn` runs only on the same string | `runtime.rs:200` |
| **glue** / **bootstrap** | `core/src/js/bootstrap.js`: one function `(ops) => hooks`; the whole Node surface | `core/src/js` |
| **ops** | the hidden object of natives the glue is built on (Runtime + host + extension natives) | `core/src/ops.rs` |
| **hooks** | what the glue returns: `dispatch`, `ticks`, `describe`, `exitCode`, `invoke` + registry | `runtime.rs:294` |
| **dispatch** | the single JS entry for loop events; integer codes `ev::*` | `runtime.rs:151`, bootstrap.js:1344 |
| **registry** | the glue's `{modules, internal, exportsOf}`, from which builtins are declared on import | `modules.rs:64` |
| **builtin** | `node:events net http fs fs/promises path process buffer timers` | `modules.rs:44` |
| **host module** | an ES module registered with `Bindings::module` under an exact specifier | `bindings.rs` |
| **native module** | an ES module whose exports are host-built values (`Scope::native_module` + `Load::Native`) | contract, `quickjs/src/lib.rs:408` |
| **`lunarstorm:internal`** | native module exporting `ops` and `live`; importable by host modules only | `modules.rs:48` |
| **`live`** | the glue's live-region slot: lets the CLI UI keep a progress region below console output | bootstrap.js:542 |
| **Bindings** | a set of `ops` functions, `ops` value installers and host modules | `bindings.rs` |
| **extension** | a named `Bindings` provider selected by the host's manifest, fenced to `lunarstorm:<name>[/...]` | `core/src/extension.rs`, `extensions/core` |
| **plugin host** / **plugin** / **area** | a slot an extension opens / what fills it / HostApi's name for a plugin | `extensions/hostapi` |
| **manifest** | `{"extensions": {...}}`, which extensions/plugins are active; embedded per host | `hosts/cli/manifest.json` |
| **Turn** | the loop's per-scope-entry state; `Turn::step` is one loop iteration | `runtime.rs:980` |
| **entry** (JS entry) | one call from the loop into JS, bracketed by beat/settle | `runtime.rs` |
| **settle** | drain nextTicks and promise jobs until both empty, then report rejections | `runtime.rs:1175` |
| **deferred** | close/drain/resume events ops queue while JS runs, delivered after it returns | `io.rs:248` |
| **slab key** | id of a socket/listener: the mio token and the JS handle index | `io.rs` |
| **tombstone** | `Entry::Closed`: a slot reserved until JS hears the close | `io.rs` |
| **scratch** | the loop's one 64 KiB read buffer | `runtime.rs:137` |
| **pool** | lazy thread pool for fs and DNS; *not* `lunarstorm:worker` | `core/src/pool.rs` |
| **Poster** | the `Send` handle that posts closures to the loop | `runtime.rs:256` |
| **Nudge** | wakes a loop from another thread (flag + mio waker, coalesced) | `channel.rs:105` |
| **Channel** / **Inbox** | bounded byte queue between threads / the host's producer side of `lunarstorm:ipc` | `channel.rs` |
| **refs** | native references that keep the loop alive (ipc receiver, unreported worker exits) | `io.rs:330` |
| **hold** | host keeps an idle loop alive for posted work | `Runtime::hold` |
| **pulse** | counter, odd while JS runs; read by a budget watchdog | `Runtime::pulse`, `Host::PULSE` |
| **epoch** | one desktop backend instance; reload = new Runtime with the next epoch | `hosts/desktop` |
| **Shell** | what the desktop process implements for the backend thread | `hosts/desktop/src/lib.rs` |
| **worker** | `lunarstorm:worker`: a full Runtime on its own thread and VM | `core/src/worker.rs` |
| **facade** / **body** | a CommonJS file's `#esm` re-export module / the module holding its code | `modules/src/transform.rs` |
| **`.jbn`** | signed package: CBOR manifest + Ed25519 signature + bytecode blobs + import table | `runtime/jbn-package-format` |
| **trust store** | JSON of signer keys, the package names each may sign, and minimum releases | `jbn/src/trust.rs` |
| **base compiler** | the Rust compiler installed on every context by the patched engine | `engines/rquickjs/sys/quickjs/PATCHES.md` |
| **jetbunny** | the predecessor project much of this was ported from; the prefix of its engine patches and symbols | everywhere |
| **sticky OOM** | engine flag set by any allocation failure; the Runtime ends the program on it | `vm-and-engine.md` |
| **provenance** | `REVISION`/`PATCHES.md`/`Sites` ledger of every engine edit, checked by a test | `engines/rquickjs/sys/quickjs` |
