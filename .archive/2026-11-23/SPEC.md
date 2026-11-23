# Zebra specification

This replaces the plan in `ZEBRA.pdf`. Section 9 lists every problem found in
that plan and the section here that resolves it.

Status: the model types and the starter exist (`starters/app`) and typecheck.
Nothing else in this document is built in this directory.

## 1. Principle: model → compile → output

1. **Model.** The developer writes a TypeScript project that declares the
   application in JSX. It is a configuration file. It is never executed.
2. **Compile.** The compiler reads the model from the AST and generates three
   things: a WebView frontend, a backend, and the native host configuration.
3. **Output.** A generic native host runs the generated frontend and backend.

What runs in the WebView is a compile output: a React app generated from the
React output starter (`starters/react/`). No React appears in the developer's
project, and developers never edit the generated app.

## 2. The model (`starters/app`)

- **Schema.** `desktop/model.ts` defines every element and prop. The prop types
  are the schema: `tsc` validates a project, and the compiler accepts exactly
  what they describe. The host's Rust config structs are generated from the
  same types, so the two cannot drift.
- **Custom JSX.** `desktop/jsx-runtime.ts` allows only the lower-case element
  names. There are no components and no fragments, so a tag is resolved without
  following imports.
- **Elements.** `application`, `window`, `menu`, `item`, `separator`, `tray`,
  `command`, `shortcut`, `document`, `service`, `permissions`, and the content
  elements `stack`, `text`, `button`, `textField`.
- **The whole UI is in the model.** Everything the user sees inside a window is
  declared with content elements, and the compiler maps each one to a React
  component in the output starter. The four content elements that exist today
  are a placeholder; the full control set is not designed yet.
- **Fidelity.** Every native prop names the TAO, WRY or muda call it compiles
  to. Things only the build needs (event loop, raw input, native handles) have
  no model form.

### 2.1 Parse rules (compile errors, not lint warnings)

- A prop value is a literal, an identifier naming a `state()` or `setting()`
  constant, or an identifier naming an imported function. Calls, spreads,
  conditionals and computed values are rejected.
- Props are flat scalars; one prop maps to one native attribute.
- Elements refer to each other by `id` string. Every reference must resolve,
  and every `id` is unique within its element kind.
- Nesting is checked by the compiler; the type system cannot do it.

### 2.2 Static, initial and bound props

- **Static.** A literal. Written to the manifest and applied before any
  JavaScript runs.
- **Initial.** Size, position, maximized, fullscreen and focus. Read once when
  the window is created, then changed only through `WindowHandle`, so a bound
  prop never fights the user.
- **Bound.** A `state()` or `setting()` constant. Generated code calls the
  native setter each time the state changes.

### 2.3 Code in the model

- **Actions.** A `command` names an imported function in `run` and a `side`
  (`frontend` or `backend`). The body is opaque; the compiler moves it to that
  side.
- **Event handlers and services** always run on the backend. The events come
  from the native event loop, and the backend exists before any window.
- **State.** The authoritative store is on the backend. Each window holds a
  mirror. A frontend `set` applies locally and is sent over IPC; the backend
  broadcasts the result to every window.
- **Native calls** inside action bodies go through `@desktop/api`.

### 2.4 Project layout

```
src/app.tsx        default export: the <application> element
src/parts/*.tsx    default export: one element, appended to <application>
src/state.ts       state() and setting() constants
src/actions.ts     functions referenced by commands, events and services
res/               icons, locale files
package.json       giraffe: { schema, features }
```

`src/parts/` is what makes features addable without editing existing files:
the compiler appends each file's element to `<application>` in file-name order.

## 3. Scaffolding

- **The starter is a product.** `starters/app` typechecks and compiles in CI in
  two variants: minimal, and every feature on.
- **Create is copy, rename, overlay.** The CLI copies the starter, renames the
  fixed tokens (`giraffeSample`, `GiraffeSample`, `giraffe-sample`) and applies
  the chosen overlays.
- **Overlays only add.** An overlay is new files (under `src/parts/`,
  `src/`, `res/`) plus dependency entries for `package.json`. It never modifies
  a file the starter or the developer owns. `create` and `add <feature>` are
  therefore the same code path, and `add` is safe after the developer's edits.
