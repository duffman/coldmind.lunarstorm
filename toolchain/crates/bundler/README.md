# devtools_bundler

Production frontend bundling for the web toolchain: `<root>/index.html` in, a static site out.
JS/TS/JSX goes through rolldown (one input per `<script type="module" src>`), CSS through the
app's own Tailwind 4 when it uses it, imported images and fonts become hashed files, `public/`
is copied. It is independent of any desktop host; `devtools_core` supplies configuration.

## Public surface

- `build(&BuildOptions) -> anyhow::Result<BuildReport>`: loads native config
  (`devtools_core::config::load`, mode `production` unless `options.production` is false),
  then `build_with_config(options, Option<ProjectConfig>)`, which never discovers JS config.
- `BuildOptions { root, out: Option<PathBuf>, minify: Option<bool>, production }`; precedence is
  explicit option, then project `build.outDir` / `build.minify`, then `dist` / `true`.
- `BuildReport { out, entries: Vec<(html src, built file)>, stylesheet, chunks }`.
- `desktop`: `BuildMode::{Development, Debug, Production}`, `DESKTOP_ENTRY =
  "build/generated/desktop-entry.tsx"`, `selected_shell(config, mode)` (config key `shell`, a
  specifier or an object keyed `default|development|debug|production`; default
  `@desktop/shell`), `prepare_desktop_frontend(generated, config, mode)`, which writes the
  generated React entry plus the shell sources and returns whether anything changed.

## How it fits

Depends on `devtools_core`, rolldown/rolldown_common/rolldown_plugin `=1.3.1`, tokio. Used by
`devtools-cli` (`lunarstorm bundle`), `ls-build-server` and, behind the `bundler` feature,
`tauri-plugin-devtools`. `desktop.rs` embeds three files with `include_str!` from
`toolchain/react/packages/shell/src/` and `desktop/desktop-entry.tsx`, so moving that
package breaks compilation. Not the model pipeline's bundler (that is `lunarstorm-pipeline`).

## Build and test

```bash
cargo test -p devtools_bundler
```

`tests/bundle.rs` (7 tests: TypeScript + styles + public assets, Vite config, native TOML with
explicit overrides, linked peer dependencies, missing dependency, output safety, per-directory
stylesheet assets). Tailwind cases need Node resolution from the fixture.

## Decisions and hazards

- The output directory is deleted before compilation. A later compile error leaves partial
  output; the build is not transactional (unlike `lunarstorm-pipeline`).
- The output path is canonicalized (`prospective_path`) before any removal and refused when the
  app root or project root lies inside it, so `outDir = "."` or a symlink cannot wipe the
  project (`configured_output_cannot_replace_project_or_app_root`).
- A missing browser dependency fails the build instead of leaving a bare import
  (`missing_browser_dependency_fails_instead_of_emitting_bare_import`).
- Linked source packages outside the app tree resolve peer dependencies from the app's
  `node_modules` after local resolution.
- `import.meta.env` defines come from mode plus every `VITE_*` process variable, sorted.
- Chunks are `assets/[name]-[hash].js`; asset extensions in `ASSET_EXTS` are emitted as files
  whose default export is the URL.
- `src/oj_inventory/` is staged snapshots of the vendored `oj_*` sources (MIT, Raphael Amorim
  and Lovable Labs). It is NOT a module of this crate and is never compiled; its README lists
  candidates for future plugin support. Vite plugin hooks are not executed.
