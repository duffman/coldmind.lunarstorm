# Jetbunny local QuickJS-NG patch

Base: https://github.com/quickjs-ng/quickjs revision
`7b5f588e0f9b643e2996f291bfc861abeaceddc2` (0.17.0).

Patch identity: **jetbunny-sticky-oom-v1**.

`0001-jetbunny-sticky-oom.patch` changes only `quickjs.c` and `quickjs.h`.
It adds an owner-thread-local sticky boolean to `JSRuntime`, sets it at entry
to `JS_ThrowOutOfMemory`, and exports the nonallocating
`JS_JetbunnyTakeOutOfMemory(JSRuntime *)` take/reset operation. The runtime's
existing zero initialization initializes the flag to false. Recursive OOM also
sets it. No allocator, arena accounting, memory limit or exception semantics
change. The take/reset operation does not allocate, execute JavaScript or
modify the pending exception; call it on the runtime owner thread only.

QuickJS can fail to allocate the OOM Error itself and throw `null`. Inspection
of the exception value cannot then distinguish native OOM from a script's
`throw null`. The flag survives that fallback and any JavaScript catch until
the host observes it. The Rust adapter must consume it at each native result
boundary, including success paths, and retain the typed fault out of band.

`UPSTREAM-SHA256` records hashes of the unmodified upstream inputs.
`REVISION` records hashes of the applied build inputs and patch metadata.
For an upgrade, apply the patch to clean source with `patch -p1`, inspect the
upstream allocation/throw paths, rerun the hard-OOM/null-fallback regression and
bounded-heap failure sweeps, regenerate both manifests and the patch diff, and
include the patch identity in the adapter build fingerprint. The patched
vendored inputs must not be described as wholly unmodified upstream source.

## jetbunny-modules-v3

Private compile/read flags defer resolution to the host graph. Private helpers enumerate lossless imports, prebind edges, instantiate without evaluation, and rekey native bytecode origins. Recursive function creation now checks native stack bounds. The bridge serializes modules with source stripped and native positions retained. These APIs accept only owner-checked module values; they do not validate hostile bytecode. Patch: `0002-jetbunny-modules.patch`. Native/Rust tests are written but not yet executed on branch codex/modules-jbn.

The public upstream header is unchanged by the module patch. The guarded private header and included implementation isolate the extension. Every JSValue-based module helper checks its tag and membership in the supplied context before dereferencing it. Partial builtin creation is rolled back immediately. The reader retains one returned wrapper rather than the preceding duplicated reference. Private declarations are safe under repeated C/C++ inclusion. Native smoke tests cover invalid kinds, foreign contexts, prepare/link/evaluate separation and cross-context image loading; execution remains deferred to final qualification.

The private dynamic-import callback receives borrowed native strings, preserving all UTF-16 units without the upstream C-string callback conversion. A preallocated owner-local queue retains tokens until exact completion; oversize metadata rejects that import without discarding earlier valid requests.

## jetbunny-debugger-v1

No patch file: the changes were made in the vendored sources (`quickjs.c`, `quickjs.h`), each marked `jetbunny-debugger-v1`, and `REVISION` records their hashes.

- `OP_debug_stmt`: a statement marker emitted for code compiled with `JS_EVAL_FLAG_DEBUG`. The new opcode renumbers later opcodes, so `BC_VERSION` is `29 | 0x40`. It calls the debug hook only while `ctx->debug_flags` is set. Breakpoints are one bit per marker in the function's `debug_bp`, allocated on the first breakpoint and never serialized.
- Debug-compiled functions keep the bindings a direct eval would need at any statement and give every slot a live `var_ref`, so frame evaluation reads and writes the frame's own variables.
- `js_eval_internal_frame`: a direct eval bound to a selected frame instead of the current one.
- `JS_SetDebugHook` and raw frame/variable accessors (`quickjs.h`). No pause or display policy lives in C; the embedder decides.


