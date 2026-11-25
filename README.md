# Lunarstorm

Desktop applications built the Delphi way: per form a view resource (`*.view.tsx`, controls and
event wiring, parsed and never executed) and a form class (`*.ts`, handlers). The compiler
generates a React frontend, a JS backend and a host configuration; a generic Rust host (tao + wry)
opens the windows and runs the backend on an embedded QuickJS runtime. Specification:
[`doc/plans/main.md`](doc/plans/main.md). CLI reference: [`doc/cli.md`](doc/cli.md).

One Cargo workspace (`Cargo.toml`) and one pnpm workspace (`pnpm-workspace.yaml`) at this root.

| area | path | what |
|---|---|---|
| command line | [`apps/lunarstorm`](apps/lunarstorm) | the `lunarstorm` binary: `run`, `build`, `describe`, `completions` |
| shared crates | [`crates/`](crates) | `cliui`, `cli-contract`, `cli-host`, `http1` |
| runtime (spec §5) | [`runtime/`](runtime) | `vm` contract, `core` Runtime, `engines/` (QuickJS, rquickjs), `hosts/` (cli, desktop) |
| extensions | [`extensions/`](extensions) | `core` (`lunar-extension`: names, manifest, load-time checks), `hostapi` (a host's API areas as plugins) |
| platform (spec §6) | [`platform/`](platform) | desktop process, packager, browser host, example project |
| toolchain (spec §4) | [`toolchain/`](toolchain) | `model` + `pipeline` (the compiler), the older devtools dev-server crates, vendored `oj_*`, `jetpack` |
| JS framework | [`framework/`](framework) | `@desktop/forms`, VCL, foundation, reactive, net, shared config |
| templates | [`templates/`](templates) | starters and the prebuilt WebView runtime (`zebra.framework`) |

## Commands

```sh
cargo build --workspace          # builds into ~/.cargo-target (see .cargo/config.toml)
cargo test --workspace
pnpm install && pnpm typecheck   # pnpm test:js also runs; several packages have no test script
cargo run --bin lunarstorm -- build -p templates/starters/desktop
cargo run --bin lunarstorm-debug -- platform/examples/app
```

Release binaries are copied to `_build/`. Each package has its own README with its surface,
dependencies and hazards. Maintaining the repo: `.claude/skills/lunarstorm-monorepo`.
