# lunarstorm (platform/cli/app)

The `lunarstorm` binary: one command line for the Lunarstorm runtime and the desktop build tools. It contains no logic of its own. `src/main.rs` (22 lines) builds a `lunarstorm_cli_host::Cli`, mounts the packages enabled by Cargo features, and exits with `cli.run(args)`. Everything else is in the packages and in the host.

## Public surface

The product is the command line, documented in `doc/cli.md`.

| feature (default: both) | package mounted | commands |
|---|---|---|
| `runtime` | `lunarstorm-host-cli` (`platform/cli/host`) | `run` (the default command: `lunarstorm app.js`, `-e <code>`) |
| `build` | `lunarstorm-pipeline` (`toolchain/crates/pipeline`) | `build`, `build types`, `build check` |

The host (`crates/cli-host`) adds `describe`, `completions <bash|zsh>`, `--color`, `-h/--help`, `-v/--version`, and runs unknown commands as `lunarstorm-<name>` from `PATH`. Exit statuses: 0 success, 2 usage error, 70 broken mount (two packages claim one name), otherwise the handler's status (for `run`, the script's).

## How it fits

- Depends on: `crates/cli-host` (always), `platform/cli/host` (feature `runtime`), `toolchain/crates/pipeline` (feature `build`).
- Spec: doc/plans/main.md §5 (CLI app host sits on this binary), §4 (build pipeline behind `build`). Command-line design: `doc/cli.md`.
- Mounting a new package = an optional dependency, a feature, and one `.package(...)` line in `src/main.rs`.

## Build, test, run

From the workspace root:

```text
cargo build -p lunarstorm                                      # both packages
cargo build -p lunarstorm --no-default-features --features build   # `build` only
cargo build --release -p lunarstorm                            # fat LTO, panic=abort, stripped
cargo test -p lunarstorm
```

Finished binaries are kept in `_build/` (put it on `PATH` to make `lunarstorm pack` find `_build/lunarstorm-pack`).

Tests (`tests/`) run the real binary through `CARGO_BIN_EXE_lunarstorm`:
- `cli.rs`: scripts through `-e` against real sockets and files: scheduling order vs Node, timers, TCP echo and connect failure, HTTP keep-alive/pipelining/streaming, HTTP smuggling rejection, fs, modules, exit status, console formatting, promise/tick semantics.
- `commands.rs`: help lists every package, `build check`/`build types` on the starter project, usage errors, `describe`, completion, external `lunarstorm-<name>` programs.
- `ui.rs`: drives a pseudo-terminal via `libc`: color precedence (`--color`, `FORCE_COLOR`, `NO_COLOR`), exit 2 on usage errors, code frames on color streams, `lunarstorm:ui` animation and cursor restore.

## Decisions and hazards

- Features select packages at compile time, so a `--no-default-features --features build` binary has no runtime at all; the `run` tests in `cli.rs` and `ui.rs` need the default features.
- The old `lunarstorm` binary of `platform/cli/host` is archived in `.archive/2026-11-24/`; its end-to-end tests live here now and the command line is unchanged.
- Test scratch directories: `commands.rs` uses `CARGO_TARGET_TMPDIR` (under the cargo target dir); `cli.rs` uses `std::env::temp_dir()` and deletes/recreates `lunarstorm-test-<name>-<pid>` on every call.
- `commands.rs` reads `platform/desktop/types/app` from the repo, so it must run from a full checkout.
