# lunarstorm-cli-contract (crates/cli-contract)

The contract between the `lunarstorm` command line and the packages mounted on it. A package (the runtime's `run`, the build pipeline's `build`) describes its commands and arguments as plain data and receives a parsed `Invocation`; it never sees a parser, a terminal library or serde. The crate has no dependencies at all.

## Public surface

| item | role |
|---|---|
| `trait Package` | `id()`, `commands() -> Vec<Command>`, optional `sections()` (extra top-level help sections as `Section = (String, Vec<(String, String)>)`) |
| `Command` | `new(name, about)`, `.alias`, `.arg`, `.child`, `.enter(fn)`, `.run(fn)`, `.options_first()`, `.default_command()`, `.example` |
| `Arg`, `ArgKind`, `ValueType` | `Arg::flag / option / positional / rest`; `.short .value .choices .placeholder .about .default .env .required .global .ends_options`; `ValueType::{Text, Path, Int, Choice}` |
| `Invocation { path, values, context }` | what a handler receives |
| `Values` | `flag`, `text`, `path`, `int`, `all`, `has`; infallible once parsing succeeded |
| `Context` | `insert::<T>`, `get::<T>`, `require::<T>() -> Result<&T, Failure>`; typed state from `enter` down to the leaf |
| `Failure` | `Usage { message, hint }` (status 2), `Failed { message, status }`; constructors `usage()`, `failed()` |
| `Run`, `Enter` | boxed handler types: `Fn(&mut Invocation) -> Result<i32, Failure>` / `Result<(), Failure>` |

A package crate exports `pub fn package() -> Box<dyn Package>`; see `lunarstorm_host_command_line::command::package()` (`runtime/host-command-line/src/lunar_runtime_commands.rs`) for a complete example.

## How it fits

- Depended on by every package (`runtime/host-command-line`, `toolchain/crates/pipeline`) and by `crates/cli-host`, which parses the command line against these specs and calls the handlers. A package never depends on another package or on the host.
- Not part of the runtime stack of doc/plans/main.md §5; it is the command-line layer described in `doc/cli.md` ("The contract").

## Build and test

```text
cargo build -p lunarstorm-cli-contract
cargo test -p lunarstorm-cli-contract
```

The crate has no tests of its own. Parsing, mount validation and context flow are tested in `crates/cli-host/tests/host.rs`, which uses this crate's types.

## Decisions and hazards

- Specs are plain data built by methods, not derive macros or a parser library's types: a package does not pin a parser version, and the host renders help, completions and the `describe` JSON from the same data.
- `Values` is one map for the whole command path. The host rejects trees that declare an argument name twice along a path, so a child reads its parent's options and globals with the same getters. Reusing a name in a child is therefore a mount error (exit 70), not a shadowing.
- `Context` is keyed by type, one value per type: a deeper `enter` inserting the same type replaces the outer value. Use a dedicated struct per package, not `String` or `PathBuf`, or two packages' contexts can overwrite each other.
- `enter` runs root to leaf, then the leaf's `run`. A command without `run` that ends the path prints its help to stderr and exits 2.
- Handlers own their output. A handler that already reported its own problem (compiler diagnostics) returns `Ok(1)`; returning `Failure::Failed` makes the host print `error: <message>` as well.
- `-h`, `-v`, `--help`, `--version`, `--color` are reserved for the host at every level.
