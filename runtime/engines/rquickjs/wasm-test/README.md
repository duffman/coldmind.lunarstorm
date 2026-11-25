# rquickjs-wasm-test (runtime/engines/rquickjs/wasm-test)

A smoke harness that compiles the rquickjs safe API for `wasm32-unknown-unknown` and drives it under Deno. `src/lib.rs` exposes a minimal C ABI (`rq_alloc`, `rq_dealloc`, `rq_eval`, `rq_free`, `rq_set_timeout_ms`) over one `Runtime` with a 64 MiB memory limit, a 256 KiB max stack and a wall-clock interrupt handler. `runner.ts` loads the `.wasm` and asserts that it imports nothing except `env.__rquickjs_host_now_us`, then runs a battery (language features, float formatting, Unicode, dates, promises/async drained after each eval, errors, isolation: no `fetch`/`Deno`/`require`) and the three containment limits: CPU (`while(true){}` interrupted in 100 ms to 2 s), memory (allocation bomb contained), stack (deep recursion gives `RangeError`), then that the engine is still alive.

- Not a workspace member: the root `Cargo.toml` lists it under `exclude`, and its own `[workspace]` table makes it a separate workspace. It builds only for `wasm32-unknown-unknown`. Features on `rquickjs`: `rust-alloc`, no default features; `[profile.release]` is `opt-level = "z"`, `lto = true`.
- Build and run (from this directory):
  `cargo build --manifest-path Cargo.toml --release --target wasm32-unknown-unknown`
  `deno run --allow-read runner.ts [path/to/rquickjs_wasm_test.wasm]`
  Exit status is 1 when any case fails.
- Hazard: `runner.ts` defaults to `./target/wasm32-unknown-unknown/release/rquickjs_wasm_test.wasm`, but this machine's `~/.cargo/config.toml` sets `target-dir = ~/.cargo-target`, so pass the wasm path as the first argument.
- Decision (`lib.rs`): `set_max_stack_size(256 KiB)` is mandatory on wasm. `JS_DEFAULT_STACK_SIZE` (1 MB) equals the wasm shadow stack, so the default `stack_top - 1MB` underflows. The harness drains pending jobs after every eval so promise continuations are observable.
- No Rust tests; the pass/fail output is `runner.ts`. Time-based checks (CPU interrupt window) can flake on a loaded machine.
