# oj_resolver

Module resolution for oj: `oxc_resolver` configured the way Vite resolves (extensions, mainFields,
conditions, aliases, tsconfig paths), plus Vite behaviors oxc lacks: `dedupe` and the exports-first
entry for a directory reached by path (Vite uses `exports["."]` before mainFields and never falls back
to mainFields once `exports` names an entry; Node and oxc bind `exports` only at a package-name boundary).

Vendored from [lovablelabs/oj](https://github.com/lovablelabs/oj) (MIT, Copyright (c) 2026 Raphael Amorim and Lovable Labs Incorporated; license text in `../LICENSE`, SPDX headers kept in every source file). Version `0.2.29` is the workspace version. No diff against upstream was run for this README; nothing in the code marks a local change except that the manifest inherits `edition`/`license` from the root `[workspace.package]` and dependencies from `[workspace.dependencies]`.

## Surface
- `OjResolver`: `new(root)`, `with_conditions`, `with_options(root, conditions, alias, dedupe)`,
  `with_settings(root, ResolveSettings)`, `require_variant`, `clear_cache`, `resolve(importer_dir, spec)`.
- `ResolveSettings { conditions, alias, dedupe, extensions, main_fields, preserve_symlinks, server }`
  (`server` drops `browser` from the default mainFields); `ResolveFailure` carries an `ignored` flag for
  `false`-mapped browser entries.
- `defaults`: `default_extensions`, `default_main_fields`, `default_server_main_fields`, `with_main_fallback`
  (oxc has no `main` fallback, so the list must end in `main`).

## Use in this repo
`devtools_server::State::resolve_import` (conditions `browser, import, module, development, default`);
the bundler resolves with rolldown instead.

## Test
`cargo test -p oj_resolver` (about 60 tests in `tests/adverse.rs` and the module tests; on-disk cases
in `fixtures/`: `dedupe`, `deprelative`, `dual`, `extensions`, `mainfields`, `playground`, `subpath`, `tsout`).
