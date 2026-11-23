# G01 runtime, hot replacement, and inspection characterization

Date: 2026-10-27. Bun 1.3.0; Node v24.14.1; Vitest 5.0.5; TypeScript 5.9.3.

## Scope and cases

- `tests/bun/server/runtime.test.ts`: 15 executed cases covering start/idempotence/state, concurrent tenant configuration, ambient request context, eager and lazy requests, idempotent release, tenant drain with a live lease, drain followed by disposal, all eight error phases and observer-error isolation.
- `tests/bun/core/hot.test.ts`: 6 cases covering cold replacement, dependent rebuild ordering and identity, exactly-once disposal, failed candidate rollback, failed dependent rebuild rollback/candidate cleanup, and external ownership preservation.
- `tests/bun/core/inspect.test.ts`: 6 cases covering detached snapshot shape, declared/runtime binding edges and resource edges, dependency trees, ownership/scope identities, registry-only epochs, and graph issues.

## Red evidence and bounded correction

`node-red-contract.log` and `bun-red.log`: exit 1, 22 passed / 5 failed / 27 total, plus one unhandled rejection. Five failures prove missing `onError` notifications for start/configure-tenant/validate/initialize/acquire. A synchronous configure callback failure also left its initialization rejection unobserved.

The separately authorized source correction adds phase reporting, preserves the original error/cause despite throwing observers, and awaits an already-failed tenant initialization before propagating it. Original source archived without overwriting at `.archive/2026-10-27/packages/foundation/inject/src/server/cminject.runtime.ts` under the repository root.

Initial `node-red.log` also records three assertion-shape corrections: public string token names are quoted; epoch snapshots expose `ownerScopeId`. No inspection source was changed.

## Final focused commands

Run from the inject package directory; runtime setup checks the worker's actual runtime.

```sh
CMINJECT_EXPECT_RUNTIME=node /Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/vitest/vitest.mjs run --config vitest.config.ts tests/bun/server/runtime.test.ts tests/bun/core/hot.test.ts tests/bun/core/inspect.test.ts
CMINJECT_EXPECT_RUNTIME=bun .cache/bun/bun-darwin-aarch64/bun node_modules/vitest/vitest.mjs run --config vitest.config.ts tests/bun/server/runtime.test.ts tests/bun/core/hot.test.ts tests/bun/core/inspect.test.ts
/Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/typescript/bin/tsc --noEmit -p tsconfig.json
/Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/typescript/bin/tsc --noEmit --target ES2022 --module ESNext --moduleResolution Bundler --strict --skipLibCheck --allowImportingTsExtensions tests/bun/server/runtime.test.ts tests/bun/core/hot.test.ts tests/bun/core/inspect.test.ts
git diff --check -- src/server/cminject.runtime.ts tests/bun/server/runtime.test.ts tests/bun/core/hot.test.ts tests/bun/core/inspect.test.ts
```

Final runner logs: `node-final.log`, `bun-final.log`; exact exits: `final-exits.txt`. Source typecheck and strict focused-test compilation both exit 0 (`typecheck.log`, `tests-typecheck.log`); whitespace check exits 0. No package lint script exists; no lint claim is made. No build was run by this bounded lane.

Intermediate green/cache-named logs record toolchain failures, not final results: a concurrent runner transition left shared Vitest transform cache entries pointing to a second runner installation. G00 removed only those stale generated entries before the final direct-entry runs. No source behavior was changed to address runner cache failures. The unrelated baseline frozen-thenable failure is outside these focused files and was not hidden or source-fixed.
