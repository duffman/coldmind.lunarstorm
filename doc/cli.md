# The `lunarstorm` command line

One binary, `lunar`, fronts every Lunarstorm tool. The tools are **packages**:
each crate that wants commands exports a description of them (a command tree with its
arguments) and the handlers to run them. The binary decides which packages it mounts;
the **host** parses the command line against their descriptions, renders help and
errors, and calls the handlers.

```text
apps/lunarstorm                 the binary: picks packages (one Cargo feature each)
  ├─ crates/cli-host            lunarstorm-cli-host: parse, help, describe, completions, dispatch
  │    └─ crates/cliui          lunarstorm-cliui: colors, help layout, error frames
  ├─ runtime/hosts/cli          lunarstorm-host-cli: package "runtime" → `run`, `debug`, `jbn key`, `jbn build`
  ├─ toolchain/crates/pipeline  lunarstorm-pipeline: package "build"  → `build`, `build types`, `build check`
  └─ toolchain/crates/cli       devtools-cli: package "frontend" → `serve`, `bundle`, `preview`, `transpile`
           all packages depend on
       crates/cli-contract      lunarstorm-cli-contract: Package, Command, Arg, Invocation, Context, Failure
```

The contract crate has no dependencies, so a package pulls in no parser, no terminal
code and no serde. A package never depends on another package or on the host.

## Using it

```text
lunar [options] <command> [args...]
lunar [options] <file.js> [args...]        # `run` is the default command

lunar app.js --port 8080                   # = lunar run app.js --port 8080
lunar -e 'console.log(1)'                  # = lunar run -e '...'
lunar jbn key signing.der > trust.json     # new signing key, and a trust file for it
lunar jbn build src/main.ts -o app.jbn --key signing.der
lunar run --trust trust.json app.jbn       # run a signed package (or LUNARSTORM_TRUST)
lunar debug app.ts --port 8080             # Debug Adapter Protocol on stdin/stdout; script stdout → stderr
lunar build -p templates/starters/desktop  # build into <project>/dist
lunar build -p app -o out                  # build into out/
lunar build types -p app                   # write app/src/desktop.d.ts
lunar build check -p app                   # report compile errors, write nothing
lunar serve ./app --port 5173              # frontend dev server with hot updates
lunar bundle ./app -o dist                 # production frontend bundle
lunar preview ./app --port 4173            # serve an existing bundle
lunar transpile src/main.ts                # one TS/TSX/JS/JSX module to JavaScript
lunar help build types                     # = lunar build types --help
lunar describe                             # the whole command tree as JSON
source <(lunar completions zsh)            # completion (also: bash)
lunar pack ...                             # runs `lunarstorm-pack ...` from PATH
```

Every command has `--help`, and `lunar help <command>...` prints the same screen. The host adds these options at every level:
`--color <auto|always|never>` (global), `-h/--help` and `-v/--version`.

