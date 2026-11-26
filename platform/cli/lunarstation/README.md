# lunarstation (platform/cli/lunarstation)

The `lunarstation` binary: a second command line built on the CLI host
(`crates/cli-host`), beside `lunar` (`platform/cli/app`). `src/main.rs` builds
`Cli::new("lunarstation", …)` with `external_prefix("lunarstation")`, mounts the
packages enabled by Cargo features, and exits with `cli.run(args)`.

## Packages

| feature   | crate                 | commands |
|-----------|-----------------------|----------|
| `runtime` | `lunarstorm-host-command-line` | `run` (default) |
| `build`   | `lunarstorm-pipeline` | `build`, `build types`, `build check` |
| `frontend`| `devtools-cli`        | `serve`, `bundle`, `preview`, `transpile` |
| `project` | `lunarstorm-project`  | `project types`, `project <type> <action>`, `insight`, `compile`, `sign` |

The host adds `describe`, `completions`, `--color`, `--help`, `--version`, and runs an
unknown `lunarstation <name>` as `lunarstation-<name>` from `PATH`.

Adding a lunarstation-only command: a crate exporting `fn package() -> Box<dyn
lunarstorm_cli_contract::Package>`, added here as an optional dependency with a
feature and one `.package(...)` line in `src/main.rs`.

## Build and test

```sh
cargo build -p lunarstation
cargo test -p lunarstation
cargo test -p lunarstation --no-default-features --features build
```

`tests/cli.rs` runs the real binary: `--version` and `--help` under the
`lunarstation` name.

## Decisions

- Same packages and feature names as `lunar`, so either binary can be cut to any subset.
- External prefix `lunarstation`, so `lunarstation-*` and `lunarstorm-*` externals
  never shadow each other.