Patch: `0003-jetbunny-async-context.patch` (`quickjs.c`, `quickjs.h`).

The platform owns async context (AsyncLocalStorage) propagation. At this revision the promise hook fires `BEFORE`/`AFTER` only around thenable jobs, not around `.then`/`await` reaction jobs, so it cannot carry a context. Instead, `JSRuntime` gains a context slot (`jetbunny_async_context`, unset meaning `undefined`). `perform_promise_then` captures the current value into each `JSPromiseReactionData`, `promise_reaction_job` restores it for the whole job (handler and derived-promise resolution), and `js_promise_resolve_thenable_job` restores the value captured when it was queued. `JS_JetbunnyGetAsyncContext`/`JS_JetbunnySetAsyncContext` are owner-thread-only. Until the platform first sets a value, jobs skip the swap. The embedder clears the slot before freeing the context. Microtasks queued by the bridge (`jbq_enqueue_job`) capture and restore the same slot.

## jetbunny-rust-compiler-v1

From the Rust compiler work (`crates/qjs-*`, merged in fb01aed). No patch file: the changes were made in the vendored sources, and `tests/compiler-contract/target.json` records their hashes.

- `quickjs-opcode.h`: two opcodes appended after the existing short opcodes, so every existing opcode ID is unchanged. `for_of_skip` (IteratorStep without reading the value) and `strict_op` (array store, delete, super store or global store with strict semantics inside a sloppy frame). Only the Rust compiler emits them; the Rust image verifier checks their stack effects.
- `quickjs.c`: the two opcodes in the interpreter; a per-context compiler callback (`JSJetbunnyCompiler`) that, when registered, compiles sources and selected-frame debugger `eval` in place of the C compiler; an owner-thread marker per context.
- `jetbunny-eval.h`/`.inc`: the private compiler-callback ABI and the synchronous frame leases debugger `eval` uses.
- `jetbunny-inspect.h`/`.inc`: read-only views of compiled functions and modules, for checking Rust-compiled output against the C compiler.

The jetbunny binary registers the Rust compiler callback for every compile; there is no C fallback.

The C parser, compiler and bytecode emitter are removed from `quickjs.c`: the 14 blocks that were guarded by `QJS_DISABLE_PARSER` and never built now sit verbatim, with their original line ranges, in `.redundant-legacy/vendor/quickjs-ng/quickjs-parser.c` (outside every build). `quickjs.c` preprocesses to the same code as before, apart from assert line numbers.
Programs and debugger frame evaluation use the Rust compiler path.

## jetbunny-debugger-v2

The debugger continuation keeps the original marker/bitmap mechanism and adds:

- Executed lexical-scope and statement metadata on each frame, shared by
  inspection and selected-frame evaluation. Scope teardown invalidates the
  metadata until a new verified marker, rather than guessing a caller scope.
- Native exception events before unwinding. Rust debug compilation emits
  `debug_catch` (appended opcode 255) with explicit user-catch versus cleanup
  metadata. The interpreter scans live handler slots, not the presence of any
  finally block. Existing opcode identities and bytecode version 93 remain.
- `strict_op` selector 6 saves per-finally exception-notification state; selector
  5 restores it while rethrowing the original exception. A nested caught throw
  in the finalizer therefore cannot produce a duplicate original event.
- Application exception/backtrace preservation across debugger hooks, explicit
  new-host-throw notification, and uncatchable stop.
- Non-executing object classification and a property-count upper bound, allowing
  the shim to reject oversized/exotic objects before allocating own-key lists.

Debugger transport, source policy, conditions, stepping and object lifetimes
remain in Rust. Cross-thread callers publish atomic control metadata only;
native frame/value operations remain on the owner thread. No Hermes VM, GC or
inspector implementation is linked into the QuickJS adapter.

