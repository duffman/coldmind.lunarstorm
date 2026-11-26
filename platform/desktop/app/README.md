# lunarstorm-platform

The native desktop process of the Lunarstorm platform (spec section 6, `doc/plans/main.md`).
One library holds everything the two runtimes share: the TAO event loop and WRY WebViews
(one window per form), the IPC envelope, the asset store behind the custom protocol, native
services, the dev change table and the release archive. The two binaries are thin. The app's
backend does not run here: it runs on the Lunarstorm Runtime through `platform/desktop/host`
(`lunarstorm-host-desktop-backend`), and this crate never touches the JS engine.

## Public surface

- Binaries: `lunarstorm-debug <project>` (embedded pipeline, file watcher, in-memory
  asset store, devtools) and `lunarstorm-release` (reads the archive attached to its own
  executable, shows a "damaged or incomplete" dialog if it cannot).
- Modules (see `src/lib.rs` table): `manifest` (`HostConfig`, `CREATION_PROPS`),
  `envelope` (`Envelope`, `Kind`, `VERSION`, `MAX_MESSAGE`, `QUEUE_CAPACITY`),
  `assets` (`AssetStore`, `write_archive`, `read_archive`, `append_overlay`,
  `split_overlay`, `read_attached`), `pipeline` (`Pipeline`, `OutputDirPipeline`,
  `BuildOutput`, `Diagnostic`, `run_guarded`), `changes` (`Plan`), `native` (`WorkerPool`,
  4 threads), `ui` (`run`, `Mode`, `SHELL`), `dev`, `runtime` (`AppRuntime`).
- Feature `devtools` (default) turns on `wry/devtools`; `lunarstorm-pack` and
  `lunarstorm-web` depend on this crate with `default-features = false`.
- Env var at build time: `LUNARSTORM_APP_RUNTIME` = `lunarstorm` (default), `mock`, or a
  path to a `.rs` file defining `create(ctx: RuntimeContext) -> Box<dyn AppRuntime>`;
  `build.rs` writes `$OUT_DIR/selected_runtime.rs` from it.

## How it fits

Depended on by `platform/desktop/pack` and `platform/desktop/web` (they reuse `pipeline` and `assets`).
Depends on `platform/desktop/host`. The compiler side is behind the `Pipeline` trait: today
only `OutputDirPipeline` ships, which loads a generated directory (`zebra.json`,
`backend.js`, `frontend/**`, optional `package.json`); `platform/desktop/examples/app` is one.
Spec: section 6 (6.1 change table, 6.2 backend reload, 6.3 debug/release/browser).
Overview of the area: `platform/README.md`.

## Build, test, run

```bash
cargo run --bin lunarstorm-debug -- platform/desktop/examples/app
LUNARSTORM_APP_RUNTIME=mock cargo run --bin lunarstorm-debug -- platform/desktop/examples/app
cargo test -p lunarstorm-platform
cargo build --profile platform --no-default-features --bin lunarstorm-release
```

Tests are all unit tests in `src/` (assets, changes, dev, envelope, native, pipeline,
`runtime/tests.rs`, ui props); there is no `tests/` directory and no windowed test.

## Decisions and hazards

- Release builds use the `platform` profile, not `release`: pipeline and worker pool catch
  panics (`run_guarded`), which `panic = "abort"` would turn into aborts.
- Dev reactions are computed by diffing two `BuildOutput`s (`changes.rs`), not from which
  file changed, so they stay correct for any compiler behind `Pipeline`. Watcher debounce
  is 80 ms.
- Archive format: `"ZEBRAARC" | u64 LE manifest len | manifest JSON | blobs`, SHA-256 per
  file; the engine string `quickjs/rquickjs-0.14` is checked and a release runtime refuses
  other versions. The backend is stored under a NUL-prefixed name so no frontend path can
  address it. The magic strings still say ZEBRA (pre-rename); changing them breaks archives.
- Windows archive is an appended overlay (`u64 len | "ZEBRATRL"`) instead of a PE resource;
  survives Authenticode signing. Dropped: rewriting `.rsrc`.
- Every protocol response carries a CSP allowing only the asset origin; responses are `no-store`.
- Permissions are checked in `runtime/lunarstorm.rs` before any native service sees a call;
  the backend gets no filesystem roots, environment or sockets. `permissions.fs` scopes
  only the HostApi area `fs`, whose jobs run on worker threads (8 per backend).
- One envelope for dev messages, property updates and events, so ordering is queue order.
- `process.*` waiter polls `try_wait` (10 ms) so epoch release can kill a running child.
- Hard limits: 1 MiB per message, 16 MiB per call result, 64 calls in flight per form, queues of 1024, dialogs are modal on the UI thread, and
  an `onCloseQuery` that does not answer in time cannot keep a window open.
