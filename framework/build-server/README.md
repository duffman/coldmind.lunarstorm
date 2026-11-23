# LS build server

A read-only analysis service for LS desktop projects. It parses TypeScript and JSX with the TypeScript compiler API; it does not execute application modules. The extracted project model is intended as input to future frontend, application-side, and host generators.

## Requirements

Bun and an installed TypeScript compiler (`tsc` on `PATH` or an available TypeScript package). The compiler loader is currently shared with `framework/templates/lsapp/tooling/typescript.ts`.

## Analyze a project

From this directory:

```sh
bun run analyze ../templates/lsapp
```

The command prints a JSON model. Diagnostics with severity `error` produce a nonzero exit status. Paths in the model are project-relative except for the root; symbol IDs are generated from declarations and are not application-authored wiring strings.

## Run the local service

```sh
bun run serve
```

The server listens on `127.0.0.1:4321` by default. Pass a port as an argument to override it. `GET /health` returns the server status. `POST /analyze` accepts JSON containing an absolute `projectPath`; it returns the model with status 200, or 422 when analysis reports errors. The service rejects browser-origin requests and does not enable CORS. It is intended for trusted local development.

```sh
curl -X POST http://127.0.0.1:4321/analyze \
  -H 'Content-Type: application/json' \
  -d '{"projectPath":"/absolute/path/to/project"}'
```

## Model and boundaries

The extractor records project configuration, paired view files, initial view properties, component trees and literal properties, outlet and action connections, services and their method signatures, injection relationships, direct service uses, and source locations. Unsupported computed layouts and unresolved connections are reported as diagnostics. It resolves symbols with the TypeScript checker rather than running project code.

The current pass understands the `framework/templates/lsapp` conventions, including a project-local `common/index.ts`. It does not infer execution sides, trace arbitrary dynamic calls or aliases, or generate frontend and backend projects. In particular, the existing service decorators do not specify a runtime side, and synchronous calls and property reads cannot automatically be made into cross-process calls without changing their semantics. The existing `doc/plans/main.md` describes a separate `.view.tsx` model; that format is not parsed by this pass.

Run `bun test` for parser and HTTP-handler tests.
