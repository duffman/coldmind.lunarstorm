# @coldmind/config

Shared build config for the monorepo, imported by package name instead of by relative path: the base
`tsconfig` and a vitest config factory. Private, no runtime code, no `src/`.

## Public API

Two entries in `exports`:

- `@coldmind/config/tsconfig.base.json`: `target`/`module` `ESNext`, `moduleResolution: Bundler`, `strict`,
  `noUncheckedIndexedAccess`, `noEmit`, `allowImportingTsExtensions`, `types: ["bun"]`.
- `@coldmind/config/vitest`: `shared(overrides?)`, `onBun` (true when vitest runs on Bun).

```json
{ "extends": "@coldmind/config/tsconfig.base.json", "compilerOptions": { "jsx": "react-jsx" }, "include": ["src"] }
```

```ts
import { defineConfig } from "vitest/config";
import { shared } from "@coldmind/config/vitest";

export default defineConfig(shared({ test: { name: "@coldmind/rpc" } }));
```

`shared` merges `overrides` over two plugins and one setting (all in `vitest.shared.ts`):

- `coldmind:standard-decorators`: runs after Oxc; anything that still starts a line with `@name` is a standard
  (TC39) decorator, which neither Bun nor Node parses, so esbuild lowers it.
- `coldmind:bun-import-meta`: rewrites `import.meta.dir` to `import.meta.dirname`.
- `test.fsModuleCachePath`: `<cwd>/node_modules/.vitest-cache`, one per package.

## How it fits

Root `package.json` depends on it (`workspace:*`); `vite`, `vitest`, `esbuild` and `typescript` are root devDependencies,
not this package's. Consumers of `@coldmind/config/vitest`: `foundation/{core,inject,model,decorate}`,
`reactive/{wire,bus}`, `vcl/contract/{rpc,http-routes}`, `toolchain/react/packages/{shell,bun}`. The `vcl/*` and
`net/client` packages extend the tsconfig. It carries no part of the VCL; it only keeps the packages building alike
(workspace comment in `pnpm-workspace.yaml`: names replace relative paths so packages can move).

## Scripts

- `pnpm --filter @coldmind/config test` runs `vitest run` on `vitest.shared.test.ts`.
- No `typecheck` or `build` script.

## Decisions and hazards

- Module cache per package. Vitest's default cache is `<workspace root>/node_modules/.vitest-cache`, cleared on every
  run, so `pnpm -r test` raced and failed with `ENOTEMPTY`. `vitest.shared.test.ts` runs real vitest on
  `fixtures/cache-probe` and asserts the cache lands in the fixture's own `node_modules`. Keep `fixtures/` out of
  collection: `vitest.config.ts` includes only `*.test.ts` at the package root.
- Decorators are detected by regex (`/^\s*@[A-Za-z_$]/m`): a decorator not at the start of a line is not lowered.
  The codebase only writes them at line start.
- `types: ["bun"]` in the base tsconfig means every extending package typechecks against Bun globals, including
  browser-side packages (`vcl/*`).
- `package.json` declares no dependencies, yet `vitest.shared.ts` imports `vite`, `vitest/config` and `esbuild`. They
  resolve from the root install; installing this package alone would fail.
- The test shells out to `pnpm exec vitest`, so it needs `pnpm` on `PATH` and a 60 s timeout.
