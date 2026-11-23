# G01 service observability characterization

Date: 2026-10-27. Bun 1.3.0; Node v24.14.1; Vitest 5.0.5; TypeScript 5.9.3.

## Cases

- `tests/bun/core/interceptors.test.ts`: 7 executed cases. Parent/child before/after order; synchronous and asynchronous resolution; cache hits without rebuilding; nested requester/path, factory data and attempt; hook-error cause attachment preserving original failures; internal initialization suppression; removal isolation.
- `tests/bun/core/health.test.ts`: 6 executed cases. Lifecycle hook precedence and exact instance; receiver-bound instance fallback and no-hook default; missing versus uninitialized without building; descendant traversal and sibling exclusion; asynchronous/rejected/thrown/explicit unhealthy multi aggregation; local healthCheckAll registration inventory.
- `tests/bun/core/discovery.test.ts`: 6 executed cases. Tag/method metadata lookup without constructing; ancestor inheritance and documented root-first order; generated wrapper availability and no duplicate cached notifications; inherited hooks and idempotent unsubscribe; asynchronous hook completion; bound methods and detached tag arrays.

## Captured initial results

- `node-initial.log`: exit 1; 18 passed, 1 failed, 19 total, 3 files.
- `bun-initial.log`: exit 1; 18 passed, 1 failed, 19 total, 3 files.
- Sole failure: `discovery inherits ancestor registrations in root-first order without exposing child classes upward`.
- Actual contract defect: public `discover` documentation specifies root-first; implementation reverses the already root-first ancestor array. Child discovery returns local class before parent class. Expected contract assertion is retained unchanged; the sole core owner was notified for a separate source correction and final reruns. This test-only lane does not alter core source.
- `tests-typecheck.log`: exit 0, explicit strict compilation of all three new test files.
- `source-typecheck.log`: exit 0, package source typecheck.
- Focused `git diff --check`: exit 0. No lint script exists; no lint claim or build execution from this lane.

## Exact focused commands

All run from the inject package directory. Only the stable workspace Vitest entry is used; setup asserts actual runtime in each worker.

```sh
CMINJECT_EXPECT_RUNTIME=node /Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/vitest/vitest.mjs run --config vitest.config.ts tests/bun/core/interceptors.test.ts tests/bun/core/health.test.ts tests/bun/core/discovery.test.ts
CMINJECT_EXPECT_RUNTIME=bun .cache/bun/bun-darwin-aarch64/bun node_modules/vitest/vitest.mjs run --config vitest.config.ts tests/bun/core/interceptors.test.ts tests/bun/core/health.test.ts tests/bun/core/discovery.test.ts
/Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/typescript/bin/tsc --noEmit --target ES2022 --module ESNext --moduleResolution Bundler --strict --skipLibCheck --allowImportingTsExtensions tests/bun/core/interceptors.test.ts tests/bun/core/health.test.ts tests/bun/core/discovery.test.ts
/Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/typescript/bin/tsc --noEmit -p tsconfig.json
git diff --check -- tests/bun/core/interceptors.test.ts tests/bun/core/health.test.ts tests/bun/core/discovery.test.ts
```
