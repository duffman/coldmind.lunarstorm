# Deterministic React async singleton fixture

## Scope and failure evidence

Only `tests/react/adapter.test.ts`, test `useService suspends until an async singleton is built`, changed. The old 10ms factory delay could expire while asynchronous React mounting was still flushing, so the fallback assertion observed `loaded` instead of `loading`. `preexisting-flake-bun.log` preserves the existing G03 failure: 18 passed / 1 failed at that assertion. No random reproduction was needed.

The fixture now returns a manually controlled Promise. It asserts `loading` before releasing that Promise, releases inside `act`, awaits the Promise, and asserts `loaded`. Neither assertion was weakened. All other original and added failure/reset/scope/StrictMode tests are preserved. `fixture.patch` compares against the exact archived pre-edit file, not the already-dirty Git baseline.

Pre-edit source archived without overwrite under repository `.archive/2026-10-27/revisions/react-fixture/packages/foundation/inject/tests/react/adapter.test.ts`.

## Verification

Runtimes: `/Users/patrik/.nvm/versions/node/v24.14.1/bin/node` (v24.14.1) and `.cache/bun/bun-darwin-aarch64/bun` (1.3.0). All Vitest runs used the stable workspace `node_modules/vitest/vitest.mjs` and `--config vitest.react.config.ts`. `CMINJECT_EXPECT_RUNTIME` verifies the actual test worker runtime; only Bun runs set `INJECT_EXPECT_BUN=1`.

Full-file commands:

```sh
CMINJECT_EXPECT_RUNTIME=node /Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/vitest/vitest.mjs run --config vitest.react.config.ts tests/react/adapter.test.ts
CMINJECT_EXPECT_RUNTIME=bun INJECT_EXPECT_BUN=1 .cache/bun/bun-darwin-aarch64/bun node_modules/vitest/vitest.mjs run --config vitest.react.config.ts tests/react/adapter.test.ts
```

- `full-node.log`: 18 passed / 1 existing Bun-only runtime guard skipped (19 collected).
- `full-bun.log`: 19 passed / 19 collected, including actual Bun worker assertion.
- Repeat each full-file command with `-t 'useService suspends until an async singleton is built'` three times per runtime: all six runs passed the target (1 passed / 18 name-filtered skips per run), in `focused-{node,bun}-{1,2,3}.log`.

Strict compile commands, executed with both runtimes above:

```sh
<RUNTIME> node_modules/typescript/bin/tsc --noEmit -p tsconfig.json
<RUNTIME> node_modules/typescript/bin/tsc --noEmit --strict --skipLibCheck --target ES2022 --module ESNext --moduleResolution Bundler --lib ES2022,ESNext.Decorators,DOM --types node --allowImportingTsExtensions tests/react/adapter.test.ts
```

Source outputs: `source-{node,bun}.log`. Fixture outputs: `strict-fixture.log` (Node), `strict-fixture-bun.log` (Bun). Narrow whitespace check: `git diff --check -- tests/react/adapter.test.ts`, recorded in `diff-check.log`. Matching `.exit` files preserve exit status for each check. No source/config/manifests or workflow state changed by this lane.
