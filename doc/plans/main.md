# Zebra: specification

## 1. What it is

A developer builds a desktop application the Delphi way: forms, controls and
event handlers. A compiler turns that into a working native app.

1. **Model.** A TypeScript project. Each form has two files, like a Delphi unit and its `.dfm`:
   - a **view resource** (`*.view.tsx`): the form's controls, their properties
     and their event connections, written in JSX. It is a configuration file:
     easy to parse, and it carries every property the native side needs. The
     compiler never executes it.
   - a **form class** (`*.ts`): the handlers and the form's logic.
2. **Compile.** During the TypeScript compile, the compiler reads the view
   resources from the AST and generates three outputs:
   - a **React frontend**, by filling a React starter project with VCL
     components;
   - a **backend**: the form classes, bound to generated control proxies;
   - the **host configuration** for TAO and WRY.
3. **Output.** A generic Rust host opens the windows from the host
   configuration, runs the generated React in the system WebView and runs the
   backend on a JS thread.

React is an output detail. The developer never sees or edits it.

## 2. The model

### 2.1 View resources

```tsx
<form name="MainForm" caption="Project Register" width={960} height={650}
      position="screenCenter" onCloseQuery={form.formClosing}>
  <panel name="CommandBar" align="top" layout="row" gap={8} padding={12}>
    <button name="SaveButton" caption="Save" onClick={form.saveProject} />
  </panel>
  <listbox name="ProjectList" align="client" items={[]} onChange={form.selectProject} />
  <statusbar name="StatusBar" caption="Ready" />
</form>
```

- Only lower-case element names are valid tags. There are no components and no
  fragments, so a tag is resolved without following imports.
- The root element of a view resource is `<form>`. Its props are the window's
  properties.
- Every element has a `name`, unique within its form. The form class reaches a
  control by that name.

### 2.2 Parse rules (compile errors)

- A prop value is a literal (string, number, boolean, array of literals) or
  `form.<member>`, naming a handler on the form class. Calls, spreads,
  conditionals and computed values are rejected.
- Props are flat scalars. One prop maps to one native attribute or one React
  prop.
- There is no conditional or loop syntax. Visibility is the `visible` prop;
  repetition is a list control's `items`.
- Nesting is checked by the compiler: only container controls have children.

### 2.3 Form properties (the native window)

Each prop names the TAO or WRY call it compiles to.

| Prop | Values | Compiles to |
|---|---|---|
| `caption` | string | `with_title` / `set_title` |
| `width`, `height` | logical pixels | `with_inner_size` |
| `minWidth`, `minHeight`, `maxWidth`, `maxHeight` | logical pixels | `set_inner_size_constraints` |
| `left`, `top` | logical pixels | `with_position` |
| `position` | `designed`, `screenCenter`, `ownerCenter`, `default` | position computed from `current_monitor` or the owner |
| `borderStyle` | `sizeable`, `single`, `dialog`, `toolWindow`, `none` | `set_resizable`, `set_decorations`, `set_minimizable`, `set_maximizable` |
| `borderIcons` | array of `minimize`, `maximize`, `close` | `set_minimizable`, `set_maximizable`, `set_closable` |
| `windowState` | `normal`, `minimized`, `maximized`, `fullscreen` | `with_maximized`, `set_minimized`, `with_fullscreen` |
| `formStyle` | `normal`, `stayOnTop`, `stayOnBottom` | `set_always_on_top`, `set_always_on_bottom` |
| `visible` | boolean | `set_visible` |
| `icon` | project-relative path | `set_window_icon` |
| `showInTaskbar` | boolean | `set_skip_taskbar` |
| `transparent` | boolean | `with_transparent` |
| `color` | `#rrggbb` or `#rrggbbaa` | `set_background_color` |
| `theme` | `system`, `light`, `dark` | `set_theme` |
| `owner` | form name | `with_parent_window`, `with_owner_window`, `with_transient_for` |
| `modal` | boolean | disables the owner while open |
| `titleBar` | `visible`, `transparent`, `overlay`, `hidden` | macOS `with_titlebar_transparent`, `with_fullsize_content_view`, `with_titlebar_hidden` |
| `contentProtection` | boolean | `set_content_protection` |
| `allWorkspaces` | boolean | `set_visible_on_all_workspaces` |
| `cursor` | CSS cursor name | `set_cursor_icon` |
| `menu` | name of a `mainmenu` | muda menu bar |

