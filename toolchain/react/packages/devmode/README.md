# @lunarstorm/devmode

The dev parent app for React apps. `DevApp` wraps the app in an error boundary and docks the dev mode below it: a developer console and a redis-cli style executor for the host's API.

```tsx
import { DevApp, installCapture } from "@lunarstorm/devmode";

if (import.meta.env.DEV) installCapture(); // optional: keep logs from before the first render
root.render(<DevApp enabled={import.meta.env.DEV} api={hostApiSpec}><App /></DevApp>);
```

Toggle with `` Ctrl+` `` (prop `hotkey`) or the floating launcher (`launcher={false}` hides it). The title bar's orange button minimizes the dock to its title bar; the dock is resized from its top edge (`react-resizable`).

## DevApp
- An error boundary around the app: a render error shows a fallback in the app's place (message, component stack, **Retry** remounts the app) and is logged in the console with its component stack.
- `enabled={false}` renders the app alone, so a production build can keep the wrapper.

## Console
- Captures `console.*`, uncaught errors, unhandled rejections, `fetch` and `XMLHttpRequest` traffic. The originals still run.
- Filters with counts, text filter, Clear (`Ctrl+L`), lazily expanded objects, and a REPL in global scope (`$_` is the last result).

## API executor
- redis-cli style: commands are typed as text, one per line (several lines are a pipeline), and replies print as redis-cli does (`1)`, `(integer)`, `(nil)`, `(error)`). Pub/sub pushes go to a Pushes tab.
- Transport per endpoint: `ipc://resp` uses the desktop host's TAO/wry WebView IPC envelope (contract in `src/transport.ts`); `ws://` and `wss://` carry RESP in binary WebSocket frames. RESP3 is negotiated with `HELLO 3`, with RESP2 as the fallback.
- The side panel is a TreeView of API calls grouped into categories:
  - your saved calls (★), filed under categories you name
  - the host's API: from the `api` prop (`HostApi::spec`, one category per area, calls `AREA.FUNCTION`), or, without one, from the server's `COMMAND DOCS`
  - filter, keyboard navigation, click loads a template, double-click runs it

## Public surface
`DevApp`, `DevMode`, `installCapture`, `capture`/`Capture`, `evaluate`, `RedisClient`, the transports (`ipcTransport`, `webSocketTransport`, `transportFor`, `defaultEndpoint`), the RESP codec (`encodeCommand`, `RespDecoder`, `formatReply`, `parseCommands`, `tokenize`), `ApiTree`, `parseApiSpec`, `parseCommandDocs`.

## Decisions and hazards
- The panel renders through a portal on `document.body` with scoped `.lsdm-*` CSS; no bundler CSS support is needed.
- Capture is a module-level store that `<DevMode>` never uninstalls, so logs survive closing and StrictMode remounts. Listeners are notified in a microtask, because React logs during render.
- The API spec, when given, is the tree's only source: the API is described once, in `HostApi::spec` (extensions/hostapi). Only spec version 1 is read.
- A socket close in the unmount cleanup is deferred one tick, so StrictMode's dev remount does not kill the connection.
- `dev/resp-mock.ts` is an in-memory Redis-protocol server used by the tests.

## Commands
`pnpm --filter @lunarstorm/devmode test` · `pnpm --filter @lunarstorm/devmode typecheck`