The changes are maintained in the vendored sources, not claimed as an upstream
release or a separately reproducible patch file. `REVISION` and the compiler
contract manifest record the exact changed inputs. Root build fingerprints also
hash the native shim and Rust compiler sources, invalidating old artifacts.

## jetbunny-test262-vm-v1

Patch: `0004-jetbunny-test262-vm.patch` (`quickjs.c`). The VM half of the drafted `jetbunny-test262-v1`: Test262 failures listed in upstream's `test262_errors.txt` at this revision whose cause is in the interpreter, builtins or module linking. Their parser and compiler half goes to the Rust compiler (`crates/qjs-*`), which replaces the C compiler. Each change is marked `jetbunny-test262-vm-v1`.

- **Canonical numeric strings.** `"NaN"` is a canonical numeric string for typed array keys.
- **`with` bindings.** GetBindingValue and SetMutableBinding of an object environment check `HasProperty` first, so a binding deleted after its reference was made reads `undefined` (or throws in strict code) and is written as a new property.
- **Call assignment targets.** `OP_throw_error` type 5 throws `ReferenceError: invalid assignment left-hand side`. The compiler emits it for a call used as a sloppy-mode assignment target (Annex B).
- **AsyncFromSyncIterator.** For `next` and `throw` (closeOnRejection), the sync iterator is closed when its result's value rejects or `PromiseResolve` of it throws.
- **`for await` closing.** The exception unwinder does not close the loop's iterator when calling `next()`, awaiting its result or reading `done`/`value` throws. It recognises that step by its pc on `await; iterator_get_value_done`, a pair only the loop's `next` step emits; a compiler must keep emitting it. A throw from the loop body still closes the iterator.
- **ResolveExport.** A local export of a named import resolves to the imported binding. Star-export ambiguity compares bindings, so `export * as x from M` and `import * as x from M; export { x }` name the same namespace.
- **Top-level await.** A module executed synchronously after its async dependencies sets `[[AsyncEvaluationOrder]]` to done, so a graph imported later does not wait on it. A rejected async module rejects its own capability before its ancestors'.
- **Bytecode builtins.** `js_bytecode_eval` frees its arguments when reading or evaluating the builtin fails; they leaked and the runtime aborted at teardown.

`builtin-array-fromasync.h`, `builtin-iterator-zip.h` and `builtin-iterator-zip-keyed.h` are regenerated from upstream's `.js` sources at this revision with the patched compiler (`JS_WRITE_OBJ_BYTECODE | JS_WRITE_OBJ_STRIP_SOURCE | JS_WRITE_OBJ_STRIP_DEBUG`, file name `builtin-*.js`). The upstream headers are bytecode version 29, which `jetbunny-debugger-v1` (version `29 | 0x40`) rejects, so `Array.fromAsync`, `Iterator.zip` and `Iterator.zipKeyed` threw `invalid version`. The regenerated bytes differ from upstream's only in the 5-byte header and in opcodes from `nop` on, each one higher.

Verification: `tests/quickjs_ng_conformance.rs`, run through the real binary. 7 of its 8 tests fail on the unpatched engine; the async-module rejection order test passes on both.

### Resumable debugger inspection (G002)

The debugger-v2 substrate now exposes raw own-property and selected-frame scope
pages (`JS_DebugPropertyPage`, `JS_DebugScopePage`) plus `JS_DebugCopyString`.
Pages walk dense array indices followed by native shape slots, or lexical chain,
arguments, function locals, and closure slots. This deterministic storage order
is the new Inspect contract; the legacy Variables ordering remains unchanged.
Deleted, private, and scope-filtered slots consume the scan budget. A stoppable
visitor leaves an entry pending when the owned reply budget is full. Cursors
store offsets or a collection-list link, require a retained root/current paused activation, and must
be invalidated after evaluation/editing and on resume/detach. Scope/value walks
never call ownKeys, property getters, exotic object methods, or lazy AUTOINIT
initializers; accessors and uninitialized lazy properties are explicit metadata.
Unsupported exotic values remain opaque. The string helper reads only the
requested UTF-16 prefix directly from native string storage, avoiding a full
string copy even for enormous values. Exact source hashes in REVISION and the
compiler target manifest distinguish these compatible debugger-v2 additions.

