# Lunarstorm platform: the native desktop process (spec §6)

Implements `doc/plans/main.md` §6 (build-order steps 6–7): debug and release runtimes,
UI and backend threads, IPC envelope, native services, asset store + custom protocol,
the dev change table (6.1), backend reload (6.2), and debug/release/browser (6.3).

Steps 1–5 (the model compiler) are not part of it. The debug runtime embeds a
`Pipeline` trait; the shipped `OutputDirPipeline` reads a generated output directory:

```
zebra.json      host configuration (4e) with per-form property/event tables (4d)
backend.js      backend artifact (4c), ES module importing @desktop/bridge and @desktop/api
frontend/**     WebView files (4b); frontend/index.html is required
package.json    optional; a change forces a restart
```

`desktop/examples/app/` is a complete project in that layout.

## Crates

Each product owns its host, its binaries and its project templates.

| product | crate | path | binaries / role |
|---|---|---|---|
| cli | `lunarstorm-host-cli` | `cli/host/` | CLI app host; the `run`, `debug`, `jbn` commands |
| cli | `lunarstorm` | `cli/app/` | `lunar` |
| cli | `lunarstation` | `cli/lunarstation/` | `lunarstation` |
| cli | `lunarstorm-cli-templates` | `cli/templates/` | `cli/program`, `cli/module` |
| desktop | `lunarstorm-host-desktop` | `desktop/host/` | desktop app host (backend thread, epochs) |
| desktop | `lunarstorm-platform` | `desktop/app/` | `lunarstorm-debug`, `lunarstorm-release` |
| desktop | `lunarstorm-pack` | `desktop/pack/` | `lunarstorm-pack` |
| desktop | `lunarstorm-web` | `desktop/web/` | `lunarstorm-web` |
| desktop | `lunarstorm-desktop-templates` | `desktop/templates/` | `desktop/app` (also the desktop starter) |
| both | `lunarstorm-project` | `project/` | `lunar project`: init from templates, insight, compile, sign |

| binary               | use                                                                                   |
|----------------------|---------------------------------------------------------------------------------------|
| `lunarstorm-debug`   | `lunarstorm-debug <project>`: watches the project and applies changes per the 6.1 table; devtools on |
| `lunarstorm-release` | runs the archive attached to its own executable; no watcher, no devtools              |
| `lunarstorm-pack`    | `lunarstorm-pack <project> --runtime <lunarstorm-release> --out <dir> [--target macos\|windows\|linux] [--sign <identity>]` |
| `lunarstorm-web`     | `lunarstorm-web <project> [--port 7878]`: the project in a browser tab, backend in a Worker |

Run from the repo root:

```bash
cargo run --bin lunarstorm-debug -- platform/desktop/examples/app
cargo run --bin lunarstorm-web -- platform/desktop/examples/app
```

Release builds use the `platform` profile (`release` aborts on panic; the platform catches panics):

```bash
cargo build --profile platform --no-default-features --bin lunarstorm-release --bin lunarstorm-pack
```

## AppRuntime (provisional)

The host talks to the app's runtime only through `runtime::AppRuntime`
(`desktop/app/src/runtime/mod.rs`): `send` / `send_blocking` carry `HostMessage` (`Load`,
validated `event`/`request`/`cancel` envelopes, `Stop`); the runtime answers with
`RuntimeMessage` through `RuntimeContext::out`. It must report `Loaded` for every `Load`
and answer every `request`.

`build.rs` embeds the runtime named by `LUNARSTORM_APP_RUNTIME` at build time:

| value         | runtime                                                              |
|---------------|----------------------------------------------------------------------|
| `lunarstorm`  | default: the backend on the Lunarstorm Runtime, through the desktop app host (`platform/desktop/host`) |
| `mock`        | `MockRuntime`: logs every IPC message to stderr as `[mock-runtime] {…}` |
| `<path>.rs`   | that file; it defines `pub fn create(ctx: crate::runtime::RuntimeContext) -> Box<dyn crate::runtime::AppRuntime>` |

```bash
LUNARSTORM_APP_RUNTIME=mock cargo run --bin lunarstorm-debug -- platform/desktop/examples/app
```

## Layout

| path                       | role                                                                     |
|----------------------------|--------------------------------------------------------------------------|
| `desktop/app/src/manifest.rs`  | host configuration, control tables, creation-time props, limits          |
| `desktop/app/src/envelope.rs`  | envelope, WebView/backend validation, property mirror                    |
| `desktop/app/src/assets.rs`    | asset store, protocol responses (CSP, no-store), archive, attachment     |
| `desktop/app/src/pipeline.rs`  | `Pipeline` boundary, `OutputDirPipeline`, panic → diagnostic            |
| `desktop/app/src/changes.rs`   | 6.1 change classification as a diff of two builds                        |
| `desktop/app/src/native.rs`    | path/os inline, clipboard/notification/shell/process on a 4-thread pool  |
| `desktop/app/src/ui/`          | UI thread: tao loop, wry WebViews, muda menus, tray, dialogs, routing    |
| `desktop/app/src/shell/`       | shell injected into every WebView (IPC client, update queue, overlay)    |
| `desktop/app/src/dev.rs`       | watcher + compiler thread, 80 ms debounce                                |
| `desktop/app/src/runtime/`     | AppRuntime interface, build-time selection, Lunarstorm adapter, MockRuntime |
| `desktop/build.rs`         | generates the selected runtime's `create` from `LUNARSTORM_APP_RUNTIME`  |
| `desktop/pack/src/package.rs` | macOS bundle + codesign, Windows overlay + signtool, Linux dir + .desktop |
| `desktop/web/src/`         | loopback server, in-page host, backend Worker                            |

## Limits and fixed values

`MAX_MESSAGE` 1 MiB per envelope; queues bounded at 1024; per-message budget 5000 ms,
heap 256 MiB, request deadline 3000 ms (all overridable in `limits`). The budget applies to every JS entry (event, timer,
settled native call). Worker pool: 4.

## Deviations from the spec

- Windows archive: appended overlay + `u64 len | "ZEBRATRL"` trailer instead of a PE
  resource section (no resource editor dependency; signtool signs over it).
- Browser host shows the main form only; menus, trays, other forms and window natives
  are refused (not faked); `onCloseQuery` is not sent (`beforeunload` cannot await);
  `os.info` is the only native it answers.

## Blocked

- Windows build: `rquickjs-sys` needs an MSVC C compiler and Windows SDK headers; none on this host.
- Linux build: `wayland-sys`/GTK need target pkg-config and system libraries; none on this host.
- 6.3 JS debugger attach in the debug runtime: QuickJS exposes no debugger API.
