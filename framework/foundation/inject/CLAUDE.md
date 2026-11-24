# cminject: agent notes

Bun >=1.3 and Node >=22 are distribution targets; final source/dist verification passed and is recorded in `.omx/evidence/prio-one/solo-final/README.md`. Bun uses source, Node uses compiled ESM.

## Bun decorator facts

- Bun reads decorator settings from the tsconfig in the current working directory, not from one that a base config extends.
- Bun 1.3 ignores `experimentalDecorators` set in an extended tsconfig. The legacy-decorator test suite therefore has its own `tests/bun/legacy/tsconfig.json`, and it runs with that directory as cwd.
- Bun has no `Symbol.metadata`. `@coldmind/decorate` (`src/decorate.metadata.ts`) polyfills it with `Symbol.for('Symbol.metadata')`. Decorator dialect handling and per-class records live there; `cminject.decorators.ts` keeps only the DI meaning.

## Layout rules

- Files are named `cminject.<role>.ts`. A group gets its own directory only when it has more than two files.
  - `src/core/`: `cminject.core.ts` (entry `.`), `container`, `graph`, `token`, `contracts`, `errors`, `decorators`.
  - `src/server/`: `cminject.server.ts` (entry `./server`), `runtime` (`DiRuntime`), `context` (AsyncLocalStorage).
  - `src/cminject.hot.ts`, `src/cminject.inspect.ts`, `src/cminject.react.ts`: the `./hot`, `./inspect` and `./react` entries.
- `src/core/`, `cminject.hot.ts` and `cminject.inspect.ts` must not import `node:` modules. The browser bundle depends on it.
- `src/server/` is server-only and reachable through `cminject/server` alone.
- `cminject.react.ts` imports only core and React.

## Resolution

- Resolution passes an explicit `Resolver` (path, build, staged replacements, interceptor flag). No ambient async state.
- A cross-chain cycle is detected from the waits-for graph: a build records the builds it awaits, and a request for a build that already reaches the asker is a cycle.

## Current implementation

Core responsibilities are split across async/binding/view/construction/resolution/mutation/observe with a type-only host boundary. Root-owned modules, snapshots, deferred slots, typed factories and conditional candidates share these owners. Server import is neutral; DiRuntime attaches ambient resolution per root. No wildcard exports or source-path package imports.
