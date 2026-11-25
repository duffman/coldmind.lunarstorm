# devtools_server

The development server of the web toolchain. Serves `<root>/index.html` and compiles every
requested module on demand through `oj_compiler`, resolves imports with `oj_resolver`, records
each module in an `oj_graph::ModuleGraph`, and pushes hot updates over a WebSocket that speaks
Vite's HMR protocol (React components swap through React Refresh). The same socket carries
app channels (`app:<name>` custom events) between pages and the host process.

## Public surface

- `Config` (`new(root)`, `set_port`, `set_host`, `with_project_config(ProjectConfig)`,
  `with_validation(..)`; fields `root`, `host` default `127.0.0.1`, `port` default 5173 in
  the CLI, `define`, `alias`, `watch`).
- `DevServer::start(config)` runs its own Tokio runtime thread; `addr()`, `url()`, `state()`,
  `invalidate(files)`, `publish(channel, json)`, `messages() -> broadcast::Receiver<AppMessage>`,
  `stop()` (also on drop).
- `Embedded::start(&config)` + `router() -> axum::Router`: no listener; the host mounts the
  router on its own socket (the Tauri plugin does).
- `State` (`root`, `allow`, `resolver`, `graph`, `env`), `HMR_PATH = "/__devtools_hmr"`,
  `SHELL_HOT = "@coldmind/devtools-shell/hot"`, modules `module_pipeline` and `module_urls`.
- `assets/`: `client.js` (fallback HMR client), `refresh-runtime.js`, `refresh-preamble.js`.

## How it fits

Depends on `devtools_core`, `oj_compiler`, `oj_graph`, `oj_resolver`, axum 0.8, `notify`.
Used by `devtools-cli` (`lunarstorm serve`), `ls-build-server` and, with feature `dev-server`,
`tauri-plugin-devtools`. Not part of the Lunarstorm desktop `model/pipeline` chain.

## Build and test

```bash
cargo test -p devtools_server
```

Integration tests in `tests/`: `serve.rs` (page, modules, HMR frames, shell runtime, app
channels, Vite and TOML config), `linked.rs`, `assets.rs`, `styles.rs`, `validation.rs`.
`styles.rs` runs the app's own Tailwind through Node and needs it installed in the fixture.

## Decisions and hazards

- Access control: only `root` and its enclosing workspace are readable (`State::allow`).
  Imports that resolve outside register that one exact file (`resolved_files`), never its
  directory, so a linked package does not expose its neighbours (`linked.rs`).
- Bare imports from linked source outside the app root fall back to resolving from the app
  root (peer dependencies), except when the resolver said "ignored".
- Resolver conditions: `browser, import, module, development, default`.
- If the app depends on `@coldmind/devtools-shell`, `/@devtools/client.js` re-exports the
  shell's runtime so the page has one socket; otherwise the built-in client is served.
- Compile errors are HTTP 500 with the file name; a graph failure or a missing HMR
  acceptance boundary becomes a full reload and wins over incremental updates.
- Tailwind CSS is compiled by spawning Node with the app's own `@tailwindcss/node`; any source
  edit invalidates loaded stylesheets because utility candidates live outside the CSS graph.
- `stop()` does not wait for open WebSockets (test `stopping_server_does_not_wait_for_an_open_websocket`);
  the listener is bound with std so `start` works inside another async runtime.
- The server thread stack is `oj_compiler::COMPILE_STACK_SIZE`; deep ASTs overflow smaller ones.
- The watcher watches the workspace root (last entry of `allow`), not only the app root.