| Exit status | Meaning |
|---|---|
| 0 | success |
| 2 | usage error, or nothing runnable named (help printed to stderr) |
| 70 | broken mount: two packages claim one name (`EX_SOFTWARE`) |
| other | what the handler returned (`run`: the script's exit code) |

## The contract (`lunarstorm-cli-contract`)

### `Package`

```rust
pub trait Package {
    fn id(&self) -> &'static str;              // "runtime", "build": used in mount errors
    fn title(&self) -> &'static str { … }      // help block title, shown in capitals; the id by default
    fn headline(&self) -> &'static str { … }   // one line next to the title; empty by default
    fn commands(&self) -> Vec<Command>;        // top-level commands, in help order
    fn sections(&self) -> Vec<Section> { … }   // extra top-level help sections (MODULES, …)
}
```

A crate exports it as `pub fn package() -> Box<dyn Package>`. The package names its
own top-level commands. For a namespace (`runtime eval`), use a command with children.
For a root command (`run`), use a command without them.

### `Command`

| Field / builder | Meaning |
|---|---|
| `Command::new(name, about)` | the selecting word and the help line |
| `.alias(word)` | another selecting word |
| `.arg(Arg)` | an argument of this level; positionals fill in the order added |
| `.child(Command)` | a subcommand; any depth |
| `.enter(fn)` | context producer, see [Context](#multi-level-commands-and-context) |
| `.run(fn)` | leaf handler → `Result<i32, Failure>` (exit status) |
| `.options_first()` | options only before the first positional; the rest are positionals (Node style) |
| `.default_command()` | parsed when the first word names no command; one per CLI, top level only |
| `.example(line)` | example for the command's help screen |

A command without `run` that is the last word prints its own help to stderr and exits 2.

### `Arg`

| Constructor | Command line | Read with |
|---|---|---|
| `Arg::flag("release")` | `--release` | `values.flag("release")` |
| `Arg::option("out")` | `--out x`, `--out=x`, `-o x` | `values.text`, `.path`, `.int`, `.all` (repeated) |
| `Arg::positional("n")` | a word in its position | `values.text("n")` |
| `Arg::rest("args")` | every remaining positional word | `values.all("args")` |

| Builder | Meaning |
|---|---|
| `.short('o')` | short letter (`-h` and `-v` are the host's) |
| `.value(ValueType::Text / Path / Int)`, `.choices(&[..])` | checked while parsing; a bad value is a usage error, so getters never fail |
| `.placeholder("<dir>")` | value name on help screens (default `<name>`) |
| `.about(text)` | help line |
| `.default(v)` / `.env("VAR")` | used when absent, env first |
| `.required()` | usage error when absent after env and default |
| `.global()` | also accepted by every command below (`build types --project x`) |
| `.ends_options()` | after this option every word is a positional (`-e <code> args...`) |

### Multi-level commands and context

When the path passes through a command, the host calls its `enter`, from the root down
to the leaf, and then calls the leaf's `run`. `enter` reads its level's arguments and
puts a typed value into `invocation.context`. Handlers below it read the value back:

```rust
pub struct Target { pub project: PathBuf }

Command::new("build", "…")
    .arg(Arg::option("project").short('p').value(ValueType::Path).default(".").global())
    .enter(|inv| { inv.context.insert(Target { project: inv.values.path("project").unwrap() }); Ok(()) })
    .child(Command::new("types", "…").run(|inv| {
        let target = inv.context.require::<Target>()?;   // Failure if a parent did not provide it
        Ok(write_types("lunarstorm", &target.project))
    }))
```

- The context is keyed by **type**, one value per type. A deeper `enter` that inserts
  the same type replaces the outer value.
- `values` is a single map for the whole path. The host rejects a tree that declares an
  argument name twice along one path, so a child reads its parent's options and the
  globals with the same getters.

### `Failure`

| Variant | Host prints | Status |
|---|---|---|
| `Failure::usage(msg)` / `Failure::Usage { message, hint }` | `error: msg`, `hint: …`, `usage: <typed path> --help` | 2 |
| `Failure::failed(msg)` / `Failure::Failed { message, status }` | `error: msg` | `status` |

A handler that already reported its own problem (for example compiler diagnostics)
returns `Ok(1)`.

## Parsing rules (`lunarstorm-cli-host`, `parse.rs`)

1. While options are open, `--` closes them and is dropped.
2. While options are open, a word starting with `-` is an option. The options in scope
   are the current command's plus the `global` ones of every command above it.
   `-h/--help` and `-v/--version` work at every level.
3. Any other word selects a child of the current command, but only while that command
   has taken no positional. Otherwise it fills the next positional, and a `Rest`
   argument takes all the words that remain.
4. At the top level, a word that names nothing is handled in this order:
   - an external `lunarstorm-<word>` on `PATH`;
   - an unknown command, with a hint, when it is a near-miss of a command name and not
     an existing file (so `lunar biuld` gives a hint instead of a "module not found");
   - otherwise it goes to the default command, where it is read again.

   An option unknown at the top level also goes to the default command, so
   `lunar -e code` reaches `run`.
5. After the last word, env variables and defaults fill absent arguments, then
   required arguments are checked.

Unknown options get a "did you mean" over the options in scope. The `usage:` line
names the commands the user actually typed, so a default command entered implicitly
is not part of it.

## Mount validation

`Cli::tree()` refuses the mount, and `run` exits 70 naming the problem, when:
- two commands or aliases share a top-level name, including the host's `describe`,
  `completions` and `__complete`;
- more than one default command exists, or the default command is not top level;
- an option uses `help`, `version` or `color`, or the short letters `h` or `v`;
- an argument name repeats along a path;
- a `Rest` argument is not the last positional;
- two sibling subcommands share a name.

`crates/cli-host/tests/host.rs` asserts each of these.

## `describe` JSON

```json
{
  "program": "lunar", "version": "0.1.0",
  "options":  [ <arg> ],
  "commands": [ { "name", "about", "aliases", "default", "options_first", "runs",
                  "args": [ <arg> ], "commands": [ … ], "examples": [ … ] } ]
}
<arg> = { "name", "kind": "flag|option|positional|rest", "short", "value": "text|path|int|choice",
          "choices", "placeholder", "about", "default", "env", "required", "global", "ends_options" }
```

Use it for docs, editor integrations and tests. It is produced from the same specs
the parser uses.

## Completion

`lunar completions bash|zsh` prints a script. The script only forwards the words
typed so far to the hidden `lunar __complete <words…>`, which answers from the
live tree with:
- the choices of the option before the cursor;
- options, when the word starts with `-`;
- otherwise subcommand names.

When it answers with nothing, the shell completes file names. Because completion is
dynamic, mounting a package never requires regenerating the script.

## External commands

`lunar foo args…` runs the first executable `lunarstorm-foo` on `PATH` when no
mounted command is named `foo`, and returns its exit status. The top-level help lists
the externals it finds. Externals own their own `--help`. With `_build/` on `PATH`,
the platform binaries (`lunarstorm-pack`, `lunarstorm-web`, `lunarstorm-debug`, …) show
up as `lunarstorm pack`, `lunarstorm web`, ….

## Choosing packages

`apps/lunarstorm/Cargo.toml` has one feature per package (`runtime`, `build`, `frontend`,
all on by default):

```sh
cargo build -p lunarstorm                                            # run + build + frontend
cargo build -p lunarstorm --no-default-features --features build     # build only
```

`src/main.rs` mounts each enabled package with one `.package(...)` line.

## Adding a package

The `lunarstorm-cli` skill in `.claude/skills/lunarstorm-cli/SKILL.md` has the
step-by-step procedure and checklist. In short:
1. Depend on `lunarstorm-cli-contract`. Add a `command` module (or `cli.rs`) with a
   `Package` impl and `pub fn package()`.
2. Add the crate to `apps/lunarstorm` as an optional dependency with a feature, and
   add one `.package(...)` line to `main.rs`.
3. Test the real binary in `apps/lunarstorm/tests/`.

## Tests

| Suite | Covers |
|---|---|
| `crates/cli-host/tests/host.rs` | parsing rules, context chain, value checks, env/defaults, statuses, mount validation (probe package) |
| `apps/lunarstorm/tests/commands.rs` | real binary: help, `build`/`types`/`check` on the starter, usage errors, `describe`, completion, externals |
| `apps/lunarstorm/tests/frontend_commands.rs`, `frontend_transpile.rs` | real binary: `serve`, `bundle`, `preview`, `transpile` (moved from `toolchain/crates/cli`, interface rewritten from `--serve`-style flags) |
| `apps/lunarstorm/tests/cli.rs`, `ui.rs` | the runtime's `run`: event loop, net, fs, modules, exit codes, color, help, usage errors (moved unchanged from `runtime/hosts/cli`) |
