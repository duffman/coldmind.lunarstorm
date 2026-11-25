# devtools-native

Shared source-transform engine, text in and text out: literal replacement, scope-aware
constant defines, and constructor-type dependency-injection metadata. One parse (oxc) serves
the define and inject stages. It does not transpile TypeScript or JSX and never reads files or
runs code. By default it is a plain Rust library for Rust and Tauri hosts; the `bun` feature
adds the napi/Bun `onBeforeParse` plugin (crate type `cdylib` + `rlib`).

## Public surface

- `Transformer` (`TransformState`): `Transformer::new(TransformOptions)` and
  `transform_source(..) -> TransformOutput { code, changed, parsed, replaced, defined,
  injected, warnings }`, drainable warning queue, `TransformStats` counters.
- `TransformOptions { replace: Option<Vec<ReplacePair>>, define: Option<Vec<DefinePair>>,
  inject_all: Option<bool> }`, `Loader::{Js, Jsx, Ts, Tsx}`.
- Feature `bun`: `devtools_transform` C-ABI symbol via `define_bun_plugin!("devtools")` and
  `createState` napi external; absent stages are off.

## How it fits

Consumed by `tauri-plugin-devtools` (without `bun`) and by the Bun adapter in
`toolchain/react/packages/bun` (`scripts/build-native.ts` builds the addon). Not wired into
the dev server or bundler. Deps: oxc crates pinned `=0.157.0`.

## Build and test

```bash
cargo test -p devtools-native
cargo build --profile release-node -p devtools-native --features bun
```

`tests/rust_api.rs` covers all stages, no-op identity, config validation, per-input
warnings, invalid source, TS/JSX preservation and thread safety. Unit tests sit in each stage.

## Decisions and hazards

- Order per file: replace (raw text), then define + inject over one parse of the stage-1
  text with merged edits. A stage whose cheap pre-check fails costs no parse. Syntax errors
  pass the file through (replace edits are kept: `preserves_replace_edits_when_source_is_invalid`).
- Bun runs only one `onBeforeParse` hook per file, hence all stages behind one symbol.
- Define replaces real references only: not strings, comments, property names, or locally
  declared bindings of the same name; an ambient `declare const X` still counts as global.
- Inject emits `static get $inject()` (a getter, not a field) so classes declared later or
  reached through circular imports are initialized when read. Selected classes: decorated, or
  with `static $injectable` (all classes in `inject_all`), with a constructor and no own
  `$inject`. Interface imports without `import type` are indistinguishable from classes.
- Warnings are capped at 1000 undrained per plugin.
- Build profile `release-node` unwinds on panic (root `Cargo.toml`): under `panic = "abort"`
  a panic in the addon would kill the host Bun/Node process.
- Bun builds link with `-undefined dynamic_lookup` (macOS) or `--unresolved-symbols=ignore-all`
  (Linux) so napi symbols resolve at load; unit tests never call them.