Form events: `onCreate`, `onShow`, `onHide`, `onCloseQuery` (may cancel),
`onClose`, `onActivate`, `onDeactivate`, `onResize`, `onMove`, `onDropFiles`,
`onThemeChange`.

Size, position and `windowState` are read once when the window is created.
After that they change only through the form's methods, so the model never
fights the user.

### 2.4 Controls

Each control maps to one React component in the React VCL.

Every control accepts: `name`, `caption`, `align` (`none`, `top`, `bottom`,
`left`, `right`, `client`), `width`, `height`, `enabled`, `visible`, `hint`,
`tabOrder` and `popupMenu` (name of a `popupmenu`).

| Group | Element | Props and events beyond the common ones |
|---|---|---|
| Containers | `panel` | `layout` (`row`, `column`), `gap`, `padding` |
| | `groupbox` | as `panel`; draws a titled frame |
| | `scrollbox` | as `panel`; scrolls its content |
| | `pagecontrol`, `tabsheet` | `pagecontrol`: `activePage`, `onChange` |
| | `splitter` | sits between two aligned siblings and resizes them |
| | `toolbar`, `statusbar` | `toolbar` contains buttons |
| Display | `label` | none |
| | `image` | `src`, `stretch`, `proportional` |
| | `progressbar` | `position`, `min`, `max` |
| Input | `button` | `default`, `cancel`, `action`, `onClick` |
| | `edit` | `text`, `placeholder`, `readOnly`, `password`, `onChange` |
| | `memo` | as `edit`; multi-line; `wordWrap` |
| | `checkbox` | `checked`, `onClick` |
| | `radiogroup` | `items`, `itemIndex`, `onClick` |
| | `combobox` | `items`, `itemIndex`, `onChange` |
| | `trackbar` | `position`, `min`, `max`, `onChange` |
| Lists | `listbox` | `items`, `itemIndex`, `onChange` |
| | `listview`, `column` | `listview`: `itemIndex`, `onChange`, `onDblClick`; `column`: `caption`, `width`, `alignment` |
| | `treeview` | `onChange`, `onExpanded` |
| Non-visual | `mainmenu`, `popupmenu`, `menuitem` | `menuitem`: `caption`, `shortcut`, `checked`, `action`, `role`, `onClick` |
| | `actionlist`, `action` | `action`: `caption`, `shortcut`, `enabled`, `checked`, `onExecute` |
| | `trayicon` | `icon`, `hint`, `popupMenu`, `onClick` |
| | `timer` | `interval`, `enabled`, `onTimer` |

- **Rows are data, not markup.** `listview` and `treeview` receive their rows
  from the form class at runtime; the view resource declares only columns.
- **Look.** The React VCL owns the visual design. The model picks controls and
  layout; it carries no fonts or CSS.

### 2.5 Form classes

```ts
export class MainForm extends Form {
  private readonly statusBar = this.find<StatusBar>('StatusBar');
  readonly saveProject = (): void => {
    this.store.update(this.current());
    this.statusBar.caption = 'Saved.';
  };
}
```

- A form class extends `Form` and holds the handlers the view resource names.
- It reads and writes control properties through typed control objects
  (`Button`, `Edit`, `ListBox`, …). Writing a property updates the screen.
- It runs on the backend. Native calls go through `@desktop/api`.

### 2.6 The application

`src/app.tsx` default-exports one `<application>` element.

| Prop | Meaning |
|---|---|
| `id`, `name`, `version`, `icon` | Identity; bundle id on macOS |
| `mainForm` | Name of the form shown at startup |
| `singleInstance`, `keepAlive` | Process behaviour |
| `urlSchemes` | URL schemes the application handles |
| `activationPolicy` | macOS: `regular`, `accessory`, `prohibited` |
| `theme` | Default colour scheme of every form |
| `onReady`, `onActivate`, `onOpenFile`, `onOpenUrl`, `onQuit` | Application events |

