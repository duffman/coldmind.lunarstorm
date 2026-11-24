# lunarstorm-cli-host (crates/cli-host)

The host side of the `lunarstorm` command line. A product binary hands it a program name, version and the packages it mounts (types from `lunarstorm-cli-contract`); the host validates the combined command tree, parses `argv` against it, renders help and usage errors through `lunarstorm-cliui`, answers `describe` and shell completion, runs external `lunarstorm-<name>` programs, and dispatches to the matching handler. Packages never parse anything themselves.

## Public surface

```text
Cli::new(program, mark, version, tagline)   // e.g. ("lunarstorm", "☾ lunarstorm", VERSION, "...")
    .detail(text)                           // shown after the version on a terminal
    .package(Box<dyn Package>)              // repeat per mounted package, in help order
    .run(words: Vec<String>) -> i32         // the process exit status
Cli::tree() -> Result<Tree, String>         // validation only; Tree { root, sections }
```

What it adds to every CLI: global `--color <auto|always|never>`, `-h/--help` and `-v/--version` at every level, `describe` (the tree as JSON), `completions <bash|zsh>` plus the hidden `__complete`, and external commands (`lunarstorm foo` runs `lunarstorm-foo` from `PATH` when no mounted command is named `foo`).

Modules: `parse` (command line against the tree), `help` (screens from the specs), `describe` (JSON and completion). Only `Cli` and `Tree` are public.

## How it fits

- Depends on `lunarstorm-cli-contract`, `lunarstorm-cliui`, `serde_json`. Used by `apps/lunarstorm` only.
- Rules and examples are specified in `doc/cli.md` ("Parsing rules", "Mount validation", "`describe` JSON", "Completion"). No section of doc/plans/main.md; this is the command-line layer around its §5 CLI host.

## Build and test

```text
cargo build -p lunarstorm-cli-host
cargo test -p lunarstorm-cli-host
```

`tests/host.rs` mounts two fixture packages and checks: default command takes the script and everything after it, `ends_options`, `enter` context reaching children and globals following subcommands, value checking at parse time, env fallback, usage errors and statuses, and `broken_mounts_are_refused` (one assertion per rule below).

## Decisions and hazards

- Exit statuses: 0, 2 (usage error, or an empty command line: top help goes to stderr), 70 (`EX_SOFTWARE`, broken mount), otherwise the handler's. A broken mount is reported as `<program>: internal: <problem>` before any parsing.
- `tree()` refuses: duplicate top-level names or aliases (also against `describe`, `completions`, `__complete`); a second default command, or a default that is not top level; options named `help`, `version`, `color` or short `-h`/`-v`; an argument name repeated along one path; a `Rest` argument not last; duplicate sibling subcommands.
- Top-level resolution order for a word that names nothing: external `lunarstorm-<word>` on `PATH`, then (near-miss of a command and not an existing file) an unknown-command hint, else the default command. This is why `lunarstorm biuld` hints while `lunarstorm app.js` runs.
- Values are checked against their `ValueType` in the parser, so handlers read them with infallible getters and every bad value has one error format.
- Help is generated from the same specs the parser uses, so the two cannot disagree; layout and color stay in `cliui::screen`.
- Completion is dynamic: the shell script only forwards typed words to `lunarstorm __complete`, so mounting a package never needs a regenerated script. `__complete` returns before color initialization.
- External lookup rejects names containing a path separator, and requires an executable file named `<program>-<name>`.
- `--color` is applied (`term::init`) before anything is printed so usage errors are already in the requested mode; `term::restore()` runs after every action to put the cursor back.
