# lunarstorm-pipeline

The build pipeline after model extraction (spec 4b-4e): turns a model project into the
output directory the native host runs. It owns the `lunarstorm build` command package and
the standalone `lunarstorm-build` binary. Its own crate because the same library is
linked into the CLI and the debug runtime (spec 4).

## Public surface

- `build(project_root, out) -> Result<Project, Failure>`; `Failure` is `Model(Vec<Diagnostic>)`,
  `Backend(String)` (rolldown diagnostics) or `Io`. Writes `zebra.json` (host config, 4e/4d),
  `backend.js` (4c), `frontend/**` (4b) and a copy of `package.json`.
- `host::host_config` (`HOST_SCHEMA = 1`, `APPLICATION_OWNER`, `CONTEXT_POPUP`),
  `backend::generate` (`RUNTIME_MODULE = "@desktop/forms"`, `HOST_MODULES`),
  `frontend::generate`, `types` (writes `src/desktop.d.ts`).
- `command`: the `build` package for `lunarstorm-cli-contract`: `lunarstorm build [-p <dir>]
  [-o <dir>]` (output defaults to `<project>/dist`), `build types`, `build check`;
  `command::report` and `write_types` are shared with the binary.
- Binary `lunarstorm-build <project> <out>` and `lunarstorm-build --types <project>`.
  Exit 0 built, 1 compile errors or write failure, 2 usage.

## How it fits

Depends on `lunarstorm-model` and `lunarstorm-cli-contract` (`crates/cli-contract`);
bundles with `rolldown =1.3.1` on a tokio runtime. Mounted by `platform/cli/app` behind its
`build` feature. Its output is what `platform/desktop/app`'s `OutputDirPipeline` loads. It
embeds three files from elsewhere in the repo via `include_str!`: `framework/desktop/forms.js`
(backend runtime), `templates/zebra.framework/dist/vcl.js` (prebuilt React VCL; a missing
`dist/` breaks compilation) and `framework/vcl/react/src/vcl.css`. Spec: section 4.

## Build, test, run

```bash
cargo run -p lunarstorm-pipeline --bin lunarstorm-build -- platform/desktop/templates/app _build/out
cargo test -p lunarstorm-pipeline
```

Tests in `tests/`: `host.rs`, `backend.rs` (runs the bundled backend in a script harness),
`frontend.rs` (loads the built starter and checks its element tree), `types.rs` (type-checks
the starter with the real `tsc`, so it needs `tsc` on the PATH).

## Decisions and hazards

- Never a partial build: backend bundling, the only step that can fail on project code,
  runs before the first write, so a failing build leaves the previous output intact. The
  `frontend/` directory is removed and rewritten whole so a deleted form's module is not
  served stale.
- Fix (`backend.rs`): bundling work directories were named by pid and clock; two threads
  could get the same one and bundle each other's entry. Now a process-wide sequence number
  plus `create_dir` (which fails on an existing name). Test:
  `concurrent_builds_each_bundle_their_own_project`.
- Fix (`backend.rs`): a relative project root produced relative class paths in the entry
  and "Module not found"; the root is canonicalized in `generate`. Test:
  `the_cli_builds_a_project_given_by_a_relative_path`.
- Work directories live under `std::env::temp_dir()` and are removed on drop.
- Bridge tables list every prop and event of a control kind, not only the connected ones,
  so a write from the form class always reaches the WebView; a form's own `events`, menu
  and tray lists hold only connected ones. Output is deterministic (source order for props,
  control tables sorted by name).
- Form modules are plain JS calling `h(C.X, props, ...children)` with no per-project
  bundling, so a view edit regenerates one file and maps to `dev.refresh` of that form.
- `--project` is an option, not a positional, to avoid clashing with the subcommand names.
