# lunarstorm-host-desktop-backend (runtime/host-desktop-backend)

The desktop app host of doc/plans/main.md §5 ("DESKTOP HOST") and §6.2 (backend reload): it runs a desktop application's backend on a `Runtime<QuickJs, _>` on its own thread. The library knows nothing about windows, menus or IPC envelope types; it talks to the native desktop process through the `Shell` trait and carries envelopes and native-call arguments as JSON text. The crate also ships `lunarstorm-desktop`, a minimal tao-based shell (native windows, no WebView) that exercises the library end to end.

## Public surface

- `Backend<S: Shell>`: `spawn(shell)`, `load(Program)` (replaces the running backend, retires the previous epoch), `dispatch(epoch, envelope)` (event, request, cancel; envelopes for any other epoch are dropped), `settler()`. Dropping it stops the thread.
- `Settler<S>`: `settle(epoch, call, Result<String, String>)` finishes a native call from any thread; results for a retired epoch are dropped.
- `trait Shell: Send + Sync + 'static` (implemented by the desktop process): `loaded`, `send`, `native(epoch, call, namespace, method, arguments)`, `error`, `log`, `retired`.
- `Program { epoch, code, restore, memory, budget, roots, environment, name }`: one backend to run. `code` is one ES module importing only `@desktop/bridge` (`setProperty`, `serve`, `restored`) and `@desktop/api` (`native.<namespace>.<method>(...args) -> Promise`); both are native modules (`src/desktop_bridge_natives.rs`); no script runs for them.
- `Engine = QuickJs`: the one alias where the VM is bound.
- Binary `lunarstorm-desktop <script.js>`: scripts get `native.window.create(name, {title, width, height})`, `setTitle`, `close`, `native.app.quit(code)`, and events `Window.onResize`, `Window.onCloseQuery`, `Window.onClose`. Exit: `app.quit`'s code, 0 when the last window closed, 1 when the script did not load, 2 for usage.

## How it fits

- Depends on `lunarstorm-event-loop-runtime`, `lunarstorm-engine-quickjs-adapter`, `libc`; the binary adds `tao` and `serde_json`.
- Used by `platform/desktop` (the native desktop process: `lunarstorm-host-desktop-backend = { path = "../../runtime/host-desktop-backend" }`).
- Spec: doc/plans/main.md §5 "DESKTOP HOST", "Isolation", §6.2 "Backend reload" (new Instance per epoch, late messages blocked by epoch).

## Build, test, run

```text
cargo build -p lunarstorm-host-desktop-backend
cargo test -p lunarstorm-host-desktop-backend              # tests/backend.rs, tests/desktop.rs
cargo run -p lunarstorm-host-desktop-backend -- runtime/host-desktop-backend/examples/hello.js
```

`tests/backend.rs` uses a recording `Shell` against a real Runtime: events/updates/requests are answered, native calls settle from another thread, script errors are reported and the backend goes on, a non-loading backend reports why, a reload retires the old epoch and drops its messages. `tests/desktop.rs` runs the real binary on `examples/selfcheck.js` and **opens a real native window for about a third of a second**, so it needs a desktop session; it also checks exit 1 for a script that does not load and exit 2 for no arguments.

## Decisions and hazards

- One backend thread, one Runtime per epoch: a reload is a new VM instance, so heap, jobs, timers and pending native calls of the old backend go with it. Reload and stop travel on a control channel (they replace the Runtime the tasks run in) and wake the loop with an empty task.
- `dispatch` and `settle` post tasks straight to the Runtime's loop; there is no second queue. The loop is `hold`-ing, so it sleeps in the kernel between events.
- Execution budget: a watchdog thread (`lunarstorm-budget`) samples the Runtime's `pulse()` twice per budget; the same odd value for a whole budget means one JS entry has run that long, and it calls `Interrupt::request`. It covers every JS entry (event, timer, settled call). An interrupt is reported to the shell as "execution budget exceeded". `Host::PULSE` is true here (it is false for the CLI host).
- Hooks (`dispatch`, `settle`) live in a thread-local on the backend thread because the posting threads cannot own engine handles; it is cleared before the instance is destroyed.
- A script error never ends the process: `Host::uncaught` reports to the shell and the loop goes on. `process.exit(code)` ends the application.
- Policy: fs `Read`/`Write` allowed only under `Program.roots` (`Path::starts_with`, so callers should pass canonical roots; `main.rs` canonicalizes the script's directory); `Listen` and `Connect` are always refused.
- The shell binary: the application ends when the last window closes; a close request unanswered within `CLOSE_DEADLINE` (2 s) closes the window; budget 5 s, heap 256 MiB; an `update` envelope has no target (no WebView) and is reported on stderr rather than dropped silently. `native` methods other than `window.*` and `app.quit` are refused with "this host has no native method <ns>.<method>".
- `setProperty` batches property writes per form and control and sends them from a microtask queued at the first write, so a handler's writes go out as one envelope per form without a flush call.
- `tests/desktop.rs` writes `broken.js` under `std::env::temp_dir()` and removes it afterwards.
