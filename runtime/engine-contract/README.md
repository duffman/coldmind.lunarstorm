# lunarstorm-engine-contract (runtime/engine-contract)

The VM interface of the Lunarstorm runtime: the contract a JavaScript engine adapter implements and the Runtime depends on, plus a conformance suite every adapter must pass. It is the lower boundary of doc/plans/main.md §5: the Runtime owns a VM (calls down) and implements `Callbacks` (calls up); it names no engine type. The crate has no dependencies and no engine.

## Public surface

- `trait Vm`: a marker type with associated types `Instance`, `Context`, `Scope<'s>`, `Value<'s>`, `Persistent`, plus `descriptor()` and `capabilities()`.
- `trait Instance<V>`: heap, GC, job queue, limits, interrupt; created with `Instance::new(&Limits, callbacks)`.
- `trait Context<V>`: one realm. `trait Scope<'s, V>`: the entered context handle; `Scope::local` turns a `Persistent` back into a scoped `Value`; `with_str` and `with_bytes` lend engine memory to a closure; `exception()` fetches a pending exception.
- `trait Callbacks<V>`: `resolve`, module load (`Load::{Ready, Later}`), unhandled-rejection report, `interrupt`; each receives a scope, never the Runtime.
- Data: `Descriptor { adapter, engine, engine_version, contract }`, `Capabilities` (`bytecode_cache`, `snapshots`, `jit`, `memory_limits`, `termination`, `debug_hooks`, `async_module_load`), `Limits { memory, stack }`, `Heap`, `Kind`, `ErrorKind`, `ModuleMeta`, `ModuleSource`.
- `Call<'s, V>`: one native-function invocation (`scope()`, `args()`); `Thrown`/`Outcome<T>`: failure marker; `Interrupt`: thread-safe termination request (`request()` / `take()`).
- `conformance::run::<V>() -> usize`: runs the checks and returns how many ran. `CONTRACT_VERSION = 1`.

## How it fits

- Implemented by `runtime/engine-quickjs-adapter` (the only adapter). Consumed by `runtime/event-loop-runtime`, which re-exports it as `lunarstorm_event_loop_runtime::vm`.
- Spec: doc/plans/main.md §5, "VM INTERFACE", "VM CALLBACKS", "Isolation", "Capabilities", "Callbacks".

## Build and test

```text
cargo build -p lunarstorm-engine-contract
cargo test -p lunarstorm-engine-contract          # no tests here: the suite needs an adapter
cargo test -p lunarstorm-engine-quickjs-adapter  # runs conformance::run::<QuickJs>() and expects 13 checks
```

A new adapter adds `#[test] fn conforms() { lunarstorm_engine_contract::conformance::run::<MyVm>(); }`. Checks: descriptor, values, borrowed data, objects, calls and exceptions, native functions, persistent handles, jobs, modules, rejections, host objects, limits, interrupts.

## Decisions and hazards

- Static dispatch only: `Vm` is a marker with associated types, the Runtime is monomorphized over it, there is no `dyn` anywhere in the contract.
- Two handle kinds: `Value<'s>` borrows its scope and cannot outlive it; `Persistent` is `'static` and **must be dropped before its `Instance`**.
- Exceptions stay in the engine: failing operations return the zero-sized `Thrown`; the value is fetched only on demand.
- `Call` is a struct holding two raw pointers into the adapter's stack frame, not two closure parameters, because a higher-ranked closure bound must name its lifetime in a parameter type and associated types alone (`V::Scope<'s>`) do not count. `Call::new` is `unsafe`: scope and args must outlive the value.
- Callbacks take `&self` (one may run while another is active, e.g. a rejection reported during a module load), so implementations use interior mutability.
- `Load::Later` only works when `Capabilities::async_module_load` is set; otherwise the callback must answer `Load::Ready`.
- `Interrupt::take` does a plain load before the swap so the unrequested case is one relaxed read. The interrupt error is uncatchable and clears the flag.
- Isolation and reload mean a new `Instance`; nothing is reset in place.
- The conformance suite is generic code compiled against the adapter, uses a fresh instance per check, panics with the check's name, and skips optional checks when `capabilities()` says absent. The adapter test asserts the count (13), so adding a check requires updating `runtime/engine-quickjs-adapter/tests/conformance.rs`.
