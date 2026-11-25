# The VM contract and the QuickJS engine

```
runtime/core (Runtime<V, H>)
   │ calls down: Instance, Context, Scope          ▲ calls up: Callbacks (resolve, load, rejection, interrupt)
   ▼                                               │
runtime/vm            lunarstorm-vm          the contract + conformance suite
   ▲ implements
runtime/engines/quickjs   lunarstorm-vm-quickjs   the only adapter
   │ uses
runtime/engines/rquickjs  rquickjs (vendored, first-party)   Rust binding
   │ compiles
runtime/engines/rquickjs/sys/quickjs   patched quickjs-ng 0.17.0 (C)
   │ every JS compile is delegated to
runtime/compiler/crates/qjs-*          the Rust compiler (see compiler.md)
```

## The contract (`runtime/vm/src/lib.rs`)

`Vm` is a zero-sized marker with associated types. The Runtime is generic over it, so every
call is monomorphized and there is no `dyn` on any call path.

| type | meaning | QuickJS |
|---|---|---|
| `Instance` | heap, GC, job queue, limits, interrupt | `rquickjs::Runtime` (`JSRuntime`) |
| `Context` | one realm | `rquickjs::Context` with the full standard library |
| `Scope<'s>` | the entered context; creates, inspects, calls values | `rquickjs::Ctx<'s>` |
| `Value<'s>` | a scoped handle; cannot outlive its scope | `rquickjs::Value<'s>` |
| `Persistent` | `'static` handle; **must drop before its Instance** | `Persistent<Value<'static>>` |

Load-bearing rules:
- **`Thrown` is a marker.** A failing operation returns `Err(Thrown)`, and the exception stays
  in the engine until `Scope::exception()` takes it. Nothing is converted on the error path.
- **`Call<'s, V>`** (lib.rs:275) is two raw pointers into the adapter's stack frame: the scope and
  the args slice. It is a struct because a higher-ranked closure bound must name its lifetime in
  a parameter type.
- **`with_str` and `with_bytes` lend engine memory.** The closure must not call into the engine,
  which could move or detach the buffer.
- **Callbacks take `&self`.** A rejection can be reported while a module load is in progress.
  Callbacks get a scope, never the Runtime, so the engine cannot re-enter the Runtime.
- **A Runtime starts once.** Reload or isolation means a new Instance.

### Module answers (`Load`, lib.rs:250)

| answer | meaning |
|---|---|
| `Ready(ModuleSource)` | source text or an `Image`; the only form QuickJS supports for source |
| `Later(ticket)` | async load; needs `Capabilities::async_module_load`; QuickJS refuses it |
| `Native` | the callback already called `Scope::native_module(name, &[(export, value)])`; the exports are host-built values |

### `Image`: bytecode trust (lib.rs:198)

The engine does not validate bytecode, so an `Image` has private bytes and only two
constructors:
- `Image::from_package`, from a `.jbn` package that `Package::open` has verified (signature and
  every blob digest);
- `Image::compile`, which this process's engine wrote.

Before, `ModuleSource` carried a `bytecode: bool` that any caller could set. Do not add a third
constructor.

### Conformance suite (`runtime/vm/src/conformance.rs`)

`conformance::run::<V>()` returns how many check groups ran. The QuickJS test asserts **15**
(`runtime/engines/quickjs/tests/conformance.rs`). Adding a group means updating that number. The
vm and quickjs READMEs still say 13.

## The QuickJS adapter (`runtime/engines/quickjs/src/lib.rs`)

- **Identity** (lib.rs:94): `quickjs` / `quickjs-ng` / `0.17.0+lunarstorm.1` / contract 1.
  `engine::<V>()` in runtime.rs formats it, and a `.jbn` records it. **Bump `lunarstorm.N` on any
  engine edit that changes bytecode or opcodes**, or old packages will be misread instead of
  refused.
- **Capabilities**: memory limits, termination, bytecode cache, debug hooks. No JIT, no
  snapshots, no async module load. The README still says only the first two.
- **Native functions** (`Native<F>`, lib.rs:567) copy up to 8 arguments into a stack
  `MaybeUninit` array (`INLINE_ARGS`) and build a `Call`. Each copy is a refcount bump. More
  arguments heap-allocate.
- **`Scope::call`** goes straight to `JS_Call` with the arguments in a stack array. The binding's
  own builder heap-allocates above four arguments.
- **Native modules** (lib.rs:408-475). rquickjs's `ModuleDef` is a static trait (no `self`), so
  `native_module` parks the exports in context userdata (`NativePending`) under the module name.
  The one generic `NativeDef` declares and evaluates whatever is parked there. Evaluating twice
  is an error.
