---
name: run-lunarstorm
description: Operate the Lunarstorm monorepo. Covers what the runtime (the Node.js-like JS runtime in runtime/), the platform (the lunar CLI, desktop process, browser host, packager that own the runtime) and the extensions (lunar-extensions, HostApi, manifests) are, and how to build, run, drive, test and screenshot them. Use when asked to run, start, launch, smoke-test, click through or screenshot the app, the example app or a project; to run JS on the runtime; to build, pack or serve a project; to add or test an extension; or to learn how the runtime, a host, the platform or extensions work before changing them.
---

# Operate Lunarstorm

Paths are relative to the repo root. Harness: `.claude/skills/run-lunarstorm/driver.sh`, plus
`driver.mjs`, which drives headless system Chrome over the DevTools protocol with no npm
dependencies. State, logs and screenshots go to `.scratch/run-lunarstorm/`.

## The three parts

| part | what it is | read |
|---|---|---|
| **Runtime** `runtime/` | Node.js-like JS runtime: QuickJS-NG behind a VM contract (`runtime/vm`), a mio event loop with `node:*` builtins (`runtime/core`), TS/CJS/packages (`runtime/modules`), signed `.jbn` packages, a DAP debugger. A library: a **host** owns it (`platform/cli/host`, `platform/desktop/host`). | [runtime.md](runtime.md) |
| **Platform** `platform/`, `toolchain/` | Owns the runtime and ships apps: the `lunar` CLI (runtime + build + frontend + project packages), the desktop process (`lunarstorm-debug`/`-release`: windows, WebViews, `native.*`), the browser host `lunarstorm-web`, the packager `lunarstorm-pack`; the pipeline compiles a project into the output dir they run. | [platform.md](platform.md) |
| **Extensions** `extensions/` | lunar-extensions: modules, natives and state a host installs per Runtime, selected by the host's manifest; plugin hosts and plugins (HostApi with the CLI's `ui` area). Runtime level built; platform level design only. | [extensions.md](extensions.md) |

