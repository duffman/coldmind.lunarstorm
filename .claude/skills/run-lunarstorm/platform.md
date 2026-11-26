# Platform: the owner of the runtime

The platform is everything that **embeds** the runtime and ships an application. That means the
`lunar` command line, the desktop process (windows, WebViews, native calls), the browser host and
the packager. The toolchain (`toolchain/`) compiles a project into the output directory these
hosts run.

```
project (src/app.tsx, *.view.tsx, *.ts) --lunar build--> output dir (zebra.json, backend.js, frontend/)
output dir --lunarstorm-debug--> native window, backend on the Runtime, watcher
output dir --lunarstorm-web----> browser tab, backend in a Worker
output dir --lunarstorm-pack---> .app / .exe / linux dir, with lunarstorm-release + archive
```

## `lunar`: the command line (`platform/cli/app`)

`src/main.rs` mounts one package per Cargo feature on `crates/cli-host`. The default features are
all four:

| feature | package (crate) | commands |
|---|---|---|
| `runtime` | `lunarstorm-host-cli` | `run` (default command), `debug` (DAP on stdio), `jbn key`, `jbn build` |
| `build` | `lunarstorm-pipeline` | `build [-p dir] [-o dir]`, `build types`, `build check` |
| `frontend` | `devtools-cli` | `serve`, `bundle`, `preview`, `transpile` |
| `project` | `lunarstorm-project` (`platform/project`) | `project init` (`--template cli/program|cli/module|desktop/app|<folder>`), `project templates`, `project insight`, `project compile`, `project sign` |

The host adds `help`, `describe` (the whole tree as JSON), `completions <bash|zsh>`, `--color`,
`--help` and `--version`.

An unknown word runs `lunarstorm-<word>` from `PATH` (`Cli::external_prefix("lunarstorm")`). A
mounted name always wins: `lunar debug` is never `lunarstorm-debug`, and `lunar build` is never
`lunarstorm-build`. With no `lunarstorm-pack` on `PATH`, `lunar pack` falls through to `run` and
fails with `cannot read pack`.

How a crate adds commands: `crates/cli-contract` (`trait Package`, `Command`, `Arg`,
`Invocation`, zero dependencies). See the `lunarstorm-cli` skill.

## Desktop process (`platform/desktop/app`, crate `lunarstorm-platform`)

- **Binaries.**
  - `lunarstorm-debug <dir>` (`src/bin/debug.rs`): loads an output dir, opens windows, and watches *the output dir* with an 80 ms debounce. Re-run `lunar build` yourself.
  - `lunarstorm-release` reads the archive attached to its own executable.
- **Threads.**
  - UI: tao, wry, muda menus, tray, dialogs. All routing happens here.
  - The backend thread, in `platform/desktop/host`.
  - A 4-thread pool.
- **Native calls** `native.<ns>.<method>` (`src/native.rs`).

  | namespaces | run on |
  |---|---|
  | `app`, `window`, `screen`, `menu`, `tray`, `dialog` | UI thread |
  | `path`, `os` | inline |
  | `clipboard`, `notification`, `shell`, `process` | pool |

  The gate is `HostConfig::allows_native` (an exact `ns.method` or `ns.*`, from `permissions.native` in `zebra.json`).
- **IPC envelope** (`src/envelope.rs`).
  - Shape: `{version, kind, id, windowId, epoch, method, payload}`.
  - Kinds: hello, event, update, request, response, cancel, dev.
  - Limits: 1 MiB per message, queues of 1024.
- **`zebra.json`** (`src/manifest.rs`, `HOST_SCHEMA = 1`).
  - Contents: application, forms (props, events, controls), menus, trays, timers, permissions, limits.
  - Default limits: 5000 ms budget, 256 MiB heap, 3000 ms request deadline.
- **Dev change table** (`src/changes.rs`).

  | change | reaction |
  |---|---|
  | application, permissions or `package.json` | restart |
  | creation-time property | recreate the window |
  | other property | live setter |
  | forms | `dev.refresh` |
  | CSS | `dev.css` |
  | other frontend file | reload |
  | `backend.js` or limits | new backend epoch |
