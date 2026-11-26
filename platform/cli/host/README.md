# lunarstorm-host-command-line (platform/cli/host)

The CLI app host of doc/plans/main.md §5 ("CLI APP HOST: process, argv, stdio, exit code"), packaged as the `run` command of the `lunarstorm` command line. It owns a `Runtime<QuickJs, CliHost>`, answers the Runtime's host services with the process's own argv, stdio, environment, clock and exit, and gives scripts the terminal API through the HostApi extension (`lunarstorm:hostapi/ui`, also `lunarstorm:ui`). It is a library; the binary that mounts it is `platform/cli/app`.

## Public surface

- `package() -> Box<dyn Package>`: id `"runtime"`, one command: `run` (default command, `options_first`), with a `MODULES` and an `EXAMPLES` help section.
- `engine() -> String`: e.g. `quickjs-ng · aarch64-macos`, shown on the version line.
- `run` arguments: `-e, --eval <code>` (evaluate as an ES module; ends options) and the rest `<file.js> [args...]`. `process.argv` is `["lunarstorm", <script | "[eval]">, ...args]`, Node's shape, so scripts read their arguments from index 2.
- Inside: `CliHost` (`impl Host`, extension set `(HostApi<V, (Ui,)>,)`, manifest `manifest.json`), `bindings()` (the constants `ops.color` and `ops.tty`, read once, and `lunarstorm:ui`, the same native module as `lunarstorm:hostapi/ui`), `terminal_ui_area::Ui` (the HostApi area `ui`: the native module `lunarstorm:hostapi/ui`, built by `terminal_ui_module`), `UI_NAME = "lunarstorm:ui"`.
- `manifest.json`: `{ "extensions": { "hostapi": { "plugins": ["ui"] } } }`, embedded with `include_str!` and parsed once.
- Exit status: `process.exitCode` or 0; 1 when the script cannot be read or load fails; 70 when the event loop cannot be created or bootstrap fails; uncaught errors and `process.exit()` end the process through `Host::exit`.

## How it fits

- Depends on `lunarstorm-event-loop-runtime`, `lunar-extension-hostapi`, `lunarstorm-engine-quickjs-adapter`, `lunarstorm-cliui`, `lunarstorm-cli-contract`, `libc`.
- Mounted by `platform/cli/app` behind feature `runtime`; this crate is the one place the engine is chosen (`Runtime<QuickJs, CliHost>`) and everything below is generic.
- Spec: doc/plans/main.md §5 "CLI APP HOST"; command line in `doc/cli.md`.

## Build, test, run

```text
cargo build -p lunarstorm-host-command-line
cargo test -p lunarstorm-host-command-line        # the manifest matches the host's extension set
cargo test -p lunarstorm --no-default-features --features runtime --test cli --test ui   # end-to-end
cargo run -p lunarstorm -- platform/cli/host/examples/demo.js <file>
cargo run -p lunarstorm -- platform/cli/host/examples/ui.js        # in a terminal
```

`examples/demo.js` reads a file through `node:fs` and fires a timer; `examples/ui.js` demonstrates `style`, `progress`, `spinner` from `lunarstorm:ui`.

## Decisions and hazards

- A library, not a binary: the former `lunarstorm` binary and its hand-written argument parser are archived in `.archive/2026-11-24/platform/cli/host/src/{main,cli}.rs`; their end-to-end tests moved to `platform/cli/app/tests/`, and the command line is unchanged.
- `run` is the default command, so `lunarstorm app.js` and `lunarstorm -e code` work. Options are read only up to the script name and `-e <code>` ends them, so everything after belongs to the script (as in Node).
- Policy is allow-everything (`Host::allow` returns `true`): a command-line program runs with the user's rights.
- Output uses `write(2)` directly with no extra buffering (the Runtime already batches stdout per loop turn). An interrupted write is retried and a full non-blocking pipe is waited on (`poll`), so output is never dropped while the reader lives; any other error (reader gone) ends the write silently.
- `Host::exit` restores a cursor hidden by a live progress region before `std::process::exit`.
- `entropy` uses `getentropy(2)` in chunks of at most 256 bytes and must never hand a script predictable bytes if it fails.
- Uncaught script errors are printed as a framed report (`cliui::report::uncaught`, labelled `Uncaught` or `Uncaught (in promise)`) after restoring the cursor, then exit with 1 (the `Host::uncaught` default). `print`, `flush` and `now`, which the `ui` module also uses, are the Runtime's own ops; it gets them with `Bindings::native_module_with_internals`.
- The terminal API is the HostApi area `ui`, chosen by the host's own manifest: a host without a terminal does not offer it. `ops.color` and `ops.tty` stay host bindings because the Runtime's console reads `ops.color` for its own colouring whether or not `ui` is used; The `ui` module reads the depths through `cliui::term`. It sets `live.wrap`, so the host's console must route output through it while bars are shown.
- `lunarstorm:ui` is registered natively next to `lunarstorm:hostapi/ui`; the first build keeps its objects on the console's `live` slot, so both specifiers export the same objects and share one live region.
- The manifest lives with the host and is fixed when the host is built; a malformed one panics at first use, which `the_manifest_matches_the_host` catches in the crate's tests.