- **Permissions are derived.** The compiler collects the native APIs that
  action bodies reference and fills `apis` and `frontend` from them.
  `<permissions>` supplies what cannot be derived (filesystem roots, executable
  paths, environment names) and may narrow the derived lists.
- **Upgrades.** `giraffe.schema` is an integer. The CLI refuses a project with
  a newer schema, and `app migrate` runs the ordered codemods between the
  project's schema and the CLI's.

## 4. Build pipeline

One library crate, linked into both the CLI and the debug runtime.

| Stage | Input | Output |
|---|---|---|
| 4a Model extraction | project sources (AST) | project model; errors for 2.1 violations |
| 4b Frontend generation | content elements, `frontend` actions, state | a React app, generated by filling the React output starter (`starters/react/`), wrapped in the shell |
| 4c Backend generation | `backend` actions, handlers, services, state store | one backend module, then a VM artifact |
| 4d Bridge generation | every command's `side` | typed proxies and runtime argument validators |
| 4e Host configuration | application, windows, menus, tray, shortcuts, documents, permissions | manifest section |

- **Archive.** HTML, JS, CSS, icons, the VM artifact and the manifest with
  hashes. The manifest records the artifact kind and the engine version; the
  host refuses a mismatch.
- **Shell.** The frontend is wrapped in `<ShellDebug>` for dev and debug, and
  in `<Shell>` for build and release. Both are framework-owned React components
  with their own error boundary around the generated app.
- **Typecheck.** `tsc` runs in the CLI process beside the debug runtime, out of
  band. Its diagnostics are forwarded to the shell as `dev.error`.

## 5. Runtime layering (host, runtime, VM)

The rule at both boundaries: the layer above depends only on a contract, and
anything that meets the contract is valid. The Runtime holds no host type and
no engine type.

- **Runtime interface (host calls down).**
  - `start`, `load`, `stop`.
  - `call(export, args)`: invoke an exported function and receive its promise
    result. This is how the host runs a command.
  - `post(task)`: a thread-safe handle that queues work onto the loop from any
    thread.
  - `poll()`: run the loop one step and return, so a host with its own event
    loop is never blocked. `run` is `poll` in a loop.
  - `register(bindings)`: host-supplied native bindings, added before `load`.
- **Host services (runtime calls up).** stdio, args, fs and net policy, exit,
  clock, entropy, environment, log sink.
- **Bindings.** The Runtime owns the generic ones: timers, module loader, fs
  and net under the host's policy. Everything desktop-specific (window, tray,
  menu, dialog, clipboard, notification, screen, shell, process) is registered
  by the host. There is one fs implementation, the Runtime's.
- **VM interface.**
  - Instance (heap, GC, job queue), Context, scoped and persistent handles, one
    native function signature, a capabilities query.
  - Host objects with finalizers, so a binding can own a native resource.
  - Capabilities: bytecode cache, snapshots, JIT, memory limits, termination,
    debug hooks.
  - A conformance suite ships with the interface.
- **Isolation.** A Context is a realm, not an isolation boundary: interrupt,
  memory limit and job queue belong to the Instance. Isolation and reload use a
  new Instance.
- **VM callbacks (VM calls up).** Module resolve and load (asynchronous, so
  dynamic `import()` works), unhandled rejection, interrupt check. A callback
  receives a context handle and never the Runtime, which rules out re-entrancy.

## 6. Native desktop process

The desktop host is one host of the Runtime in section 5. Its JS thread runs
that Runtime; the VM behind it is whichever engine the build selected.

- **Threads.** UI thread (TAO event loop, windows, WebViews), JS thread (the
  Runtime), and a worker pool for blocking natives (file dialogs, fs, process
  spawn). The debug runtime adds a watcher thread and compiler threads.
- **Startup order.** Read manifest → create startup windows, menus and tray
  from the host configuration → start the Runtime → load the backend → mount
  each WebView. No JavaScript is needed to show the first window.
- **IPC.** One session per WebView, started by the shell before the app mounts.
  Envelope `{version, kind, id, windowId, epoch, method, payload}` with
  correlation, cancel, deadlines and bounded queues. It carries API calls,
  state sync and `dev.*` messages. There is no second channel.
