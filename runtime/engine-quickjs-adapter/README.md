# lunarstorm-engine-quickjs-adapter (runtime/engine-quickjs-adapter)

The VM adapter for QuickJS: it implements the `lunarstorm-engine-contract` contract on top of the in-repo `rquickjs` binding (quickjs-ng C source), so the Runtime can run on QuickJS. It is the only engine adapter in the repo and the one place the contract meets an engine's types.

## Public surface

`QuickJs` (the zero-sized `Vm` marker you pass as `V` to `Runtime<V, H>`), `QuickInstance` (a `JSRuntime`, with `heap()`, `interrupt()`, GC), `QuickContext` (a `JSContext` with the full standard library). Descriptor: adapter `quickjs`, engine `quickjs-ng`, contract version 1.

| contract | QuickJS |
|---|---|
| `Instance` | `rquickjs::Runtime` |
| `Context` | `rquickjs::Context` |
| `Scope<'s>` | `rquickjs::Ctx<'s>` |
| `Value<'s>` | `rquickjs::Value<'s>` (ref-counted `JSValue`) |
| `Persistent` | `rquickjs::Persistent<Value>` |
| native function | a `JSCFunctionData` holding the closure |
| host object | an instance of one opaque class (`HostBox`) owning a `Box<dyn Any>`; dropping it is the finalizer |

Capabilities reported: `memory_limits` and `termination` only.

## How it fits

- Depends on `lunarstorm-engine-contract` and `runtime/vendor/rquickjs-binding` (feature `loader`; a first-party workspace member, so adapter and binding change together).
- Used by `runtime/host-command-line` and `runtime/host-desktop-backend`, which choose the engine (`Runtime<QuickJs, ...>`), and by `runtime/event-loop-runtime`'s tests.
- Spec: doc/plans/main.md §5, "VM ADAPTER A wraps ENGINE A".

## Build and test

```text
cargo build -p lunarstorm-engine-quickjs-adapter
cargo test -p lunarstorm-engine-quickjs-adapter     # tests/conformance.rs
```

`quickjs_conforms_to_the_vm_contract` runs `conformance::run::<QuickJs>()` and asserts exactly 13 checks. The suite lives in `runtime/engine-contract/src/engine_conformance_suite.rs`; extend it there and update the expected count here.

## Decisions and hazards

- Every trait method is a thin `#[inline]` wrapper, so with the Runtime monomorphized over `QuickJs` the abstraction compiles away.
- `Scope::call` calls `JS_Call` directly with arguments in a stack array; the binding's own argument builder goes to the heap above four arguments. Native function arguments use a stack array of `INLINE_ARGS = 8` slots and allocate only for longer calls. Strings and `Uint8Array`s are lent in place (`with_str`, `with_bytes`); the slice aliases the engine's heap and is valid only until JS runs again.
- Modules load synchronously: `Load::Later` is refused and `complete_load` throws "QuickJS loads modules synchronously".
- No JIT, no snapshots, no debug hooks. Bytecode serialization exists in the engine but is not exposed through the contract, so `bytecode_cache` is reported false.
- One `Rc` of the owner's callbacks is shared by the module loader, the rejection tracker and the interrupt handler; they take `&self`, so a rejection reported during a module load cannot conflict. The interrupt hook is `flag.take() || callbacks.interrupt()`.
- `HostBox` implements `JsLifetime` unsafely (it holds no JS values and no `'js` lifetime) and traces nothing: host data cannot hold scoped handles, persistent handles are roots of their own.
- A `Persistent` used with a different instance than it came from panics; that is a caller bug, not a runtime condition. Persistents must drop before the instance.
- The `unsafe` blocks each carry a `SAFETY:` comment (stack-slot initialization in native calls, `JS_Call`, pointer-as-identity for `identity()`, which is never dereferenced).
