# devtools_core

Compile core and configuration contract of the toolchain's web side. It does two things:
compiles one file to one ESM module over the vendored `oj_compiler` (no bundling, imports
left as written), and loads project configuration (`.devtools/config.toml`, with an
optional Vite-config fallback) into one `ProjectConfig` that the dev server, the bundler
and the CLI all consume. Development serving and production bundling therefore read the
same settings.

## Public surface

- `Mode::{Development, Production}` with `options()` returning `oj_compiler::CompileOptions`
  (`dev()` adds React Refresh registration and HMR hot-context injection; `prod()` neither).
- `compile_source(path, source, &CompileOptions)`, `compile_file(path, &CompileOptions)`,
  `build_file(entry, out)` (production compile; writes `<out>` and `<out>.map` with a
  `sourceMappingURL` comment). Errors are `CoreError::{Read, Write, Compile}`.
- Re-exports: `CompileOptions`, `CompileOutput`, `JsxConfig`, `map_json_to_data_url`.
- `config` (alias `vite`): `ProjectConfig { root, define, alias, server_port, server_host,
  build_out_dir, build_minify }`, `load(root, command, mode)`, `load_preview(root, mode)`,
  `load_native(file, root)`; `ViteConfig` is a type alias of `ProjectConfig`.

## How it fits

Depends on `oj_compiler` (and `toml`, `serde_json`, `anyhow`, `thiserror`). Used by
`devtools_server`, `devtools_bundler` and `devtools-cli` (workspace deps in the root
`Cargo.toml`); `ls-build-server` coordinates those. `.devtools/config.toml` keys: `root`,
`[server] host port`, `[build] outDir minify`, `[define]`, `[resolve.alias]`; unknown keys are
errors (`unknown setting server.x`).

## Build and test

```bash
cargo test -p devtools_core
```

`tests/build_file.rs` (production output has no types or JSX; parse errors name the file) and
unit tests in `config.rs`. `built_entry_runs_in_bun` needs `bun` on PATH and fails without it.

## Decisions and hazards

- Native TOML wins over everything: `vite.config.*` is not even looked at when
  `.devtools/config.toml` exists, and the tests plant a throwing Vite config to prove it.
- A present-but-invalid native file is an error, never a fallback to defaults or to Vite.
- Alias replacements in TOML that start with `.`/`..` are resolved against the project root and
  canonicalized; `define` values are JS expressions (strings must carry their quotes).
- The Vite fallback executes JavaScript: it shells out to `bun --no-env-file --eval`, supports
  only `root`, `define`, `resolve.alias` (string entries, absolute replacements),
  `server.port/host`, `build.outDir/minify`, and warns on stderr for everything else. Vite
  plugin hooks are never run. It requires Bun on PATH; a missing Bun is an error, not a skip.
- `build.minify` from Vite only toggles minification; the requested minifier backend is ignored.
- `build_file` leaves imports as written, so its output runs only where the specifiers resolve.
