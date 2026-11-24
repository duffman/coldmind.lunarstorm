# tauri-plugin-devtools

Tauri v2 plugin that exposes the toolchain's transform engine to a webview and can host the
development frontend on the application's own HTTP gateway. Registered as plugin `devtools`.

## Public surface

- `init<R>()` and `Builder::new().development(root).build()`: install the plugin; `development`
  adds the embedded dev frontend (needs feature `dev-server`, else it prints
  "development root ignored" and starts nothing).
- `development_router(&AppHandle) -> Option<axum::Router>`: the host mounts it on its listener;
  the plugin never binds a socket. `None` when no root was set, the feature is off or the app
  is exiting.
- IPC command `plugin:devtools|transform` with `TransformRequest { source, loader
  (js|jsx|ts|tsx), filename, options: { replace, define, inject } }` returning
  `TransformResult`; `transform_source` is the same logic without Tauri.
- Features: `plugin` (default), `dev-server` (adds `devtools_server`), `bundler` (re-exports
  `devtools_bundler` as `bundle`, a compatibility export only).
- Permissions: `permissions/default.toml` grants `allow-transform`; windows must list
  `devtools:default` (or `devtools:allow-transform`) in their capability.

## How it fits

Depends on `devtools-native` (`toolchain/crates/transform`, no `bun` feature), optionally
`devtools_server` and `devtools_bundler`. Library name `tauri_plugin_devtools`; nothing else
in the workspace depends on it. `build.rs` runs `tauri_plugin::Builder` for the `transform`
command only when feature `plugin` is on.

## Build and test

```bash
cargo test -p tauri-plugin-devtools
cargo test -p tauri-plugin-devtools --features dev-server
```

13 unit tests in `src/transform_commands.rs` (real HTTP plus permission-controlled IPC,
unknown-field rejection, per-loader behaviour) and one in `frontend_host.rs` (dev-server
only; serves `../server/tests/fixtures/app`).

## Decisions and hazards

- The command takes source text only: it never opens `filename` (label for warnings), reads
  no files, starts no server, runs no code and does not transpile TS or JSX.
- Requests deserialize with `deny_unknown_fields`; `inject` is `false`, `true` (marked classes
  only) or `{ "all": true }`.
- Transformation runs on a blocking worker; invalid config fails the call, and invalid native
  config fails plugin initialization.
- The frontend lives in managed state and is dropped on `RunEvent::Exit`, which stops its
  watcher.
- The `Cargo.toml` comment "the sole executable lives in crates/cli" is stale: that crate
  has no binary.