- **Engine hooks share one `Rc` of the callbacks**: the loader, the rejection tracker and the
  interrupt handler. The interrupt poll is `flag.take() || callbacks.interrupt() || any debugger
  poll()`.
- **Every context is wrapped by the debugger bridge at creation** (`Debugger::wrap`, lib.rs:254),
  even with no debugger attached. A session attaches to the wrapper later. The Rust compiler
  itself is installed earlier, by rquickjs on every context it creates
  (`rquickjs_core::compiler::install_base`, `core/src/context/base.rs:81`). The wrapper only
  layers its compile observer on top. The adapter's comment at lib.rs:120 ("the wrapper is what
  makes jetbunny's Rust compiler the context's compiler") predates that move.

Fixed defects documented at their sites:
- `number()` calls `JS_NewNumber`. The binding normalized `-0.0` to integer `+0`, so `1/-0` was
  `+Infinity`.
- `with_str` validates the lent bytes. QuickJS keeps an unpaired surrogate as WTF-8, which
  reached natives as an invalid `&str`; it now throws a `TypeError`.
- `take_out_of_memory` reads the patched engine's sticky OOM flag. The Runtime checks it in every
  `settle`, so a caught out-of-memory error still ends the program.

## The vendored engine (`runtime/engines/rquickjs/sys/quickjs`)

This is not upstream quickjs-ng. `REVISION` lists the base (`7b5f588`, 0.17.0) and every patch
identity, and `PATCHES.md` describes each one. Both are generated by `../scripts/provenance.sh`
and checked by `../tests/provenance.rs`.

| patch | what it changes |
|---|---|
| jetbunny-sticky-oom-v1 | owner-thread sticky OOM flag + `JS_JetbunnyTakeOutOfMemory` |
| jetbunny-modules-v3 | host-driven module graph: deferred resolution, prebinding, image loading |
| jetbunny-debugger-v1/v2 | `OP_debug_stmt` markers, breakpoints per marker, frame eval, debug hook (BC_VERSION changes) |
| **jetbunny-rust-compiler-v1** | **the C parser/compiler is removed**; a per-context compiler callback compiles every source and debugger `eval`; two appended opcodes `for_of_skip`, `strict_op` that only the Rust compiler emits |
| jetbunny-host-jobs-v1 | job sink: the host can own the promise job queue |
| jetbunny-async-context-v1 | a context slot carried across promise reactions (for AsyncLocalStorage) |
| eval-control, exception-info, ABI 11–14 | debugger internals (frame leases, scopes, async history, rejection checkpoints) |
| lunarstorm-user-catch-table | replaces jetbunny's `debug_catch` opcode; restores upstream's `OP_COUNT…255` dispatch range |
| lunarstorm-import-attributes | import attributes are written into module bytecode |
| lunarstorm-int32-constants | integer literals are written as `INT32`, not `FLOAT64` |
| lunarstorm-writer-out-of-memory | the bytecode writer refuses a truncated image after an allocation failure |
| lunarstorm-compiler-temporaries | (in `qjs-compiler-core/js_emit.rs`) temporary slots are reused, so verifier state no longer grows with the square of a function's length |

Consequences:
- **There is no C fallback compiler.** Every `Module::declare`, `eval` and `compile` goes through
  `rquickjs_core::compiler`, which is the Rust compiler installed as each context's *base
  compiler*. A debugger wrapper layers its observer on top. This is why compiling the 50 KB glue
  took about 150 ms in a debug build, and why it is cached as an image per process.
- **The opcode set is shared with Rust.** An opcode edit in C must be matched in
  `qjs-bytecode`'s verifier, then followed by a descriptor bump.
- **Every engine edit needs a provenance update**: the patch section, the `Sites` list (one
  entry per `lunarstorm patch` comment) and `REVISION`. `tests/provenance.rs` fails otherwise.
- `src/bindings/*.rs` were generated by bindgen for `aarch64-apple-darwin` and ported by hand to
  the other targets. The 0.17.0 header removed the `concat` atom, which shifted every later
  `JS_ATOM_*` number.

## Odd dependency direction

`lunarstorm-vm`, the engine-neutral contract, depends on `lunarstorm-debug`
(`default-features = false`) and `lunarstorm-jbn`. `Instance::debug` returns
`lunarstorm_debug::debug::VmDebugHandle`, and `Image::from_package` takes a `jbn::Package`. The
vm README still says the crate "has no dependencies". A second engine would have to speak this
debugger's protocol types.
