# toolchain/react (`@coldmind/devtools-workspace`)

The pnpm package that drives the React side of the devtools toolchain: it owns the
cross-runtime integration runner, the native-binding build entry and two runnable examples.
It has no source of its own; the code lives in its two sub-packages and in the Rust crates
under `toolchain/crates/`. Product overview: `toolchain/README.md`.

| sub-package | name | what it is |
|---|---|---|
| [`packages/bun`](packages/bun/README.md) | `@coldmind/devtools` | Bun `onBeforeParse` plugin and Tauri client over the Rust transform crate (`toolchain/crates/transform`, `devtools-native`) |
| [`packages/shell`](packages/shell/README.md) | `@coldmind/devtools-shell` | page shell for apps on the dev server: one HMR/app WebSocket, error boundary, overlay |

## Layout

| path | role |
|---|---|
| `package.json` | scripts (below); dev dependencies on both sub-packages, React 19 and Tailwind 4 |
| `scripts/test-integration.ts` | the `pnpm test` runner: 9 sequential phases, per-phase deadline and process group, JSON report |
| `scripts/install-cli.sh` | builds the CLI and symlinks it into `$BIN_DIR` (default `~/.local/bin`) |
| `tsconfig.scripts.json` | typechecks `scripts/**/*.ts` against `@coldmind/config/tsconfig.base.json` |
| `examples/react-shell/` | Vite-style `index.html` + `startShell({ root, load })` + `App.tsx` using `useShellChannel` |
| `examples/bun-transform/` | `build.ts` runs `Bun.build` with the `devtools({ define, inject })` plugin on `app/entry.ts` |
| `test-results/integration.json` | report written by the runner; rewritten on every run, including failures |
| `res/logo.png` | logo used by `toolchain/README.md` |

## Commands

From the repo root (pnpm workspace member `toolchain/react`):

```bash
pnpm --filter @coldmind/devtools-workspace test          # = test:integration, all 9 phases
pnpm --filter @coldmind/devtools-workspace test:adapters # build:native, then bun and shell unit tests
pnpm --filter @coldmind/devtools-workspace typecheck     # bun + shell + scripts
pnpm --filter @coldmind/devtools-workspace check         # cargo clippy --workspace --all-targets, then typecheck
pnpm --filter @coldmind/devtools-workspace dev           # cargo run -p lunarstorm -- serve
pnpm --filter @coldmind/devtools-workspace build         # cargo build -p lunarstorm --release
cargo run -p lunarstorm -- serve toolchain/react/examples/react-shell
bun toolchain/react/examples/bun-transform/build.ts
```

The scripts call `bun run --cwd ...` and `bun scripts/...`; Bun and Node 22+ (native `fetch`
and `WebSocket`) must be on PATH even though pnpm is the package manager.

## Fit with the spec

Spec section 4 puts a React shell around the generated frontend (`<ShellDebug>` for dev,
`<Shell>` for release) and forwards `tsc` diagnostics as `dev.error`. This package tests
that stack end to end: Rust server to React shell over real HTTP and WebSocket
(phase 9, `packages/shell` `test:integration`, which needs `DEVTOOLS_TEST_CLI`).

## Hazards found in the code

- The CLI under test is the `lunarstorm` binary (`apps/lunarstorm`, feature `frontend`);
  the runner reads its path from Cargo's build messages and hands it to `packages/shell`
  as `DEVTOOLS_TEST_CLI`. The former `jetbunny`/`devtools` binary names no longer exist.
- The runner requires real Tailwind: `node_modules/{@tailwindcss/node,@tailwindcss/oxide,tailwindcss}`
  under this package (or `DEVTOOLS_TAILWIND_NODE_MODULES`). It fails instead of skipping.
  `test-results/integration.json` currently records that failure.
- Phase thresholds assert minimum Rust test counts (1, 9, 28, 14, 2); a phase that runs fewer
  tests fails even with exit code 0. `--ignored` is intentional in the Tailwind phase.
- Windows is refused (the Tailwind fixture needs Unix symlinks).
- `packages/bun/README.md` still refers to `packages/tools/devtools` as the product root and
  `bun run --filter`; both predate the monorepo layout.
- The `native/devtools.darwin-arm64.node` binding is a build product (git-ignored via `packages/bun/.gitignore`); `build:native` must
  run before the bun and shell tests (`test:adapters` does this).
- Env: `DEVTOOLS_TEST_REPORT` (report path), `DEVTOOLS_TEST_TIMEOUT_MS` (per phase, default 1200000).
