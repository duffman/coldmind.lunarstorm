# Delphi-inspired desktop starter

An exploratory desktop application mockup built with a small, local JSX framework. It uses a form-and-control model with event handlers, inspired by Delphi, and an application event loop inspired by [Tao](https://docs.rs/tao/latest/tao/). The example is a project desk for selecting projects and editing their details.

## Run

Requires Bun and TypeScript (`tsc`) on your PATH. No packages need to be installed.

```sh
bun run dev
```

Open the loopback URL printed by the server. `bun run build` creates browser assets in `dist/`; `bun run typecheck` checks the starter. Restart `bun run dev` after editing source to rebuild the assets.

## Structure and scope

- `desktop/` contains the JSX runtime, desktop controls, renderer, and event-loop primitives.
- `src/` contains the example application's forms, handlers, and styling.
- `index.html` and `serve.ts` provide a small local browser host.

This is a browser mockup: its windows and controls are rendered in the page, and its data lives in memory. It does not integrate Tao, create OS windows, or provide native desktop APIs. Tao informs the separation of application, windows, and dispatched events; a native host and a rendering bridge would be needed to turn that model into a native desktop application.

This exploratory runtime executes JSX in the browser. It is separate from the AST-only compilation model described in the repository's `SPEC.md` and does not claim to implement that specification.
