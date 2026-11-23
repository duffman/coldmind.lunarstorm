# zebra-desktop — native desktop process (spec §6)

Implements `doc/plans/main.md` §6 (build-order steps 6–7): debug and release runtimes,
UI/JS threads, IPC envelope, native services, asset store + custom protocol, the dev
change table (6.1), backend reload (6.2), and debug/release/browser (6.3).

Steps 1–5 (the model compiler) are not part of this crate. The debug runtime embeds a
`Pipeline` trait; the shipped `OutputDirPipeline` reads a generated output directory:

```
zebra.json      host configuration (4e) with per-form property/event tables (4d)
backend.js      backend artifact (4c), ES module importing @desktop/bridge and @desktop/api
frontend/**     WebView files (4b); frontend/index.html is required
package.json    optional; a change forces a restart
```

`example/` is a complete project in that layout.

## Binaries

| binary          | use                                                                                   |
|-----------------|---------------------------------------------------------------------------------------|
| `zebra-debug`   | `zebra-debug <project>`: watches the project and applies changes per the 6.1 table; devtools on |
| `zebra-release` | runs the archive attached to its own executable; no watcher, no devtools              |
| `zebra-pack`    | `zebra-pack <project> --runtime <zebra-release> --out <dir> [--target macos\|windows\|linux] [--sign <identity>]` |
| `zebra-web`     | `zebra-web <project> [--port 7878]`: the project in a browser tab, backend in a Worker |

```bash
cargo run --bin zebra-debug -- example
```

```bash
cargo build --release --no-default-features --bin zebra-release --bin zebra-pack
```

```bash
cargo run --bin zebra-web -- example
```

## AppRuntime (provisional)

The host talks to the app's runtime only through `runtime::AppRuntime`
(`src/runtime/mod.rs`): `send` / `send_blocking` carry `HostMessage` (`Load`, validated
`event`/`request`/`cancel` envelopes, `Stop`); the runtime answers with `RuntimeMessage`
through `RuntimeContext::out`. It must report `Loaded` for every `Load` and answer every
`request`.

`build.rs` embeds the runtime named by `ZEBRA_APP_RUNTIME` at build time:

| value         | runtime                                                              |
|---------------|----------------------------------------------------------------------|
| `quickjs`     | default: the QuickJS JS thread                                       |
| `mock`        | `MockRuntime`: logs every IPC message to stderr as `[mock-runtime] {…}` |
| `<path>.rs`   | that file; it defines `pub fn create(ctx: crate::runtime::RuntimeContext) -> Box<dyn crate::runtime::AppRuntime>` |

```bash
ZEBRA_APP_RUNTIME=mock cargo run --bin zebra-debug -- example
```

## Layout

| path              | role                                                                     |
|-------------------|--------------------------------------------------------------------------|
| `src/manifest.rs` | host configuration, control tables, creation-time props, limits          |
| `src/envelope.rs` | envelope, WebView/backend validation, property mirror                    |
| `src/assets.rs`   | asset store, protocol responses (CSP, no-store), archive, attachment     |
| `src/pipeline.rs` | `Pipeline` boundary, `OutputDirPipeline`, panic → diagnostic            |
| `src/changes.rs`  | 6.1 change classification as a diff of two builds                        |
| `src/js/`         | JS thread: rquickjs Instance per epoch, timers, budget, heap, deadlines  |
| `src/native.rs`   | path/os inline, clipboard/notification/shell/process on a 4-thread pool  |
| `src/ui/`         | UI thread: tao loop, wry WebViews, muda menus, tray, dialogs, routing    |
| `src/shell/`      | shell injected into every WebView (IPC client, update queue, overlay)    |
| `src/dev.rs`      | watcher + compiler thread, 80 ms debounce                                |
| `src/package.rs`  | macOS bundle + codesign, Windows overlay + signtool, Linux dir + .desktop |
| `src/browser/`    | loopback server, in-page host, backend Worker                            |
| `src/runtime/`    | AppRuntime interface, build-time selection, MockRuntime                  |
| `build.rs`        | generates the selected runtime's `create` from `ZEBRA_APP_RUNTIME`       |

## Limits and fixed values

`MAX_MESSAGE` 1 MiB per envelope; queues bounded at 1024; per-message budget 5000 ms,
heap 256 MiB, request deadline 3000 ms (all overridable in `limits`). Worker pool: 4.

## Deviations from the spec

- Windows archive: appended overlay + `u64 len | "ZEBRATRL"` trailer instead of a PE
  resource section (no resource editor dependency; signtool signs over it).
- Browser host shows the main form only; menus, trays, other forms and window natives
  are refused (not faked); `onCloseQuery` is not sent (`beforeunload` cannot await);
  `os.info` is the only native it answers.

## Blocked

- Windows build: `rquickjs-sys` needs an MSVC C compiler and Windows SDK headers; none on this host.
- Linux build: `wayland-sys`/GTK need target pkg-config and system libraries; none on this host.
- 6.3 JS debugger attach in the debug runtime: rquickjs 0.14 / QuickJS exposes no debugger API.