Inspection page entry points save and restore the pending application exception
without rethrowing it. Numeric atom-name and array-index allocation failures
return an explicit capacity status; the inspection-created exception is consumed
inside that boundary, and the original throw remains available on resume.

Initial item offsets are carried as a remaining-skip count in the native cursor:
filtered, deleted and skipped slots consume scan work, and skipped entries do
not copy names or retain values. Offsets larger than one scan budget can return
an empty unfinished page. Continuations require `Page.start=0`; an offset past
the end completes empty, with the total left unknown. Visitor status 2 similarly
filters an entry without counting it toward the emitted-item cap.

Bounded string copying handles flat strings, slices and rope strings separately.
Rope prefixes use a fixed depth-limited stack and a work budget proportional to
the requested prefix, returning explicit capacity failure on exhausted bounds;
the rope is never flattened. Property atom names are flat atom strings.

The rich inspection extension uses allocation-free fixed UTF-16 metadata for native
classes, bounded Symbol descriptions, BigInt limbs, function identity/arity/source,
and existing Error trace data. It never builds a stack trace or converts an object.
Small BigInts use bounded signed decimal; huge values expose labelled truncated
raw two's-complement hex and limb count. Empty and absent Symbol descriptions remain
distinct, including new Inspect property labels; legacy Variables labels are unchanged.
Map/Set cursors retain a rooted collection and pin their pending record; even a
deleted zombie record survives collection of an unreachable JavaScript iterator.
Cursor copies retain an independent pin so failed page construction preserves
the prior continuation. Cursor release unpins exactly once before dropping the root. The link must be
invalidated before any evaluation, resume or mutation. Each key/value
visit consumes the scan budget, without walking the already-consumed prefix.
Promise pages use only native state/result accessors and never execute jobs.
Buffer/view pages copy bounded bytes only after validating detachment and current
bounds. Shared backing data is never read; it remains explicitly opaque.

Length-tracking typed arrays use the native element count shifted by element size,
so an odd-length resizable buffer tail never exposes bytes outside the view.
Length-tracking DataViews remain byte-granular. Native function raw-name prefixes
are separately bounded so truncation cannot discard the native arity suffix.

Own-property inspection classifies both local-symbol and registered/global-symbol
atoms as Symbol keys. A Symbol.for description never aliases a same-text string
key in Inspect labels; raw native flags distinguish the two without getters.


## jetbunny-eval-control-v1 (private bridge ABI 7)

The debug-only `JS_JetbunnyDebugEvalControlled` boundary saves the pending
application exception, native trace, notification, interrupt counter and OOM
metadata. Its returned status and owned evaluator value are extracted before
restoring that state. Normal application direct eval retains its existing ABI.

A context-local active-operation latch is authoritative independently of the
allocation-free null exception carrier. Interrupt return codes 2/3 select
cancellation/deadline; application stop (1) overrides them. The outer debug
boundary alone clears the latch. Native host/compiler return and JS reentry
polls cannot preempt arbitrary native callbacks. Interpreter unwind bypasses
user catch/finally, iterator return, trace formatting, rejection conversion and
suppressed-error construction, while releasing native stack/frame resources.
Ordinary evaluator throws retain normal catch/finally semantics. Existing jobs
and prior writes remain; this boundary never pumps or clears a job queue.

G005 / jetbunny-binding-edit-v1 (private bridge ABI 8): inspection supplies exact
native slot/atom metadata; bounded owner-local edit targets root function/receiver
and duplicate atoms. Selected-frame leases validate visible mutable initialized
bindings; own writable data descriptors are re-found before each store. Synthetic
views, accessors, autoinit, proxy/exotic receivers and read-only descriptors refuse
implicit edits. Native store duplicates the actual RHS before releasing old value.