- **Build-time switch** `LUNARSTORM_APP_RUNTIME`: `lunarstorm` (default), `mock`, or a `.rs` path.
- **Release build:** `cargo build --profile platform --no-default-features --bin lunarstorm-release`. The `platform` profile unwinds on panic and drops devtools.

**Split with `platform/desktop/host`.**
- `platform/desktop/host` (`lunarstorm-host-desktop`) owns the JS side: `Backend<S: Shell>`, one Runtime per epoch, a watchdog on `PULSE`, the `@desktop/bridge` and `@desktop/api` modules (`js/desktop.js`), and a JSON-only `Shell`.
- `platform/desktop/app` implements `Shell` (`src/runtime/lunarstorm.rs`) and owns windows, natives, the envelope and permissions.
- `_build/lunarstorm-desktop` (from `platform/desktop/host/src/main.rs`) is a minimal tao shell with no WebView that runs one script.

## Browser host (`platform/desktop/web`, `lunarstorm-web <dir> [--port n]`)

- Serves an output dir on loopback, built once with no watcher.
- Injects `src/host.js` (the in-page host: it answers `os.info` and refuses every other native call).
- The backend runs in `src/worker.js`.
- **Hazard:** the bridge exists twice, in `platform/desktop/host/js/desktop.js` and `platform/desktop/web/src/worker.js`. Change both.

## Packager (`platform/desktop/pack`, `lunarstorm-pack`)

`lunarstorm-pack <dir> --runtime <lunarstorm-release> --out <dir> [--target macos|windows|linux] [--sign id]`
loads an already built dir (it does not run the pipeline).

| target | output | signing |
|---|---|---|
| macOS | `.app`, archive at `Contents/Resources/app.zebra` | `codesign` (ad hoc `-` without `--sign`) |
| Windows | overlay plus `ZEBRATRL` trailer | `signtool` only with `--sign` |
| Linux | directory with `<exe>.zebra` and a `.desktop` file | none |

It does not check that `--runtime` matches `--target`.

## Toolchain pipeline (`toolchain/crates/model`, `toolchain/crates/pipeline`)

- **`model::extract`** reads `src/app.tsx` and `*.view.tsx` as an AST and executes nothing.
- **`lunar build`** writes:
  - `backend.js` (a rolldown bundle; `@desktop/*` are external);
  - `frontend/{index.html, main.js, vcl.js, vcl.css, forms/<Form>.js}`;
  - `zebra.json`;
  - a copy of `package.json`.

  It writes nothing on error.
- **Dummy runtimes (commit `e59dce6`).** The embedded `vcl.js` has `mount = _load => {}`, so built projects render **blank**. The embedded `forms.ts` wires no handlers.
- **`lunar build types`** always writes `<project>/src/desktop.d.ts`. Don't run it on a tracked template.
- **Example app.** `platform/desktop/examples/app` is hand-written output with a working `vcl.js`. It is the only thing that renders a real UI. Rust tests name its controls, so don't regenerate it.

## Operate

```sh
.claude/skills/run-lunarstorm/driver.sh pipeline platform/desktop/templates/app   # -> .scratch/run-lunarstorm/dist
~/.cargo-target/debug/lunar build check -p platform/desktop/templates/app         # ok: 1 form(s)
.claude/skills/run-lunarstorm/driver.sh web platform/desktop/examples/app wait .vcl-statusbar Ready shot .scratch/run-lunarstorm/web.png
.claude/skills/run-lunarstorm/driver.sh desktop platform/desktop/examples/app
.claude/skills/run-lunarstorm/driver.sh stop
~/.cargo-target/debug/lunar describe                                         # every command as JSON
cargo test -q -p lunarstorm-web
cargo test -q -p lunarstorm --test ui --test cli --test commands
```

## Hazards

- **Rebuilds always change `backend.js`.** Rolldown embeds the temp work directory path (`$TMPDIR/lunarstorm-backend-<pid>-…`), so the dev table sees a backend change on every rebuild.
- **A failed `lunar bundle` leaves partial output** in `-o`.
- **`platform/extensions/` is not about lunar-extensions.** Its only crate, `project`, is a CLI package.
- **Stale READMEs:** `platform/cli/app/README.md` (says `lunarstorm`, two features) and `doc/cli.md` (no `project` package).
