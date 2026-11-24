# @coldmind/devtools-shell

The parent of an app on the devtools dev server (`crates/server` in the Devtools product). The shell owns the page:

- **One WebSocket.** HMR frames and app channels share it. The server detects the package and serves
  its HMR runtime (`./hot`) as `/@devtools/client.js`, so the built-in client never opens a second
  socket. Pages get only the React Refresh preamble.
- **Error boundary.** The app renders inside it. A hot update re-mounts an app that failed to render.
- **Overlay** in a shadow root (app CSS cannot reach it): build errors from the server, load failures,
  render errors, uncaught errors and rejections, failed hot updates, and a badge while the server is
  unreachable.

| Failure | Shell behavior |
|---|---|
| Server compile error | Overlay until the next successful update |
| App entry fails to load | Overlay; next update reloads the page (no boundaries were registered) |
| Render error | Boundary + overlay; next hot update re-mounts the app |
| Uncaught error / rejection | Listed behind an error badge; the app stays mounted |
| Server restart | Badge, probe until it answers, reload |

## Usage

`index.html` loads `main.tsx`; the entry's default export is the root component.

```tsx
// src/main.tsx
import { startShell } from "@coldmind/devtools-shell";

startShell({ root: document.getElementById("root")!, load: () => import("./App.tsx") });
```

## Desktop apps

During a desktop build, Devtools reads `shell` from `app.config.ts`, selects the shell module,
and generates the React entry that wraps the default component exported from `src/main.tsx`.
The selected module is compiled into that frontend build.

`app create` includes `src/shell/DebugShell.tsx` for `app dev` and `app debug`, and
`src/shell/ProductionShell.tsx` for `app build` and `app release`. The debug shell shows the
frontend generation and backend epoch so you can see updates as they happen.

```ts
// app.config.ts
import type { DesktopConfig } from '@desktop/api';

export default {
  id: 'com.example.notes',
  name: 'Notes',
  version: '0.1.0',
  shell: {
    default: '@desktop/shell',
    debug: './src/DebugShell.tsx',
    production: './src/ProductionShell.tsx',
  },
} satisfies DesktopConfig;
```

| Command | Shell mode |
|---|---|
| `app dev` | `development` |
| `app debug`, with or without `--dap` | `debug` |
| `app build`, `app release` | `production` |

Devtools selects the current mode's module, then `default`, then the built-in `@desktop/shell`.
A string such as `shell: './src/MyShell.tsx'` selects the same module for every mode.
Relative module paths resolve from the application root. Package specifiers can select a shell
provided by an installed dependency. Since `app.config.ts` is TypeScript, its exported shell
setting can also be computed from the project's other settings.

Custom shell modules default-export a React component that accepts `ShellWrapperProps` from
`@desktop/shell`. They can compose the built-in wrapper to retain its IPC, error boundary, and
HMR handling while adding their own UI.

```tsx
// src/DebugShell.tsx
import { ShellWrapper, type ShellWrapperProps } from '@desktop/shell';

export default function DebugShell(props: ShellWrapperProps) {
  return <ShellWrapper {...props}><aside>Debug session</aside>{props.children}</ShellWrapper>;
}
```

```tsx
// src/ProductionShell.tsx
import { ShellWrapper, type ShellWrapperProps } from '@desktop/shell';

export default function ProductionShell(props: ShellWrapperProps) {
  return <ShellWrapper {...props}>{props.children}</ShellWrapper>;
}
```

The built-in wrapper opens one session over the WebView's native IPC bridge before loading the
component. React Fast Refresh, CSS updates, and calls from `@desktop/api` and `@desktop/backend`
share that session. Devtools passes the connection and wrapped app to the selected shell.

Components can call `useDesktopShell()` from `@desktop/shell` to read the current React update
generation and QuickJS backend epoch. For an older entry that mounts itself into `#root`, the
generated shell starts IPC and then loads the entry. Export the root component to put it inside
the shell's error boundary and context.

## API

- `startShell(options): ShellHandle`. Options are `root`, `load`, `transport` (a `ShellTransport` or its
  options), `overlay`, `catchWindowErrors`, `fallback(error, retry)`, `base`, `reload`, `importModule`.
  The handle exposes `transport`, `errors`, `channel(name)` and `stop()`.
- `getShell()` returns the running shell and throws if `startShell` has not been called.
- `useShellChannel<T>(name, onMessage?)` returns `send`. Messages travel as the custom event `app:<name>`.
- `useShellStatus()` returns `"connecting" | "open" | "lost" | "closed"`.
- `ShellTransport`, `ShellErrors` and `ShellBoundary` can be used on their own.
- `@coldmind/devtools-shell/hot` exports `createHotContext`, `updateStyle`, `removeStyle` and
  `handleHotFrame`, which is the `import.meta.hot` runtime.

On the server, a page's `app:<name>` message goes to every other page and to the host through
`DevServer::messages()`. `DevServer::publish(name, data)` sends to all pages.

## Example

```sh
cd packages/tools/devtools
PATH="$HOME/.cargo/bin:$PATH" cargo run -p lunarstorm -- serve examples/react-shell
```

Edit `examples/react-shell/src/App.tsx` while the server runs: component state survives. Throw in render and
save, the overlay appears; fix and save, the app re-mounts without a reload.

## Tests

```sh
bun run --filter @coldmind/devtools-shell typecheck
bun run --filter @coldmind/devtools-shell test
```

## Source organization

`start-shell.tsx` owns mounting and teardown; `app-loader.tsx` and
`error-boundary.tsx` isolate app loading/rendering failures. `transport.ts` owns
the socket, `protocol.ts` its frames, and `hot-runtime.ts` the module registry.
`error-store.ts`, `error-overlay.tsx`, and `terminal.tsx` supply the development UI.
The terminal is an app-channel console, not an OS terminal/PTY.

The canonical example lives at the product root in `examples/react-shell`.
Package IDs and the `.` / `./hot` entry points are unchanged; legacy source-path
exports remain compatibility mappings to the renamed implementations.