G006 / jetbunny-safe-lookup-v1 (private bridge ABI 9): bounded literal keys resolve
only verified active lexical/argument/local/capture bindings (first TDZ wins),
then own global lexical/object descriptors. Hidden `this` uses debug-retained
locals/captures; absent nondebug metadata refuses. Raw own data/VARREF and dense
array slots never invoke property machinery, prototypes, accessors or AUTOINIT.
Existing supported-class/exotic admission applies; typed/shared/raw byte views
remain unavailable. Lookup isolates and restores exception, trace, notification
and OOM state and never compiles, evaluates or invalidates inspection identities.
Parameter-initializer visibility follows the verified evaluator metadata in
lookup, scope inspection and exact target validation: body locals are excluded
until body scope is active; receiver and function-self pseudo bindings remain.
Lookup refuses ranked dynamic with/eval-variable environments before any outer
fallback, preserving nearer proven lexical bindings and keyword receiver lookup.

G006 split-environment proof uses existing four-bit variable metadata: kind12
marks only the proved arguments initializer, kind13 the anonymous body scope,
and kind14 a body scope with a distinct body arguments declaration. The Rust
verifier ties initialization, shadow state, scope links, capture provenance and
body activation to initialized parameter cells. Native scope paging accumulates
these flags during its budgeted chain walk; it never rescans all vardefs per row.
Parameter-created closures retain only then-visible bindings. Nested parameter
class scopes link to the parameter environment, never a future body anchor.
Split-parameter REPL/RHS evaluation remains deliberately unsupported. Existing
build/input fingerprints qualify the changed compiler representation; bytecode
container93/private ABI9 layouts and the evaluator pipeline are unchanged.

## jetbunny-exception-info-v1

The exception debugger hook exposes a duplicate of the original saved throw and
compiler-proved catch status through an owner-thread hook-only accessor. The
borrowed pointer is cleared before the hook returns; evaluator exceptions and
application pending exceptions remain separate. The accessor does not allocate,
format fields, invoke JavaScript, or change pending exception/trace state.

Own-data exception descriptions scan at most 4096 shape slots for each optional
Error field and copy at most 512 UTF-16 units in total. Cause is one raw preview,
not recursive traversal. Saturated raw string previews stop before computing
remaining capacity, including when consecutive Error fields are oversized.

## Distinct debugger function scopes (private bridge ABI 11)

Arguments/function-var bindings use JS_DEBUG_SCOPE_FUNCTION, distinct from
lexical-depth and closure scope identities. Scope paging and exact target
validation use the same identity, so DAP parent/name selection cannot alias
an inner lexical binding with an outer function binding. No evaluator lookup
or application scope semantics change.

## Continuation stepping (private bridge ABI 12)

Frames carry monotonic runtime-local 64-bit activation identities instead of
reusable addresses. Metadata-only return, yield and await notifications occur
before unlinking a frame and preserve pending exceptions. Allocation-free parent
lookup follows at most 256 Promise reactions / 32 levels, refuses ambiguous
awaiters, and never follows a suspended frame's stale physical caller links.

## Independent debugger async history (private bridge ABI 13)

Await frames capture copied scheduling metadata before unlinking. Promise
reactions retain that integer token; pending jobs save/restore a separate debugger
slot around execution. Ordinary Promise callbacks, thenables and bridge microtasks
capture their actual registration site. No JavaScript roots, application context
values, pending exceptions or job ordering are changed by history metadata.

## Checkpoint-qualified rejection tracking (private bridge ABI 14)

A separate owner-only rejection hook retains bounded debugger state without
consuming application raw-tracker events. Raw event storage is capped at 1024;
capacity loss is explicit rather than unbounded retention. The host qualifies
unhandled status only after its combined nextTick/native-job checkpoint drains.