Its children are application-wide non-visual components (`trayicon`,
`actionlist`), `<document>` elements (file types the application opens) and one
`<permissions>` element (filesystem roots, executables, environment names).
The native API permissions are derived from what the form classes reference.

### 2.7 Project layout

```
src/app.tsx                 the <application> element
src/forms/<name>.view.tsx   a form's view resource
src/forms/<name>.ts         its form class
src/models/                 application code used by form classes
res/                        icons, locale files
package.json                giraffe: { schema, features }
```

## 3. Scaffolding

- **The starter is a product.** It typechecks, passes its tests and compiles in CI in two variants: minimal, and every feature on.
- **Create is copy, rename, overlay.** The CLI copies the starter, renames the
  fixed tokens and applies the chosen overlays. `create` and `add <feature>`
  are the same code path.
- **Overlays only add.** A feature is new files (a form and its view resource,
  a model) plus dependencies. It never modifies an existing file, so `add` is
  safe after the developer's edits.
- **Upgrades.** `giraffe.schema` is an integer. The CLI refuses a project with
  a newer schema, and `app migrate` runs the ordered codemods between the
  project's schema and the CLI's.

## 4. Build pipeline

One library, linked into both the CLI and the debug runtime.

```
+-----------------------------------------------------------------+
| 4a MODEL EXTRACTION                                             |
| reads app.tsx and every *.view.tsx from the AST, never executes |
| errors: parse rules (2.2), duplicate names, bad nesting,        |
| handlers that do not exist on the form class                    |
+------+----------------------+----------------------+------------+
       |                      |                      |
+------v---------+   +--------v-------+   +----------v-----------+
| 4b FRONTEND    |   | 4c BACKEND     |   | 4e HOST              |
| copies the     |   | form classes   |   | CONFIGURATION        |
| React starter  |   | + generated    |   | application, forms   |
| and fills it:  |   | control        |   | as windows, menus,   |
| one VCL React  |   | proxies per    |   | tray, shortcuts,     |
| component per  |   | form           |   | documents,           |
| control        |   | -> VM artifact |   | permissions          |
+------+---------+   +--------+-------+   +----------+-----------+
       | 4d BRIDGE            |                      |
       |<- per form: the property and event table both sides share
+------v----------------------v----------------------v------------+
| ARCHIVE (build / release only)                                  |
| HTML + JS + CSS + icons + VM artifact                           |
| manifest: hashes, artifact kind + engine version, host config   |
+-----------------------------------------------------------------+
```

- **Frontend.** For each form, 4b emits a React tree of VCL components with the
  literal props from the view resource. The tree holds no application logic.
- **Backend.** For each form, 4c emits control proxies matching the view
  resource, so `this.find<ListBox>('ProjectList')` is typed and resolved at
  compile time.
- **Bridge.** A property write on a proxy is sent to the form's WebView, which
  updates the matching VCL component. A user event in the WebView is sent to
  the backend, which updates the proxy and calls the handler. The wire format
  is the VCL protocol contract.
- **Shell.** The generated app is wrapped in `<ShellDebug>` for dev and debug
  and in `<Shell>` for build and release. Both are framework-owned React
  components with an error boundary around the generated app.
- **Typecheck.** `tsc` runs in the CLI process beside the debug runtime, out of
  band. Its diagnostics are forwarded to the shell as `dev.error`.

## 5. Host, runtime and VM

