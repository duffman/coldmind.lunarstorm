---
name: run-lunarstorm
description: Build, run, drive and screenshot the Lunarstorm binaries in this monorepo — the `lunar` CLI (run JS, build a desktop project), the browser host `lunarstorm-web` and the desktop host `lunarstorm-debug`. Use when asked to run, start, launch, smoke-test, click through or screenshot the app, a project, the example app or the starter, or to check that a change works in the real binaries rather than only in unit tests.
---

# Run Lunarstorm

Paths are relative to the repo root. The harness is `.claude/skills/run-lunarstorm/driver.sh`:
it builds the binaries, launches them in the background and drives the UI through
`driver.mjs`, which uses headless system Chrome over the DevTools protocol with no npm
dependencies. State, logs and screenshots go to `.scratch/run-lunarstorm/`.

## Prerequisites (macOS)

Rust (workspace toolchain), Node >= 22 (global `WebSocket`), Google Chrome in `/Applications`
(or `CHROME=<binary>`), `swift` (Xcode CLT; finds the desktop window), `python3`.

## Run: everything (agent path, first)

```sh
.claude/skills/run-lunarstorm/driver.sh smoke
```

Builds `lunar`, `lunarstorm-web` and `lunarstorm-debug` (debug, about 20 s warm), runs the
CLI, builds the starter, and drives the example app in the browser:

1. type into the name field;
2. wait for the status bar to read `Modified`;
3. click Save;
4. wait for `Saved “…”`.

It writes `web-before.png` and `web-after.png`, then opens the desktop window.

## Run: the UI in a browser (the interactive path)

```sh
.claude/skills/run-lunarstorm/driver.sh web platform/examples/app \
  wait .vcl-statusbar Ready \
  type input.vcl-edit ' 2' wait .vcl-statusbar Modified \
  click text=Save wait .vcl-statusbar Saved \
  text .vcl-listbox shot .scratch/run-lunarstorm/web-after.png
.claude/skills/run-lunarstorm/driver.sh stop
```

`web <dir>` serves the directory on a free port. The server keeps running until `stop`, and
its URL is in `.scratch/run-lunarstorm/web.url`. Run more steps against it directly:

```sh
node .claude/skills/run-lunarstorm/driver.mjs open "$(cat .scratch/run-lunarstorm/web.url)/" eval 'document.body.innerText'
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

Page console output and exceptions, including the backend Worker's, print as `[page] …`.

Selectors come from `frontend/vcl.js`: every control is `.vcl-<type>` (`input.vcl-edit`,
`button.vcl-button`, `.vcl-listbox`, `.vcl-statusbar`). Controls have no name attribute, so
address buttons by caption (`text=Save`).

## Run: CLI and pipeline

```sh
.claude/skills/run-lunarstorm/driver.sh cli
.claude/skills/run-lunarstorm/driver.sh pipeline templates/starters/desktop .scratch/run-lunarstorm/dist
```

## Run: desktop host

```sh
.claude/skills/run-lunarstorm/driver.sh desktop platform/examples/app .scratch/run-lunarstorm/desktop.png
.claude/skills/run-lunarstorm/driver.sh stop
```

This opens a real tao/wry window, waits for its CoreGraphics window id, and captures it. The
host log is in `.scratch/run-lunarstorm/desktop.log`.

## Test (focused)

```sh
cargo test -q -p lunarstorm-web
cargo test -q -p lunarstorm --test ui --test cli --test commands
```

## Gotchas

- **`lunarstorm-web` and `lunarstorm-debug` take a built directory** (one holding
  `zebra.json`), not a source project. Pointing them at `templates/starters/desktop` fails
  with `templates/starters/desktop/zebra.json: No such file or directory`. Build it first
  (`driver.sh pipeline …`), or use `platform/examples/app`, which is hand-written output.
- **Built projects render a blank page.** The pipeline embeds a dummy WebView runtime (commit
  `e59dce6`): `frontend/vcl.js` exports `mount = _load => {}`, so `#root` stays empty and
  nothing errors. Drive the UI on `platform/examples/app`, which has its own working `vcl.js`.
- **The desktop capture needs Screen Recording permission** for the terminal running the agent
  (System Settings > Privacy & Security > Screen & System Audio Recording). Without it,
  `screencapture` prints `could not create image from window` (also with the sandbox off),
  and `driver.sh desktop` exits 3 after confirming the window opened. The browser path
  shows the same frontend and backend and needs no permission.
- **In the browser the status bar reads `Ready · browser/web`.** The in-page host answers
  `os.info` itself; every other `native.*` call is refused, not faked.
- **`~/.cargo-target/debug` is on `PATH` in `~/.zshrc`,** so a bare `lunar` may be the debug
  build rather than `~/.local/bin/lunar`. The driver always uses the debug build it just made.

## Troubleshooting

| symptom | fix |
|---|---|
| `zebra.json: No such file or directory` | the host was given a source project; build it first (see Gotchas) |
| `screencapture refused: …`, exit 3 | grant Screen Recording (see Gotchas); the window itself is fine |