Original rejection locations are captured before native frames unwind, carried
through await/reaction rejection and finally propagation, and retained as copied
metadata on Promise settlement. The existing internal one-slot finally state
packs the notification bit with an exact numeric history ID; checked history
issuance ends before 2^52, without ID reuse, allocation or bytecode stack growth.

Uncaught policy defers throws in async bodies and Promise handlers to checkpoint
qualification. All-exceptions policy still stops at the actual throw. Selecting
the independent unhandled-rejection filter does not enable synchronous throws.

## jetbunny-import-meta-v1

`JS_GetImportMeta` gives the `import.meta` object a `url` property: the module's name,
which is its jetbunny module identity.

## jetbunny-host-jobs-v1

The host owns the job queue. No patch file: the change is in the vendored `quickjs.c` and
`quickjs.h`, marked `jetbunny-host-jobs-v1`.

- `JS_JetbunnySetJobSink(rt, sink, opaque)`: while a sink is set, `JS_EnqueueJob` builds
  the job entry as before (duplicated arguments, debugger async-stack and rejection-history
  ids) and hands it to the sink instead of appending it to `rt->job_list`. Every job goes
  through it: promise reactions, thenable resolution, dynamic import, FinalizationRegistry
  callbacks and host microtasks.
- A sink returning `< 0` refuses the job: the entry is freed and `JS_EnqueueJob` throws
  `InternalError: promise job queue full` and returns -1. Upstream callers ignore that
  result, so a refused job is lost; the jetbunny adapter treats the refusal as fatal for
  the context.
- `JS_JetbunnyRunJob(rt, job)` runs and frees one job. `JS_ExecutePendingJob` uses the
  same body, so the debugger state each job restores is unchanged.
- `JS_JetbunnyFreeJob(rt, job)` releases a job without running it.

Without a sink, the upstream list path is unchanged. The bridge (`jbq_set_job_sink`,
`jbq_run_job`, `jbq_drop_job`) installs the adapter's sink. The adapter keeps the jobs
in creation order, up to `QuickJsNgConfig::max_pending_jobs`, and drains them at
checkpoints. It releases unrun jobs before freeing the context. `jbq_context_free` clears
the sink first, so jobs created during teardown stay in the runtime's list, which
`JS_FreeRuntime` frees. The private bridge ABI stays 14 because the additions are
additive. Tests: `tests/quickjs_ng_jobs.rs`.

# Lunarstorm delta

Everything above is jetbunny's `vendor/quickjs-ng/PATCHES.md` at `ced69b3`,
verbatim. `JETBUNNY-SHA256` lists the files here that are still byte-identical
to jetbunny's; `REVISION` hashes every file. Both are written by
`../scripts/provenance.sh` and checked by `../tests/provenance.rs`.

Where this tree differs from the text above:

- jetbunny's sections name patch files (`0001`–`0004-*.patch`). They are not
  carried here: every change is in the sources, marked with its identity.
- **The Rust compiler is the only compiler**, as in jetbunny. The compiler is
  jetbunny's `compiler.rs`, moved to `rquickjs_core::compiler`, because every
  context rquickjs creates needs it, not only the ones the debugger wraps. It
  is installed as each context's *base compiler* (`JS_JetbunnySetBaseCompiler`,
  `jbq_set_base_compiler`). A debugger wrapper installs the same compiler with
  its observer on top. Its reset (`JS_JetbunnySetCompiler(ctx, NULL, NULL)`)
  falls back to the base. `JS_FreeContext` frees the base compiler's bridge
  record. The reference runner (`runtime/compiler`) builds unmodified upstream
  from `runtime/compiler/vendor/quickjs-ng-upstream`.
