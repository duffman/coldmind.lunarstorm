# Glossary: the words the runtime code uses

| term | means here | lives in |
|---|---|---|
| **Runtime** | one VM instance + its event loop + module loader + fs/net, on one thread; `Runtime<V, H>` | `event-loop-runtime/src/runtime_instance.rs` |
| **host** | the program that owns a Runtime and answers `Host` (stdio, exit, policy, clocks) | `event-loop-runtime/src/host_services_trait.rs`, `platform/cli/host`, `platform/desktop/host` |
| **VM** / **engine** | the contract (`Vm` marker type) / the thing behind it (quickjs-ng) | `engine-contract/src/lib.rs` |
| **adapter** | the crate implementing the contract for one engine | `engines/quickjs` |
| **Instance** | engine heap + GC + job queue + limits + interrupt (`JSRuntime`) | contract |
| **Context** | one realm (`JSContext`) | contract |
| **Scope** | the entered context; every value operation goes through it (`Ctx<'s>`) | contract |
| **Persistent** | a `'static` handle; must drop before the Instance | contract |
| **Thrown** / **Outcome** | marker "an exception is pending" / `Result<T, Thrown>` | contract |
| **Call** | one native invocation: scope + args slice, two raw pointers | `engine-contract/src/native_call_and_class_spec.rs` |
| **Image** | trusted module bytecode: from a verified `.jbn` or from this engine's `compile` | `engine-contract/src/module_source_and_image.rs` |
| **Descriptor** / **engine identity** | `adapter engine version contract N`; a `.jbn` runs only on the same string | `runtime_public_types.rs` |
| **dispatch** / **hooks** | the natives the loop calls (`dispatch`, `invoke`, `describe`, `exitCode`); the Node surface is native, no JS glue | `event-loop-runtime/src/node_builtins/loop_event_dispatch.rs` |
| **ops** | the hidden object of the Runtime's natives, exported by `lunarstorm:internal` (Runtime + host + extension natives) | `event-loop-runtime/src/internal_natives.rs` |
| **hooks** | the natives `node_builtins::install` returns: `dispatch`, `ticks`, `describe`, `exitCode`, `invoke` + registry | `runtime_public_types.rs` |
| **dispatch** | the single JS entry for loop events; integer codes `ev::*` | `runtime_public_types.rs`, `node_builtins/loop_event_dispatch.rs` |
| **registry** | the natives' `{modules, internal, exportsOf}`, from which builtins are declared on import | `module_loader_callbacks.rs` |
| **builtin** | `node:events net http fs fs/promises path process buffer timers` | `module_loader_callbacks.rs` |
| **host module** | an ES module registered with `Bindings::module` under an exact specifier | `host_bindings_registry.rs` |
| **native module** | an ES module whose exports are host-built values (`Scope::native_module` + `Load::Native`) | contract, `quickjs/src/lib.rs:408` |
| **`lunarstorm:internal`** | native module exporting `ops` and `live`; importable by host modules only | `module_loader_callbacks.rs` |
| **`live`** | the console's live-region slot: lets the CLI UI keep a progress region below console output | `node_builtins/console_and_value_inspector.rs` |
| **Bindings** | a set of `ops` functions, `ops` value installers and host modules | `host_bindings_registry.rs` |
| **extension** | a named `Bindings` provider selected by the host's manifest, fenced to `lunarstorm:<name>[/...]` | `event-loop-runtime/src/extension_manifest_check.rs`, `extensions/core` |
| **plugin host** / **plugin** / **area** | a slot an extension opens / what fills it / HostApi's name for a plugin | `extensions/hostapi` |
| **manifest** | `{"extensions": {...}}`, which extensions/plugins are active; embedded per host | `platform/cli/host/manifest.json` |
| **Turn** | the loop's per-scope-entry state; `Turn::step` is one loop iteration | `event_loop/loop_turn.rs` |
| **entry** (JS entry) | one call from the loop into JS, bracketed by beat/settle | `event_loop/loop_turn.rs` |
| **settle** | drain nextTicks and promise jobs until both empty, then report rejections | `event_loop/loop_turn.rs` |
| **deferred** | close/drain/resume events ops queue while JS runs, delivered after it returns | `reactor_io_state.rs` |
| **slab key** | id of a socket/listener: the mio token and the JS handle index | `reactor_io_state.rs` |
| **tombstone** | `Entry::Closed`: a slot reserved until JS hears the close | `reactor_io_state.rs` |
| **scratch** | the loop's one 64 KiB read buffer | `event_loop/loop_turn.rs` |
| **pool** | lazy thread pool for fs and DNS; *not* `lunarstorm:worker` | `event-loop-runtime/src/blocking_work_pool.rs` |
| **Poster** | the `Send` handle that posts closures to the loop | `runtime_public_types.rs` |
| **Nudge** | wakes a loop from another thread (flag + mio waker, coalesced) | `cross_thread_message_channel.rs` |
| **Channel** / **Inbox** | bounded byte queue between threads / the host's producer side of `lunarstorm:ipc` | `cross_thread_message_channel.rs` |
| **refs** | native references that keep the loop alive (ipc receiver, unreported worker exits) | `reactor_io_state.rs` |
| **hold** | host keeps an idle loop alive for posted work | `Runtime::hold` |
| **pulse** | counter, odd while JS runs; read by a budget watchdog | `Runtime::pulse`, `Host::PULSE` |
| **epoch** | one desktop backend instance; reload = new Runtime with the next epoch | `platform/desktop/host` |
| **Shell** | what the desktop process implements for the backend thread | `platform/desktop/host/src/lib.rs` |
| **worker** | `lunarstorm:worker`: a full Runtime on its own thread and VM | `event-loop-runtime/src/worker_threads_module.rs` |
| **facade** / **body** | a CommonJS file's `#esm` re-export module / the module holding its code | `module-sources/src/load_time_esm_transforms.rs` |
| **`.jbn`** | signed package: CBOR manifest + Ed25519 signature + bytecode blobs + import table | `runtime/jbn-package-format` |
| **trust store** | JSON of signer keys, the package names each may sign, and minimum releases | `jbn-package-format/src/signer_trust_store.rs` |
| **base compiler** | the Rust compiler installed on every context by the patched engine | `vendor/rquickjs-binding/sys/quickjs/PATCHES.md` |
| **jetbunny** | the predecessor project much of this was ported from; the prefix of its engine patches and symbols | everywhere |
| **sticky OOM** | engine flag set by any allocation failure; the Runtime ends the program on it | `vm-and-engine.md` |
| **provenance** | `REVISION`/`PATCHES.md`/`Sites` ledger of every engine edit, checked by a test | `vendor/rquickjs-binding/sys/quickjs` |
