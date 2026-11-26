---
name: lunarstorm-cli
description: Add or change commands on the `lunarstorm` command line. Covers mounting a crate as a package, declaring commands, subcommands, flags, options and positionals, passing context between command levels, and testing the result. Use whenever someone wants a crate's functionality reachable as `lunarstorm <something>`, asks to add a subcommand, option or flag, or touches crates/cli-contract, crates/cli-host, platform/cli/app, platform/cli/host/src/command.rs, platform/project/src/command.rs, toolchain/crates/pipeline/src/command.rs or toolchain/crates/cli/src/command.rs — even if they only say "expose X on the CLI" or "add a flag to build".
---

# Lunarstorm CLI packages

The `lunarstorm` binary (`platform/cli/app`) mounts **packages**. A package describes
its commands as data (`lunarstorm-cli-contract`); the host (`lunarstorm-cli-host`)
parses, renders help, validates the mount and dispatches. Full reference: `doc/cli.md`.

Read these before editing:
- `crates/cli-contract/src/spec.rs`: `Command`, `Arg`, `ValueType`
- `crates/cli-contract/src/invocation.rs`: `Invocation`, `Values`, `Context`
- `toolchain/crates/pipeline/src/command.rs`: a worked package with a context level and subcommands
- `toolchain/crates/cli/src/command.rs`: flat commands whose handlers add checks a value type cannot express
- `platform/cli/host/src/command.rs`: a default command with Node-style option handling

## Add a package (a crate that has no commands yet)

1. **Dependency.** In the crate's `Cargo.toml`:
   `lunarstorm-cli-contract = { path = "<rel>/crates/cli-contract" }`. Nothing else;
   the contract has no dependencies.
2. **Module.** Add `src/command.rs` (always this name, in every package) with a unit struct, `impl Package`
   (`id`, `commands`, and optionally `sections`) and `pub fn package() -> Box<dyn Package>`.
   Declare it with `pub mod command;` in `lib.rs`.
3. **Handlers stay thin.** A handler reads `invocation.values` and the context, calls
   the crate's existing library function, prints the result, and returns `Ok(status)`.
   If the crate already has a standalone `main.rs`, move the shared reporting into the
   module and call it from both, as `pipeline` does with `report` and `write_types`.
4. **Mount.** In `platform/cli/app/Cargo.toml`, add the crate as an
   `optional = true` dependency and a feature named after the package
   (`<id> = ["dep:<crate>"]`). Add the feature to `default` unless the user says
   otherwise. In `platform/cli/app/src/main.rs`, add
   `#[cfg(feature = "<id>")] let cli = cli.package(<crate>::command::package());` and a
   row in the module doc table.
5. **Test** (see Verify), then **document**: add a row to the package table and the
   examples in `doc/cli.md`.

## Add a command or argument to an existing package

Edit that package's `commands()` only. Never touch the host to add a command.

| Want | Write |
|---|---|
| subcommand | `.child(Command::new("name", "about").run(handler))` |
| flag | `.arg(Arg::flag("dry-run").about("…"))` → `values.flag("dry-run")` |
| option with value | `.arg(Arg::option("out").short('o').placeholder("<dir>").value(ValueType::Path))` → `values.path("out")` |
| fixed set of values | `.choices(&["web", "desktop"])` |
| integer | `.value(ValueType::Int)` → `values.int("n")` |
| required positional | `.arg(Arg::positional("file").required())` |
| remaining words | `.arg(Arg::rest("args"))` (must be the last positional) |
| default / env fallback | `.default("x")`, `.env("LUNARSTORM_X")` |
| valid below this level too | `.global()` |
| state for subcommands | `.enter(\|inv\| { inv.context.insert(MyCtx { … }); Ok(()) })` → children call `inv.context.require::<MyCtx>()?` |
| bad input | `return Err(Failure::usage("…"))` (exit 2, prints usage line) |
| failure the handler did not print | `Err(Failure::failed("…"))` (exit 1) |
| failure already printed | `Ok(1)` |

## Rules the host enforces

The mount fails with exit 70 when a rule is broken; `Cli::tree()` returns the error.
- Top-level names and aliases must be unique across all packages. `help`, `describe`,
  `completions` and `__complete` belong to the host.
- Option names `help`, `version` and `color` and short letters `h` and `v` are reserved.
- An argument name may appear only once along a path, globals included, because one
  `Values` map serves the whole path.
- Only one top-level command may be `default_command()`. `run` already is.
- A `Rest` argument comes last among positionals.

## Design choices to keep

- **Use an option for a project, file or target on a command that has children.** A
  positional there is ambiguous with subcommand names. That is why `build` uses
  `-p/--project`.
- **Pass state down with `enter` and a context struct**, not by re-reading parent
  options in every child. The struct lives next to the commands and is `pub`, so other
  crates could read it.
- **Use `options_first()` only for commands that run user programs** (Node style: the
  script's own flags must not be parsed).
- **Handlers print their own results; the host prints help and `Failure`s.** Keep
  plain-stream output byte-stable, because scripts parse it.
- **Document every new item** as the repo does: `///` summary plus `# Arguments`,
  `# Returns` and `# Errors`. Write decisions next to the code.

## Verify

Run all of these. Each must pass, and read the test count of each suite:

```sh
cargo test -p lunarstorm-cli-host -p lunarstorm        # host rules + real-binary suites
cargo test -p <your crate>
cargo clippy -p lunarstorm --all-targets -- -D warnings
cargo build -p lunarstorm --no-default-features --features <id>   # the package mounts alone
```

Add a test to `platform/cli/app/tests/commands.rs` that runs the real binary
(`env!("CARGO_BIN_EXE_lunarstorm")`). Write scratch output under
`env!("CARGO_TARGET_TMPDIR")` and assert all of these:
- the command appears in `lunarstorm --help` and `lunarstorm <cmd> --help`;
- the happy path runs and returns the expected status and output;
- one bad input gives exit 2 with the exact `error:` / `usage:` lines;
- `lunarstorm describe` contains the command;
- `lunarstorm __complete <cmd> ""` lists its subcommands, if it has any.

Then check by hand: `~/.cargo-target/debug/lunarstorm <cmd> --help`.
