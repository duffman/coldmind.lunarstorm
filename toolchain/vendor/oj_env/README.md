# oj_env

Environment handling for oj, from one immutable snapshot: oj's own `OJ_*` settings (`Knobs`) and
the app's environment as Vite resolves it (`AppEnv`: `.env` files by mode, `NODE_ENV`, `import.meta.env`
defines, `%KEY%` substitution in `index.html`).

Vendored from [lovablelabs/oj](https://github.com/lovablelabs/oj) (MIT, Copyright (c) 2026 Raphael Amorim and Lovable Labs Incorporated; license text in `../LICENSE`, SPDX headers kept in every source file). Version `0.2.29` is the workspace version. No diff against upstream was run for this README; nothing in the code marks a local change except that the manifest inherits `edition`/`license` from the root `[workspace.package]` and dependencies from `[workspace.dependencies]`.

## Surface
- `Env`, `init`, `get` (the snapshot), `Knobs` (cache, HMR gate/debounce, log-format and memory knobs),
  `AppEnv::resolve`, `mode`, `node_env`, `exposed_vars`, `import_meta_env_defines`.
- `dotenv::{load, load_with, parse}`, `html_env_map`, `replace_html_env`, `resolve_node_env`, `with_process_env`.

## Use in this repo
Only `oj_compiler` lists it, as a dev-dependency (its tests); no toolchain crate calls it. The dev
server and bundler build their own `import.meta.env` defines (`devtools_server::State::new`,
`devtools_bundler::build_defines`) and honor only `VITE_*` in the bundler.

## Test
`cargo test -p oj_env` (about 50 tests: `adverse.rs`, `properties.rs` with proptest).