Shared crates: `crates/cli-contract`, `crates/cli-host`, `crates/cliui`, `crates/http1` (the
runtime's HTTP/1.1 framing). For where a change belongs and how to add a crate, see the
`lunarstorm-monorepo` skill. For adding CLI commands, see the `lunarstorm-cli` skill.

## Run everything (agent path, first)

```sh
.claude/skills/run-lunarstorm/driver.sh smoke
```

The smoke run does this:
1. Builds `lunar`, `lunarstorm-web` and `lunarstorm-debug`.
2. Runs the CLI and builds the starter.
3. Drives the example app in the browser: type a name, see `Modified`, click Save, see `Saved “…”`. Screenshots: `web-before.png`, `web-after.png`.
4. Opens the desktop window.

| subcommand | what it does |
|---|---|
| `build [--fresh]` | debug build of the three binaries; `--fresh` rebuilds from this checkout (see Gotchas) |
| `cli` | runtime smoke: `--version`, `-e`, a script using `node:fs` |
| `runtime-ipc` | workers + `lunarstorm:ipc`; the CLI host has them off, the `ipc-demo` host turns them on |
| `project` | `lunar project init` → `compile` (signed `.jbn`) → `run --trust` |
| `pipeline <project> [out]` | `lunar build` into an output dir |
| `web <dir> [steps…]` | serve an output dir and drive it; keeps running until `stop` |
| `desktop <dir> [png]` | real window plus capture |
| `stop` | stop what `web` and `desktop` started |

## Drive the UI in a browser

```sh
.claude/skills/run-lunarstorm/driver.sh web platform/desktop/examples/app \
  wait .vcl-statusbar Ready \
  type input.vcl-edit ' 2' wait .vcl-statusbar Modified \
  click text=Save wait .vcl-statusbar Saved \
  text .vcl-listbox shot .scratch/run-lunarstorm/web-after.png
node .claude/skills/run-lunarstorm/driver.mjs open "$(cat .scratch/run-lunarstorm/web.url)/" eval 'document.body.innerText'
.claude/skills/run-lunarstorm/driver.sh stop
```

| step | what it does |
|---|---|
| `open <url>` | loads the page |
| `type <sel> <text>` | real key input |
| `click <sel>` or `click text=<caption>` | real mouse click |
| `wait <sel> <text>` | polls for 5 s until the element contains the text |
| `text <sel>` | prints the element's text |
| `eval <js>` | prints the result of the expression |
| `shot <png>` | screenshot |

Page console output and Worker exceptions print as `[page] …`.

Controls are `.vcl-<type>`, for example `input.vcl-edit`, `.vcl-listbox` and `.vcl-statusbar`.
Address buttons by caption. Each page load starts a fresh backend, so state does not carry
across `open`s.

## Runtime, extensions and the desktop window

```sh
.claude/skills/run-lunarstorm/driver.sh cli
.claude/skills/run-lunarstorm/driver.sh runtime-ipc
.claude/skills/run-lunarstorm/driver.sh project
~/.cargo-target/debug/lunar -e 'import ui from "lunarstorm:ui"; import { areas } from "lunarstorm:hostapi"; console.log(Object.keys(ui), areas)'
.claude/skills/run-lunarstorm/driver.sh pipeline templates/starters/desktop .scratch/run-lunarstorm/dist
.claude/skills/run-lunarstorm/driver.sh desktop platform/desktop/examples/app .scratch/run-lunarstorm/desktop.png
.claude/skills/run-lunarstorm/driver.sh stop
```

## Test (focused; never the whole workspace)

```sh
cargo test -q -p lunarstorm-runtime --test modules --test worker --test ipc --test extensions
cargo test -q -p lunar-extension-hostapi
cargo test -q -p lunarstorm-vm-quickjs
cargo test -q -p lunarstorm-web
cargo test -q -p lunarstorm --test ui --test cli --test commands
```

## Gotchas

- **One shared target, several checkouts.** `~/.cargo-target` is shared by `main` and every worktree in `.worktrees/`, and they write the same artifacts. Cargo then takes another branch's build as fresh, so a plain `cargo build` does not relink.
  - Seen: `lunar --help` on `main` showed the `cli-package-help` worktree's RUNTIME/BUILD sections, and `top_help_lists_every_package_and_the_host_commands` failed with `missing "COMMANDS"`.
  - Fix: `driver.sh build --fresh` (16 s), which touches every crate root and rebuilds.
- **Hosts take output dirs, not source projects.** `lunarstorm-web` and `lunarstorm-debug` need a directory holding `zebra.json`. Given `templates/starters/desktop` they fail with `templates/starters/desktop/zebra.json: No such file or directory`. Build first.
- **Built projects render blank.** The pipeline embeds a dummy WebView runtime (`e59dce6`) whose `mount` is `_load => {}`. Drive the UI on `platform/desktop/examples/app`.
- **The CLI has no workers, IPC, `fetch` or `http.request`.** `lunarstorm:worker` and `lunarstorm:ipc` are "module not found" under `lunar`; use `driver.sh runtime-ipc`. See runtime.md for the builtins.
- **Desktop capture needs Screen Recording permission** for the terminal (System Settings > Privacy & Security > Screen & System Audio Recording). Without it, `screencapture` says `could not create image from window`, even with the sandbox off. `driver.sh desktop` then exits 3 after confirming the window opened.
- **In the browser the status bar reads `Ready · browser/web`.** The in-page host answers only `os.info`; every other `native.*` call is refused.
- **`lunar` on `PATH` may not be the build you think.** `~/.zshrc` puts `~/.cargo-target/debug` on `PATH`, ahead of `~/.local/bin/lunar`.

## Troubleshooting

| symptom | fix |
|---|---|
| `zebra.json: No such file or directory` | the host got a source project; run `driver.sh pipeline` first |
| `screencapture refused: …`, exit 3 | grant Screen Recording; the window itself is fine |
| `Error resolving module 'lunarstorm:worker' … module not found` | expected under `lunar`; use `driver.sh runtime-ipc` |
