# platform/desktop/examples/app

A complete project in the generated-output layout the platform runs (`platform/README.md`,
"Steps 1-5 ... `OutputDirPipeline` reads a generated output directory"). It is not a crate
and not a model project: there is no `src/`, no `*.view.tsx`, nothing to compile.

Consumers: `lunarstorm-debug`, `lunarstorm-web`, `lunarstorm-pack`, and the Rust unit tests
of `platform/desktop/app`, `platform/desktop/pack` and `platform/desktop/web`, which build it through the real
`OutputDirPipeline` (`sample_build()` in `desktop/src/pipeline.rs`, `pack/src/package.rs`,
`web/src/browser.rs`).

## Layout

| path | spec step | role |
|---|---|---|
| `zebra.json` | 4e + 4d | host config: application (`id`, `mainForm`, `events`), forms with window props and per-control property/event tables, the `MainMenu`, timers, `permissions.native` |
| `backend.js` | 4c | ES module importing `@desktop/bridge` (`serve`, `setProperty`, `restored`) and `@desktop/api` (`native`); control proxies, `MainForm` class, connection table, `serve(...)` dispatch |
| `frontend/index.html` | 4b | required entry (the pipeline errors with "frontend/index.html is missing"); loads `vcl.css` and `main.js` |
| `frontend/main.js` | 4b | reads `window.zebra.windowId`, imports `./forms/<windowId>.js`, mounts it; on `dev.refresh` re-imports with a `?t=` stamp and remounts |
| `frontend/forms/MainForm.js` | 4b | the form tree: `[type, props, children]` with literal props only |
| `frontend/vcl.js`, `vcl.css` | 4b | a ~3.4 KB DOM implementation of the VCL, one element per control type |
| `package.json` | - | `giraffe: { schema: 1 }`; copied by the pipeline, a change forces a restart (6.1) |

The example is a Project Register: list, name editor, Save/Revert, a one-second clock timer,
a status bar, a File/Edit menu. `onCloseQuery` refuses to close while dirty.

## Who generates it

Nobody. This directory is hand-written output: its own comments say it "stands in for what
4c generates" (`backend.js`), "stands in for 4b output" (`MainForm.js`) and "stands in for
the React VCL" (`vcl.js`). The real pipeline (`lunarstorm build -p <project>`, crate
`toolchain/crates/pipeline`) writes the same file set from a model project such as
`platform/desktop/types/app`, but with different content: its starter form has `OwnerEdit`,
`StatusEdit`, `NotesMemo`, no timer, and the VCL is `templates/zebra.framework/dist/vcl.js`.

## Running

```bash
cargo run --bin lunarstorm-debug -- platform/desktop/examples/app          # native window, watcher, devtools
cargo run --bin lunarstorm-web -- platform/desktop/examples/app            # http://127.0.0.1:7878, backend in a Worker
cargo run --bin lunarstorm-web -- platform/desktop/examples/app --port 0   # free port
LUNARSTORM_APP_RUNTIME=mock cargo run --bin lunarstorm-debug -- platform/desktop/examples/app
```

Debug watches the directory (80 ms debounce) and classifies each edit by the 6.1 change
table. The browser host shows the main form only and answers only `os.info`; `permissions.native`
lists `os.*`, `window.setTitle`, `clipboard.writeText`.

## Hazards

- Checked-in, not regenerated: do not run the pipeline into this directory. Output of
  `lunarstorm build` would replace it with a different app and break the tests that name its
  controls (`NameEdit`, `ClockTimer`, `ProjectList`, `SaveItem`; e.g. `desktop/src/envelope.rs`).
- Keep `zebra.json` and `backend.js` in step by hand: the control property tables in
  `zebra.json` and the `accessors(...)` lists and `mainFormConnections` in `backend.js` must name
  the same properties and events, or the host rejects the envelope.
- `backend.js` reads `restored(name)`, so values carried across a backend reload (6.2) override the
  defaults in `mainFormControls`.