- **Security.**
  - A WebView may call only backend commands and the native methods in
    `permissions.frontend`. `process.spawn` is never callable from a WebView.
  - Every call is validated at the native boundary, backend command arguments
    included (validators from 4d).
  - Navigation is locked to the asset protocol's origin, and the default CSP
    allows only that origin.
- **Assets.** A custom protocol serves frontend modules. The debug runtime
  fills the store in memory; the release runtime reads the archive.

### 6.1 Two runtimes, same core

- **Debug runtime.** Prebuilt. Embeds the pipeline, the dev server and the file
  watcher, and rebuilds in memory. A compiler panic is caught at the pipeline
  boundary and shown as `dev.error`; the running app and its state stay up.
- **Release runtime.** Prebuilt as well. The archive is attached as a bundle
  resource (macOS), a resource section (Windows) or a sibling file in the
  package (Linux), before signing. No Rust toolchain is needed on the
  developer's machine, and building for another OS needs only that OS's
  prebuilt runtime.

### 6.2 What a change does in dev

| Change | Effect |
|---|---|
| Content element, `frontend` action | `dev.refresh` / `dev.css`; state kept |
| `backend` action, handler, service | new epoch (6.3) |
| Live window prop, menu, tray | host applies the native setter |
| Initial window prop, new or removed window | that window is recreated |
| Application identity, permissions, `package.json` | `dev.restart`: the host restarts the app |

### 6.3 Backend reload

- A new Instance replaces the old one; the command registry is swapped
  atomically and `backend.epoch` is sent to every shell.
- Calls in flight reject with a `reloaded` error. The epoch also blocks any
  late reply.
- Every native resource created from backend code (timers, spawned processes,
  file watchers, notifications) is owned by its epoch and released with it.
  Each service's `stop` runs first.
- Windows, menus and tray declared in the model are host configuration and
  survive the reload.
- State values are carried over by name when the type is unchanged.

## 7. Debug and release

- **Debug.** WebView devtools, plus a debug adapter on the JS thread through
  the VM's debug capability. Breakpoints map to the model's TypeScript through
  source maps. Paused time does not count against execution budgets.
- **Release.** CLI runs the pipeline → archive → attach to the prebuilt release
  runtime → package and sign per OS.
- **Browser tab.** The generated frontend can be served to a browser by the dev
  server for layout work. Native calls reject with `unavailable`, and every
  window opens as its own tab.

## 8. Build order

1. Model extraction (4a) with the parse rules, tested against `starters/app`.
2. Host configuration (4e) → manifest → windows and menus created with no JS.
3. Frontend and backend generation (4b, 4c) and the bridge (4d).
4. Runtime interface additions (`call`, `post`, `poll`, `register`), then the
   desktop host on top of the Runtime.
5. The pipeline inside the debug runtime, with the change table in 6.2.
6. Prebuilt release runtime with an attached archive.

## 9. Problems in `ZEBRA.pdf` and where they are resolved

| Problem in the plan | Resolved in |
|---|---|
| Project was a hand-written "Vite + React" app | 1, 2 |
| `export const meta` and `src/providers/*` as a second declaration mechanism | 2 (one mechanism: the JSX model) |
| Part 11 and 5c hard-coded QuickJS while the Runtime claimed engine neutrality | 4 (archive), 6 |
| Desktop bindings had no way into a host-agnostic Runtime; two fs implementations | 5 (bindings) |
| Runtime interface could not call a function, post from another thread, or step | 5 (runtime interface) |
| "One process, two threads" | 6 (threads) |
| Contexts described as isolation | 5 (isolation) |
| Reload left native resources and in-flight calls undefined | 6.3 |
| Project model never reached the host; no message for model changes in dev | 4e, 6 (startup order), 6.2 |
| Side of commands and services undefined | 2.3 |
| Permissions, navigation and CSP absent | 3, 6 (security) |
| VM interface lacked host objects, debug hooks, async module load, re-entrancy rule | 5 |
| Host services lacked clock, entropy, environment, log sink | 5 |
| No place for `tsc` in dev; a compiler panic killed the app | 4 (typecheck), 6.1 |
| `add` after edits needed a merge; no upgrade path | 2.4, 3 |
| Browser tab had no defined native behaviour | 7 |
| Per-app Rust link for release | 6.1 |
| Typed bridge generated proxies but no validation | 4d |
| Pipeline existed twice (CLI and debug runtime) | 4 |
| No ordering | 8 |
