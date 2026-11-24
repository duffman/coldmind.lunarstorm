# oj_compiler

The fused per-file pipeline of oj: one TS, TSX, JSX or JS file in, one ESM module out, over oxc
(parse, TypeScript/JSX transform, define replacement, React Refresh, import-specifier rewriting,
source maps). It is the compiler of the web toolchain; the Lunarstorm model pipeline does not use it.

Vendored from [lovablelabs/oj](https://github.com/lovablelabs/oj) (MIT, Copyright (c) 2026 Raphael Amorim and Lovable Labs Incorporated; license text in `../LICENSE`, SPDX headers kept in every source file). Version `0.2.29` is the workspace version. No diff against upstream was run for this README; nothing in the code marks a local change except that the manifest inherits `edition`/`license` from the root `[workspace.package]` and dependencies from `[workspace.dependencies]`.

## Surface
- `compile`, `compile_module`, `compile_module_with_maps` with `CompileOptions::{dev, prod, standalone}`
  (`standalone` lowers syntax only: no define replacement, import rewriting, glob or HMR work),
  `CompileOutput`, `HotAccept`, `ImportRewriter`, `ImportMetaEnv`, `JsxConfig`, `COMPILE_STACK_SIZE`.
- Source scanning and edits: `imports`, `exports`, `specifier_edits`, `rewrite_specifiers`.
- Modules: `glob` (`import.meta.glob`, dynamic-import variables, `new URL` assets), `cjs` and `interop`
  (CommonJS), `assets`, `json`, `html`, `tsconfig`, `ssr`, `bundle` (factory-format output, not the production
  bundler; production bundling is rolldown in `devtools_bundler`).

## Use in this repo
Consumed by `devtools_core` (single-file compile) and `devtools_server` (on-demand serving). The
dev server runs it on a thread with `COMPILE_STACK_SIZE`; smaller stacks overflow on deep ASTs.
Unused snapshots of its sources sit in `toolchain/crates/bundler/src/oj_inventory/` (not compiled).

## Test
`cargo test -p oj_compiler` (about 230 tests: `adverse.rs`, `edge_cases.rs`, `env_defines.rs`,
`properties.rs` with proptest, `ssr_exec.rs`, `tsconfig_gap.rs`). Benchmarks: `examples/bench_compile.rs`,
`examples/bench_scan.rs` (`cargo run --release -p oj_compiler --example bench_scan`).