- **`OP_COUNT ... 255` dispatch range restored** (upstream's). jetbunny dropped
  it when `debug_catch` filled the last id. Id 255 reaches `case_default` again
  instead of a null jump.
- **The user-catch table replaces `debug_catch`** (below).
- **Promise and module fixes** carried over from the previous vendored engine.
- **`jbq_context_wrap`** and its two helpers (`jbq_debug_poll`,
  `jbq_debug_rejection`) in `bridge.c`/`bridge.h`: the bridge around a context
  that rquickjs created and owns.
- jetbunny-sticky-oom-v1: Lunarstorm consumes the flag after every JS entry
  (`Scope::take_out_of_memory`, checked by the Runtime's settle step), so a
  caught out-of-memory error ends the program like an uncaught one.
- The rquickjs bindings (`../src/bindings/*.rs`) are generated by bindgen for
  `aarch64-apple-darwin` and ported to the other targets. The 0.17.0 header
  removed the `concat` atom, which shifts every later `JS_ATOM_*` number.

## lunarstorm-user-catch-table

jetbunny's `debug_catch` opcode (a `catch` plus a u16 user/cleanup flag) is
removed. It took id 255, the last free one-byte opcode id, while executing
exactly like `catch`; only the debugger read the flag. The flag is now debug
metadata:

- `JSFunctionBytecode.user_catch` / `user_catch_count`: handler targets (byte
  offsets) of user `catch` clauses, strictly ascending. `-1` means no table
  (a Rust compile without debug markers); there every handler
  counts as user code for `js_debug_handler_is_user` and none for
  `js_debug_frame_catches`, as jetbunny treated a plain `catch`.
- Both debugger lookups are a binary search over the table instead of a
  linear decode of the function per catch slot.
- Serialized as the first field of the debug section: `count + 1` (0 = no
  table), then each entry as a LEB128 delta. Stripped images carry nothing.
  The reader rejects unordered entries and offsets outside the code.
- `BC_VERSION` is 30 (byte 94); version-29 images are refused.
- Rust side: `FunctionImage::user_catches` in `qjs-bytecode-image-verifier` (format, reader,
  verifier: every entry must be a `catch` target of its function) and the
  emitter in `qjs-bytecode-emitter`, which always emits a plain 5-byte `catch`.

## lunarstorm-import-attributes

jetbunny's compiler dropped `with { … }` clauses, so rquickjs loaders never
received import attributes once the C parser was removed. Each module request
in the native image is now followed by a pair count and key/value atoms
(`js_write_module_attributes` / `js_read_module_attributes` in `quickjs.c`).
The reader builds the null-prototype object the C parser built and runs the
runtime's `module_check_attrs` where the parser ran it. Requests stay one per
specifier (jetbunny's identity); a second, different attribute set for the
same specifier is a compile error. `JS_JetbunnyModuleImport` ignores
attributes: Lunarstorm's host picks a module's kind from its file, and
jetbunny's "import attributes unsupported" rejection, unreachable before,
would now break those imports.

## lunarstorm-int32-constants

The Rust image writer emitted every number constant as `BC_TAG_FLOAT64`, so
integer literals loaded as float-tagged values (`1+1` had type Float, and
`as_int()` refused literal integers). Integer-valued constants in i32 range,
except -0, are now written as `BC_TAG_INT32`, as the C parser emitted them.

## lunarstorm-writer-out-of-memory

Upstream's and jetbunny's bytecode writer returned a truncated image as written
when an allocation failed: 13 callers drop `bc_put_atom`'s -1, so the atom was
left out of the stream, and `JS_WriteObject2` never read its buffer's
allocation flag. `bc_put_atom` now marks the buffer failed, and
`JS_WriteObject2` refuses a failed buffer, throwing out-of-memory unless an
allocation already threw. Test: `compiled_payload_under_a_memory_limit_is_refused_or_complete`.

## lunarstorm-compiler-temporaries

Not in this directory: `qjs-bytecode-emitter/src/js_emit.rs`. jetbunny's emitter
took a new local for every member assignment and every expression temporary
and never reused one, so verifier state (instructions x (locals + 1)) grew with
the square of a function's length: a script of 998 `o.pN=1;` statements failed
to load. One slot per function now serves `o[k] = v`, and `temporary` reuses
slots `clear_temporaries` has ended (debug builds assert that an ended slot is
never touched again). Tests: `long_flat_scripts_compile_without_a_local_per_statement`,
`long_script_of_member_assignments_runs`.

## Sites

Every `lunarstorm patch` comment in the sources, by file and line. The provenance
test checks that this list has one entry per comment.

- `bridge.c:32` set by jbq_context_wrap; the runtime's handlers belong to
- `bridge.c:40` the debugger half of `interrupted`, shared with the
- `bridge.c:187` the bridge ABI revision this library was built with.
- `bridge.c:194` the bridge around a context rquickjs created, for the debugger.
- `bridge.c:249` a wrapped context's runtime handlers are its owner's
- `bridge.c:784` install `compiler` as the base compiler of a context the
- `bridge.c:1030` the debugger half of track_rejection, for a wrapped context
- `quickjs.c:609` shape for promise resolving functions, with their
- `quickjs.c:657` the context's base compiler (JS_JetbunnySetBaseCompiler):
- `quickjs.c:955` handler targets (byte offsets) of user `catch` clauses,
- `quickjs.c:3149` the promise resolving functions' shape (see JSContext).
- `quickjs.c:3165` the base compiler's state belongs to the context.
- `quickjs.c:3238` see JSContext
- `quickjs.c:18371` upstream's default range, which jetbunny dropped
- `quickjs.c:19595` jetbunny's OP_debug_catch case is removed; the
- `quickjs.c:26284` user-catch table
- `quickjs.c:26589` 30 (was 29) for the user-catch table in the debug
- `quickjs.c:26734` 13 callers (upstream's and jetbunny's) ignore
- `quickjs.c:26954` user-catch table first in the debug section,
- `quickjs.c:26977` write import attributes (a null-prototype object of
- `quickjs.c:27026` each request's import attributes follow its
- `quickjs.c:27448` the writer never read the buffer's allocation flag, so
- `quickjs.c:27871` no table unless the debug section has one
- `quickjs.c:28049` user-catch table (see JS_WriteFunctionTag). Entries
- `quickjs.c:28125` read the attributes js_write_module_attributes wrote;
- `quickjs.c:28201` the request's import attributes.
- `quickjs.c:44065` every promise creates two of these functions.
- `quickjs.c:44245` resolving_funcs may be NULL when the caller settles
- `quickjs.c:44415` PromiseResolve(%Promise%, primitive), which is what
- `quickjs.c:47189` the same two properties, with the same flags and in
- `quickjs.c:53385` whether `target` is listed in the user-catch table.
- `quickjs.c:53406` reads the user-catch table instead of `debug_catch`
- `quickjs.c:53869` table lookup replaces the per-slot bytecode scan.
- `bridge.h:24` the bridge ABI revision; the Rust side (lunarstorm-debug ffi.rs
- `bridge.h:32` the bridge around a context its owner (rquickjs) created.
- `bridge.h:37` the debugger's share of the interrupt poll for a wrapped
- `bridge.h:40` forwards one rejection-tracker event of a wrapped context to
- `bridge.h:189` base compiler of a context the bridge does not own; see bridge.c.
- `jetbunny-eval.h:24` the context's base compiler; see jetbunny-eval.inc.
- `quickjs-opcode.h:393` jetbunny's `debug_catch` (a `catch` plus a u16 user/cleanup
- `jetbunny-eval.inc:53` a reset falls back to the base compiler, if any, so a
- `jetbunny-eval.inc:62` install the context's base compiler.
- `jetbunny-eval.inc:173` jetbunny installs every module image "prepared" (imports
- `jetbunny-modules.inc:26` import attributes now reach the module (rquickjs
