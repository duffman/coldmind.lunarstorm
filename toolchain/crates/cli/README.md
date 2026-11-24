# devtools-cli

The frontend tools as a package of the `lunarstorm` command line, over `devtools_server`,
`devtools_bundler` and `devtools_core`. Library only (`devtools_cli`); `apps/lunarstorm`
mounts it behind its `frontend` feature (on by default).

## Public surface

- `package() -> Box<dyn lunarstorm_cli_contract::Package>`: package id `frontend`.
- Commands (specs and handlers in `src/command.rs`):
  - `serve [root] [--host H] [--port P] [--define K=V]... [--no-watch]`: dev server with hot
    updates, runs until Ctrl-C; `--port` 0 selects a free port.
  - `bundle [root] [-o DIR] [--no-minify]`: production bundle; prints `source -> file` per
    entry, then the chunk count and time.
  - `preview [root] [-o DIR] [--port P]`: serves an existing bundle on `127.0.0.1` only, with
    SPA fallback to `index.html` for extensionless routes; default port 4173.
  - `transpile <file|-> [-o FILE] [--sourcemap] [--loader ts|tsx|js|jsx]`: one module to
    JavaScript; `--loader` only with stdin.
- Exit statuses: 0 success, 2 usage error (the host's parser, or a handler check such as
  `--define` without `=`), 1 operational failure with the full error chain on stderr.

## How it fits

Depends on `lunarstorm-cli-contract` (only), `devtools_core`, `devtools_server`,
`devtools_bundler`, axum and tokio. `apps/lunarstorm` depends on it. Each command runs on its
own multi-threaded tokio runtime (`execute` in `src/lib.rs`).

## Build and test

```bash
cargo build -p devtools-cli
cargo test -p lunarstorm --test frontend_commands --test frontend_transpile
```

The end-to-end tests live in `apps/lunarstorm/tests/` because they drive the real binary.

## Decisions and hazards

- Formerly the `jetbunny --serve | --build | --preview | --transpile` flags of a binary that
  did not survive the monorepo merge. The old parser is archived in
  `.archive/2026-11-24/toolchain/crates/cli/src/arguments.rs`.
- `--build` became `bundle`: `build` is the desktop pipeline's command on the same CLI.
- `transpile` compiles before creating output, never loads project config, leaves imports as
  specifiers, and writes files atomically instead of following an output symlink or
  mutating a hardlink inode.
- Only writes to our own stdout ignore `BrokenPipe`; a broken pipe from a subprocess stays a
  real failure.
- `preview` never builds or clears output, canonicalizes the directory, and limits path
  traversal lexically.
- Config: native `.devtools/config.toml` wins over Vite config; explicit options win over both.
