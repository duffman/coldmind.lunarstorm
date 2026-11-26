# lunarstorm-cliui (crates/cliui)

Terminal UI for Lunarstorm command-line tools. The tool decides what is said; this crate decides how it looks: per-stream color detection, styling pens and gradients, branded help screens, usage errors with "did you mean", uncaught-error reports with source code frames, and a script-facing JS module (`lunarstorm:ui`) with styles, progress bars and spinners. Its only dependency is `libc`; it knows nothing of the JS engine or the event loop.

## Public surface

| item | what |
|---|---|
| `term::init(ColorChoice)` | call once at startup; decides color depth separately for stdout and stderr |
| `term::depth(fd)`, `is_tty(fd)`, `columns()` | `Depth` tier, tty test, terminal width |
| `term::Pen(Depth)` | `sgr`, `fg`, `rgb`, `gradient`; palette consts `VIOLET`, `CYAN`, `RED` |
| `term::live(on)`, `term::restore()` | hide/show the cursor for bars; signal-safe restore |
| `screen::{Brand, Section, brand, help, usage_error, suggest}` | static screens; layout and color only, content passed as data |
| `report::uncaught(label, text)` | uncaught-error report |

The script-facing module `lunarstorm:ui` (`style`, `progress`, `spinner`) is native and lives in `runtime/host-command-line/src/terminal_ui_module.rs`, because it binds to the JS engine; it draws with `term`. The former source module `js/ui.js` is archived.

## How it fits

- Used by `crates/cli-host` (help screens, usage errors, `--color`) and `runtime/host-command-line` (error reports, `lunarstorm:ui`, terminal ops).
- Mentioned in the root `Cargo.toml` header and `doc/cli.md`; it implements no spec section (tooling around doc/plans/main.md §5's CLI host).

## Build and test

```text
cargo build -p lunarstorm-cliui
cargo test -p lunarstorm-cliui
```

The crate has no tests of its own. Its behavior is covered end to end by `apps/lunarstorm/tests/ui.rs` (`cargo test -p lunarstorm --test ui`), which runs the real binary on a pseudo-terminal.

## Decisions and hazards

- Color precedence, highest first: `--color`, `FORCE_COLOR`, `NO_COLOR`, then "is a terminal and `TERM` is not `dumb`" (Node's order). Truecolor needs `COLORTERM=truecolor|24bit`; `TERM` containing `256` gets the 256 cube; otherwise 16 colors, where gradients degrade to one named color. Every styled run ends in `ESC[0m`; runs never nest.
- Color is per stream, so `lunarstorm app.js > log.txt` keeps colored diagnostics on the terminal and a plain log file.
- `report::uncaught` on a plain stream is exactly `<label> <engine text>`; scripts and tests parse that format. The code frame (2 lines before, 1 after, caret at the column) is terminal-only.
- `term::live(true)` installs SIGINT/SIGTERM/SIGHUP handlers on first use so Ctrl-C during a bar does not leave the cursor hidden. Scripts that never draw a live region keep default signal dispositions. Handlers must stay async-signal-safe (`write(2)` only).
- `lunarstorm:ui` (in `runtime/host-command-line`): bars and spinners draw on stderr so stdout stays pipe-clean; all live items share one region at the bottom; redraws throttle to 50 ms (`FRAME_MS`); when stderr is not a tty each item prints exactly one line at completion; the 16-color tier and `NO_COLOR` use block glyphs because colors cannot tell filled from empty cells.
