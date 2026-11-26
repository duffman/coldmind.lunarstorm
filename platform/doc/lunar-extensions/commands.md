# Extension commands

Status: **design, not implemented.** See `README.md` for the trait and registration.

A command is one method of an extension's namespace: `native.<extension>.<command>(...args) -> Promise`.

## The path of one call

```
backend JS   native.store.set("theme", "dark")
  desktop.js   Proxy builds the call; arguments are JSON.stringify'd; ops.desktopNative(id, ns, method, json)
  host         parses the arguments (error: "bad arguments: ...")
  gate         HostConfig::allows_native("store", "set")      -> permissions.md
  router       built-ins first; then the extension map, keyed by full namespace (`media.audio` in a group)
               extension missing               -> "unknown native namespace store"
               command not in commands()       -> "unknown method store.nope"
  thread       Command.thread: Ui -> event loop, Pool -> WorkerPool, Inline -> the JS thread
  extension    Extension::call(cx, NativeCall)  ->  call.done.complete(Ok(value) | Err(message))
  host         Settler::settle(epoch, id, result)  ->  the promise settles
```

Steps up to the router exist today (`platform/desktop/app/src/runtime/lunarstorm.rs`, `native.rs`). The router
checks `commands()` itself, so `Extension::call` never sees an undeclared method and never has to guard the
thread.

## Declaring commands

```rust
const COMMANDS: &[Command] = &[
    Command { name: "get", thread: Thread::Inline },   // no blocking
    Command { name: "set", thread: Thread::Pool },     // writes a file
];
```

| `Thread` | runs on | use for |
|---|---|---|
| `Ui` | the UI thread (the OS requires it for windows, menus, dialogs) | anything that touches a window or a native widget |
| `Pool` | the 4-thread `WorkerPool` | anything that can block: files, network, child processes |
| `Inline` | the JS thread | a result that is ready now. It stalls every script event while it runs |

Decision: the extension declares the thread and the router applies it. The alternative, each extension choosing its
own thread, was dropped because the UI-thread hand-off (`NativeServices::ui`) is private to the router.

## Arguments and results

- Arguments are `Vec<serde_json::Value>`, the same as the built-ins. A helper per type keeps the errors uniform:
  `call.str(0)?`, `call.u64(1)?`, `call.scope::<T>()` (see `permissions.md`). A wrong argument is
  `argument 1 must be a string`, the wording `arg_str` already uses in `native.rs`.
- The result is a JSON value; `null` reaches the backend as `undefined` (existing rule in `runtime/lunarstorm.rs`).
- An `Err(message)` rejects the promise with `message`.
- A panic in `call` is caught by the router and rejects with `extension <name>: panicked`; the process goes on.
  Decision: the router catches it, because the platform already catches panics and an extension must not take the
  app down. A panic in a hook is caught the same way and logged.
- A result for a retired epoch is dropped by `Settler`; extensions inherit that.

## Channels

A command that reports more than once (progress, a watcher, a subscription) takes a `Channel`:

```js
await native.store.watch("theme", (value) => console.log(value));   // a function argument is a channel
```

```rust
let channel = call.channel(1)?;              // Channel: Clone + Send; send(Value) -> bool
cx.state::<Watchers>().add(key, channel);    // keep it; send from any thread later
call.done.complete(Ok(Value::Null));
```

How it travels (design; both ends change):

- `desktop.js`: the `native` proxy replaces each function argument by `{"$channel": <id>}` and keeps the
  callback in a map keyed by id. (Today `JSON.stringify` turns a function into `null`.)
- Host: `Channel::send` posts a `channel` envelope `{channel, data}` through `Backend::dispatch(epoch, ...)`, the
  path events already use. `desktop.js` `dispatch` looks the id up and calls the callback.
- `send` returns `false` when the epoch is retired. Channels die with their epoch: nothing is delivered to a
  new backend, and the extension releases its copy in `release_epoch`.

Decision: no separate `emit(event)` on `Context`. A channel is bound to one call, one epoch and one callback,
so there is no global event name to collide on and no listener to forget to remove.

## The JS module

`Extension::guest()` returns the source of an ES module. The platform registers it through
`Bindings::module("@extension/store", source)` (`runtime/core/src/bindings.rs`), compiled on first import. It
wraps `native.<extension>` with names and types:

```js
import { native } from "@desktop/api";

/** @param {string} key @returns {Promise<string | null>} */
export const get = (key) => native.store.get(key);
/** @param {string} key @param {string} value @returns {Promise<void>} */
export const set = (key, value) => native.store.set(key, value);
```

- The module is optional; `native.store.get(...)` works without it.
- It may import `@desktop/api` only. It cannot reach `ops` or any other extension.
- A `.d.ts` shipped next to it is the extension's public type surface.