```
+------------------------------------------------------------------+
| APPLICATION                                                      |
+--------------------------------+---------------------------------+
                                 | loaded by a host
          +----------------------+----------------------+
          |                      |                      |
+---------v---------+  +---------v---------+  +---------v---------+
| CLI APP HOST      |  | DESKTOP HOST      |  | <YOUR HOST>       |
| process, argv,    |  | section 6         |  | developer-defined |
| stdio, exit code  |  | own event loop    |  |                   |
+---------+---------+  +---------+---------+  +---------+---------+
          +----------------------+----------------------+
    owns Runtime (calls down)    |    implements host services (calls up)
+--------------------------------+---------------------------------+
| RUNTIME INTERFACE              | HOST SERVICES                   |
| start, load, stop              | stdio, args, exit,              |
| call(export, args) -> promise  | fs/net policy,                  |
| post(task)  from any thread    | clock, entropy, environment,    |
| poll()      one loop step      | log sink                        |
| register(bindings)             |                                 |
+--------------------------------+---------------------------------+
| RUNTIME                                                          |
| event loop, timers, module loader, fs and net under host policy  |
| plus the bindings the host registered                            |
| generic over the VM (bound at compile time)                      |
+--------------------------------+---------------------------------+
      owns VM (calls down)       |    implements VM callbacks (calls up)
+--------------------------------+---------------------------------+
| VM INTERFACE                   | VM CALLBACKS                    |
| Instance: heap, GC, jobs,      | module resolve/load (async),    |
|   limits, interrupt            | unhandled rejection,            |
| Context: one realm             | interrupt check                 |
| Handles: scoped + persistent   | each receives a context handle, |
| Native function signature      | never the Runtime               |
| Host objects with finalizers   |                                 |
| Capabilities query             |                                 |
| Conformance suite              |                                 |
+--------------------------------+---------------------------------+
                                 | conforms to (static dispatch)
          +----------------------+----------------------+
+---------+---------+  +---------+---------+  +---------+---------+
| VM ADAPTER A      |  | VM ADAPTER B      |  | <YOUR VM>         |
| wraps ENGINE A    |  | wraps ENGINE B    |  | direct or adapted |
+-------------------+  +-------------------+  +-------------------+
```

The rule at both boundaries: the layer above depends only on a contract, and
anything that meets the contract is valid. The Runtime holds no host type and
no engine type.

- **Runtime interface.** `call` runs an exported function and returns its
  result. `post` queues work from another thread. `poll` runs one step and
  returns, so a host with its own event loop is never blocked. `register` adds
  host-supplied bindings before `load`.
- **Bindings.** The Runtime owns only the generic ones. Anything specific to a
  kind of host is registered by that host. There is one fs implementation, the
  Runtime's, under the host's policy.
- **Isolation.** A Context is a realm. Limits, interrupt and the job queue
  belong to the Instance, so isolation and reload use a new Instance.
- **Capabilities.** Bytecode cache, snapshots, JIT, memory limits, termination,
  debug hooks.
- **Callbacks.** Module load is asynchronous. A callback receives a context
  handle and never the Runtime, which rules out re-entrancy.

## 6. Native desktop process

```
+-----------------------------------------------------------------+
| +-----------------------------+ +-----------------------------+ |
| | DEBUG RUNTIME               | | RELEASE RUNTIME             | |
| | prebuilt                    | | prebuilt                    | |
| | embeds the pipeline, dev    | | archive attached as a       | |
| | server and file watcher     | | resource before signing     | |
| +-----------------------------+ +-----------------------------+ |
| both contain:                                                   |
|                                                                 |
| +-------------------------------------------------------------+ |
| | UI THREAD                                                   | |
| | TAO event loop -> window manager -> WRY WebView(s)          | |
| | one window per form; windows, menus and tray are created    | |
| | from the host configuration before any JavaScript runs      | |
| | +---------------------------------------------------------+ | |
| | | WEBVIEW (system WebView), navigation locked to the      | | |
| | | asset origin                                            | | |
| | | +-----------------------------------------------------+ | | |
| | | | SHELL   <ShellDebug> | <Shell>                      | | | |
| | | | IPC client: one session per WebView, started        | | | |
| | | | before the form mounts                              | | | |
| | | | update queue: dev.refresh, dev.css, dev.error,      | | | |
| | | | dev.ready, dev.reload, dev.restart, backend.epoch   | | | |
| | | | +-------------------------------------------------+ | | | |
| | | | | GENERATED REACT FORM                            | | | | |
| | | | | VCL components, mirror of control properties    | | | | |
| | | | +-------------------------------------------------+ | | | |
| | | +-----------------------------------------------------+ | | |
| | +---------------------------------------------------------+ | |
| +------------------------------+------------------------------+ |
|                                |                                |
| +------------------------------v------------------------------+ |
| | IPC TRANSPORT                                               | |
| | envelope {version, kind, id, windowId, epoch, method,       | |
| | payload}; correlation, cancel, deadlines, bounded queues    | |
| | carries property updates, control events and dev.* ;        | |
| | no second channel                                           | |
| +------------------------------+------------------------------+ |
|                                |                                |
| +------------------------------v------------------------------+ |
| | JS THREAD                                                   | |
| | the Runtime of section 5 with the VM the build selected     | |
| | application, form classes, control proxies, limits          | |
| +------------------------------+------------------------------+ |
|                                |                                |
| +------------------------------v------------------------------+ |
| | NATIVE SERVICES (per-OS adapters), registered into the      | |
| | Runtime: app, window, screen, menu, tray, dialog, path,     | |
| | clipboard, notification, os, shell, process                 | |
| | blocking calls run on a worker pool                         | |
| +-------------------------------------------------------------+ |
|                                                                 |
| +-------------------------------------------------------------+ |
| | ASSET STORE + CUSTOM PROTOCOL                               | |
| | debug: in memory, filled by the embedded pipeline           | |
| | release: the attached archive                               | |
| +-------------------------------------------------------------+ |
+-------------------------------+---------------------------------+
                                |
+-------------------------------v---------------------------------+
| OS: macOS (WKWebView) | Windows (WebView2) | Linux (WebKitGTK)  |
+-----------------------------------------------------------------+
```

