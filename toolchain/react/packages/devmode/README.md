# @lunarstorm/devmode

In-app dev mode for React apps: a developer console and a Postman-style API executor in one bottom panel.

```tsx
import { DevMode, installCapture } from "@lunarstorm/devmode";

if (import.meta.env.DEV) installCapture(); // optional: keep logs from before the first render
root.render(<><App /><DevMode enabled={import.meta.env.DEV} env={{ baseUrl: "http://localhost:3000" }} /></>);
```

Toggle with `` Ctrl+` `` (prop `hotkey`) or the floating **DEV** button (`launcher={false}` hides it).

## Console
- Captures `console.log/info/warn/error/debug`, uncaught errors, unhandled rejections, `fetch` and `XMLHttpRequest` (axios) traffic. Originals still run.
- Filters (All/Errors/Warnings/Info/Network) with counts, text filter, Clear (`Ctrl+L`).
- Objects expand lazily (circular-safe). REPL evaluates in global scope; `↑/↓` history, `$_` = last result, promises are awaited.
- `→ API` on a request row opens it in the API executor.

## API executor
- Method, URL, params, headers, body (JSON/text/urlencoded form), auth (bearer, basic, API key header).
- `{{var}}` environment (Env tab); unknown variables are flagged.
- Saved collection, last 50 in history, cURL import (Network panel "Copy as cURL") and export.
- Response: status, time, size, JSON tree or raw, headers, copy. `Ctrl+Enter` sends; Cancel aborts.
- Persisted in localStorage under `lunarstorm.devmode.*`.

## Public surface
`DevMode`, `installCapture`, `capture`/`Capture`, `evaluate`, and the pure request helpers
`buildRequest`, `execute`, `interpolate`, `envVars`, `parseCurl`, `toCurl`, `splitQuery`, `newRequest`.

## Decisions and hazards
- Panel renders through a portal on `document.body` with scoped `.lsdm-*` CSS; no bundler CSS support needed.
- Capture is a module-level store and is never uninstalled by `<DevMode>`, so logs survive closing and StrictMode remounts.
- Listener notification is batched in a microtask: React logs during render, and a synchronous setState there warns.
- Requests run from the page, so the browser's CORS rules apply; failures show as status 0 with the reason.
- The REPL uses indirect `eval`; a CSP without `unsafe-eval` blocks it and the error is logged.

## Commands
`pnpm --filter @lunarstorm/devmode test` · `pnpm --filter @lunarstorm/devmode typecheck`
