# lunarstorm-host-cli (runtime/hosts/cli)

The CLI app host of doc/plans/main.md §5 ("CLI APP HOST: process, argv, stdio, exit code"), packaged as the `run` command of the `lunarstorm` command line. It owns a `Runtime<QuickJs, CliHost>`, answers the Runtime's host services with the process's own argv, stdio, environment, clock and exit, and registers the terminal bindings behind the `lunarstorm:ui` module. It is a library; the binary that mounts it is `apps/lunarstorm`.

## Public surface

- `package() -> Box<dyn Package>`: id `"runtime"`, one command: `run` (default command, `options_first`), with a `MODULES` and an `EXAMPLES` help section.
- `engine() -> String`: e.g. `quickjs-ng · aarch64-macos`, shown on the version line.
- `run` arguments: `-e, --eval <code>` (evaluate as an ES module; ends options) and the rest `<file.js> [args...]`. `process.argv` is `["lunarstorm", <script | "[eval]">, ...args]`, Node's shape, so scripts read their arguments from index 2.
- Inside: `CliHost` (`impl Host`), `bindings()` (terminal natives `color`, `tty` (constants read once), `columns()`, `live(on)` plus the `lunarstorm:ui` module compiled from `lunarstorm_cliui::UI_SOURCE`), `UI_NAME = "lunarstorm:ui"`.
- Exit status: `process.exitCode` or 0; 1 when the script cannot be read or load fails; 70 when the event loop cannot be created or bootstrap fails; uncaught errors and `process.exit()` end the process through `Host::exit`.

## How it fits

- Depends on `lunarstorm-runtime`, `lunarstorm-vm-quickjs`, `lunarstorm-cliui`, `lunarstorm-cli-contract`, `libc`.
- Mounted by `apps/lunarstorm` behind feature `runtime`; this crate is the one place the engine is chosen (`Runtime<QuickJs, CliHost>`) and everything below is generic.
- Spec: doc/plans/main.md §5 "CLI APP HOST"; command line in `doc/cli.md`.

## Build, test, run

```text
cargo build -p lunarstorm-host-cli
cargo test -p lunarstorm-host-cli        # no tests here; see below
cargo test -p lunarstorm --test cli --test ui   # the end-to-end tests of this host
cargo run -p lunarstorm -- runtime/hosts/cli/examples/demo.js <file>
cargo run -p lunarstorm -- runtime/hosts/cli/examples/ui.js        # in a terminal
```

`examples/demo.js` reads a file through `node:fs` and fires a timer; `examples/ui.js` demonstrates `style`, `progress`, `spinner` from `lunarstorm:ui`.

## Decisions and hazards

- A library, not a binary: the former `lunarstorm` binary and its hand-written argument parser are archived in `.archive/2026-11-24/runtime/hosts/cli/src/{main,cli}.rs`; their end-to-end tests moved to `apps/lunarstorm/tests/`, and the command line is unchanged.
- `run` is the default command, so `lunarstorm app.js` and `lunarstorm -e code` work. Options are read only up to the script name and `-e <code>` ends them, so everything after belongs to the script (as in Node).
- Policy is allow-everything (`Host::allow` returns `true`): a command-line program runs with the user's rights.
- Output uses `write(2)` directly with no extra buffering (the Runtime already batches stdout per loop turn). An interrupted write is retried and a full non-blocking pipe is waited on (`poll`), so output is never dropped while the reader lives; any other error (reader gone) ends the write silently.
- `Host::exit` restores a cursor hidden by a live progress region before `std::process::exit`.
- `entropy` uses `getentropy(2)` in chunks of at most 256 bytes and must never hand a script predictable bytes if it fails.
- Uncaught script errors are printed as a framed report (`cliui::report::uncaught`, labelled `Uncaught` or `Uncaught (in promise)`) after restoring the cursor, then exit with 1 (the `Host::uncaught` default). `print`, `flush` and `now`, which `ui.js` also needs, are the Runtime's own ops.
- The terminal natives and `lunarstorm:ui` are registered here, not in the Runtime: a host without a terminal has no use for them. `ui.js` requires `live.wrap`, so the host's console must route output through it while bars are shown.
