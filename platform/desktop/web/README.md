# lunarstorm-web

The browser host of spec 6.3: serves a project to a browser tab with its own in-page host,
so forms can be worked on without the native process. The backend runs in a module Worker
in the tab. A developer tool; the shipped desktop process never links it.

## Public surface

Binary `lunarstorm-web <project> [--port <n>]`: builds the project once with
`run_guarded(OutputDirPipeline)` and serves it on `http://127.0.0.1:<port>` (default 7878;
`0` picks a free port; the bound address is printed). Exit 2 on bad arguments, 1 on build
errors (printed as `<file>: <message>`) or a bind failure.

`src/browser.rs`: `serve(build, listener)`, `page(build)` (injects `host.js` then the
desktop shell first in `<head>`, replacing `__ZEBRA_CONFIG__` and `__ZEBRA_BOOT__`) and
`resolve`. Routes: frontend files from the build, and under `/__zebra/`: `worker.js`,
`backend.js` (bare `@desktop/*` imports rewritten), `bridge.js`, `api.js`.

## How it fits

Depends on `lunarstorm-platform` (`platform/desktop/app`, `default-features = false`) for
`BuildOutput`, `OutputDirPipeline`, `run_guarded`, `assets::mime` and `ui::SHELL`, so the
tab runs the unchanged desktop shell. Spec: section 6.3 (Browser).

## Build, test, run

```bash
cargo run --bin lunarstorm-web -- platform/desktop/examples/app
cargo test -p lunarstorm-web
```

Three unit tests in `src/browser.rs` (page injection, import rewriting, route resolution).
The in-page host (`src/host.js`) and Worker (`src/worker.js`) have no automated test.

## Decisions and hazards

- One tab shows the main form only. Other forms, menus, tray icons and window properties
  are mirrored but their native calls are refused, not faked. `os.info` is the only native
  answered; `timer` components are driven by `host.js`.
- `onCloseQuery` is never sent: `beforeunload` cannot await an async answer.
- The epoch is fixed at 1: no backend reload, no file watcher; restart the binary to
  rebuild.
- Plain `std::net::TcpListener`, one thread per connection, `Connection: close`, loopback
  only, `no-store`. No HTTP crate by design.
- The config is inlined into a `<script>`; `</` is escaped as `<\/` so a `</script>` in a
  JSON string cannot close it.
- `BRIDGE_MODULE` and `API_MODULE` are copies of the modules the desktop process's former JS
  thread provided; the Worker is their only consumer.
