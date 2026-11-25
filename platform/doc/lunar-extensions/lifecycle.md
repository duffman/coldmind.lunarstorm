# Extension lifecycle and state

Status: **design, not implemented.** Spec: `doc/plans/main.md` §6 (threads), §6.1 (dev changes), §6.2
(backend reload).

## Hooks

| hook | when | thread | can it refuse |
|---|---|---|---|
| `setup(cx, config)` | once per process: configuration read, before the first window | the thread that builds the platform | yes: `Err` aborts startup |
| `on_window_ready(cx, form)` | a form's window and WebView exist | UI | no |
| `on_navigation(cx, form, url)` | a WebView is about to navigate | UI | yes: `false` cancels |
| `on_event(cx, event)` | a host event, see below | UI | no |
| `call(cx, call)` | a command | the command's `Thread` | by answering `Err` |
| `release_epoch(cx, epoch)` | the backend of `epoch` is retired | where the host retires an epoch | no |
| `Drop` | the process ends | the exiting thread | no |

- Hooks other than `call` do not block: they run on the UI thread, and a stalled UI thread freezes every
  window. Slow work goes to `cx.pool().spawn(...)`.
- A hook that panics is caught, logged as `extension <name>: <hook> panicked`, and the app goes on.

### `setup`

- Receives `config` (the extension's entry in `zebra.json` `extensions`, `null` if none) and a `Context`.
- It is the place to `cx.manage(...)` state and start threads.
- `Err(message)` aborts startup and the platform reports `extension <name>: <message>`. No window has opened.
- It does not run again on a backend reload (§6.2): windows, menus and tray survive a reload, and so do
  extensions. A change to `extensions` is a `dev.restart`, which runs `setup` in a fresh process.

### `on_navigation`

- Today the WebView's navigation is locked to the asset origin (§6). The hook can only refuse more; it cannot
  allow what the host refuses.
- With several extensions the first `false` cancels; every extension still sees the call.

### `on_event` and `AppEvent`

```rust
pub enum AppEvent {
    WindowCreated(String),            // form name
    WindowCloseRequested(String),
    WindowClosed(String),
    ExitRequested,
    Exit,
}
```

Decision: extensions observe; they do not veto. Tauri lets a plugin call `prevent_exit`. Here the backend owns
that decision through `Window.onCloseQuery` and `onQuit` (with its deadline, §6), and a second authority would
make "who stopped the app from closing" unanswerable. `Exit` is the place to flush; the process ends after
every extension has returned from it.

### `release_epoch`

- The existing `NativeRouter::release_epoch`, extended to every extension. §6.2: "Every native resource the old
  backend's code created is released with its epoch."
- An extension tags what a call creates (processes, watchers, channels, timers) with `call.done.epoch` and
  releases it here. The built-in `process` namespace is the model (`ProcessTable`, `native.rs`).
- A `Channel` of the retired epoch returns `false` from `send` from this point on.

## `Context`

```rust
impl Context {
    pub fn manage<T: Send + Sync + 'static>(&self, value: T) -> Result<(), String>;  // Err if T is managed
    pub fn state<T: Send + Sync + 'static>(&self) -> Arc<T>;                          // panics if T is not managed
    pub fn try_state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>>;
    pub fn pool(&self) -> &WorkerPool;
    pub fn config(&self) -> Arc<HostConfig>;                                          // the current application configuration
}
```

- State is a map from `TypeId` to `Arc<T>`, owned by the platform: one value per type, shared by every
  extension and by the application's own Rust. An extension should wrap its state in its own type, so two extensions
  do not collide on `HashMap<String, String>`.
- `state<T>` panics on a missing type because that is a bug in `setup`, found on the first call. Code that
  cannot know uses `try_state`.
- State of an epoch lives in the extension's state keyed by epoch and is removed in `release_epoch`. The map
  itself is per process.

## Exposing a Rust API

An extension's functions are reachable from the application's Rust and from other extensions through an extension
trait on `Context`:

```rust
pub trait StoreExt { fn store(&self) -> Arc<Store>; }
impl StoreExt for lunar_extension::Context {
    fn store(&self) -> Arc<Store> { self.state::<Store>() }
}
```

```rust
use lunar_extension_store::StoreExt;
cx.store().set("theme", "dark");
```

An extension that depends on another names it in `Cargo.toml` and calls through its extension trait; the platform
does not order `setup` calls by dependency. Decision: extensions run `setup` in registration order, and an extension
that needs another's state is registered after it. `try_state` in `setup` shows the order was wrong. Extensions
that depend on each other belong in a group, where the declaration order is the setup order (`groups.md`).

## Backend reload, step by step

1. The new `Instance` starts and `backend.epoch` changes (§6.2).
2. `release_epoch(old)` runs on every extension, in reverse registration order.
3. Channels of `old` stop delivering. Late calls of `old` are dropped by `Settler`.
4. Extension state, scopes and configuration are unchanged. The new backend calls `native.<extension>` as before.

## Tests an extension author gets

`lunar-extension` ships a `testing` module: `Harness::new(extension)` builds a `Context`, runs `setup`,
calls a command on the thread it declares, and returns the settled result and the `release_epoch` effects.
An extension's tests run without a window, a WebView or the runtime.