- **Startup order.** Read manifest → create the main form's window, menus and
  tray → start the Runtime → load the backend → mount the WebView.
- **Threads.** UI thread, JS thread, and a worker pool for blocking natives.
  The debug runtime adds a watcher thread and compiler threads.
- **Security.** A WebView sends control events and receives property updates,
  nothing else. It cannot call native methods; only backend code can. Every
  message is validated against the form's property and event table.
- **Two runtimes, same core.** Both are prebuilt. Release needs no Rust
  toolchain on the developer's machine; the archive is attached as a bundle
  resource (macOS), a resource section (Windows) or a sibling file (Linux).
- **Compiler failures in dev.** A panic in the embedded pipeline is caught at
  its boundary and shown as `dev.error`. The running app keeps its state.

### 6.1 What a change does in dev

| Change | Effect |
|---|---|
| Control or layout in a view resource | `dev.refresh` for that form; control properties kept |
| Form class or model code | backend reload (6.2) |
| Live form property, menu, tray | host applies the native setter |
| Creation-time form property, form added or removed | that window is recreated |
| `<application>` identity, permissions, `package.json` | `dev.restart`: the host restarts the app |
| Compile error | `dev.error`; `dev.ready` clears it |

### 6.2 Backend reload

- A new Instance replaces the old one, and `backend.epoch` goes to every shell.
- Every native resource the old backend's code created (timers, processes,
  watchers) is released with its epoch.
- Events in flight are dropped; the epoch blocks any late message.
- Control property values are carried over by form and control name where the
  control still exists. Each open form's `onCreate` runs again.
- Windows, menus and tray survive: they are host configuration.

### 6.3 Debug, release, browser

- **Debug.** WebView devtools, plus a debug adapter on the JS thread through
  the VM's debug capability. Breakpoints map to the form classes' TypeScript
  through source maps. Paused time does not count against execution budgets.
- **Release.** CLI runs the pipeline → archive → attaches it to the prebuilt
  release runtime → packages and signs per OS.
- **Browser.** A model project can run in a browser tab with its own
  in-page host, for working on forms without the native host.

## 7. Build order

1. Model extraction (4a): `<form>` as the view root, the parse rules, and the
   handler check, tested against the starter.
2. The controls of 2.4 in the model and in the React VCL.
3. Host configuration (4e) → manifest → the main form's window with no JS.
4. Frontend and backend generation (4b, 4c) and the bridge (4d).
5. Runtime interface additions, then the desktop host on top of the Runtime.
6. The pipeline inside the debug runtime, with the change table in 6.1.
7. Prebuilt release runtime with an attached archive.
