# Final feature/API verification index

Authoritative final snapshot: 2026-10-27T18:15:46.597Z; [delivery receipt](../.omx/evidence/prio-one/solo-final/README.md). Earlier baseline/gap tables below are historical, preserved to make the coverage ratchet auditable.

Bun1.3 and Node24: **356 functional cases each**. React:19 Bun/18 Node+1 Bun-only skip. Compiled Node22.0/24:13 each. Compiler contracts:19. All13 guide examples execute in the functional suite. No required feature is represented only by a draft fixture.

## Executed file counts (same suite on both runtimes)

| File | Executed/passed cases |
|---|---:|
| `tests/bun/core/build-timeout.test.ts` | 40/40 |
| `tests/bun/core/class-token.test.ts` | 13/13 |
| `tests/bun/core/completion.test.ts` | 19/19 |
| `tests/bun/core/construct.test.ts` | 14/14 |
| `tests/bun/core/delivery.test.ts` | 32/32 |
| `tests/bun/core/discovery.test.ts` | 6/6 |
| `tests/bun/core/health.test.ts` | 6/6 |
| `tests/bun/core/hot.test.ts` | 6/6 |
| `tests/bun/core/inspect.test.ts` | 6/6 |
| `tests/bun/core/interceptors.test.ts` | 7/7 |
| `tests/bun/core/lazy-leaf.test.ts` | 12/12 |
| `tests/bun/core/lifetimes.test.ts` | 6/6 |
| `tests/bun/core/module-definition.test.ts` | 9/9 |
| `tests/bun/core/module-loader.test.ts` | 26/26 |
| `tests/bun/core/promises.test.ts` | 3/3 |
| `tests/bun/core/qualification.test.ts` | 14/14 |
| `tests/bun/core/regressions.test.ts` | 21/21 |
| `tests/bun/core/token-cache.test.ts` | 33/33 |
| `tests/bun/core/view.test.ts` | 3/3 |
| `tests/bun/docs/examples.test.ts` | 13/13 |
| `tests/bun/legacy/decorators.test.ts` | 7/7 |
| `tests/bun/server/ambient.test.ts` | 7/7 |
| `tests/bun/server/runtime.test.ts` | 15/15 |
| `tests/bun/standard/decorators.test.ts` | 10/10 |
| `tests/bun/standard/deferred.test.ts` | 2/2 |
| `tests/bun/static/container.test.ts` | 26/26 |

## Required features

| Item | Coverage/evidence |
|---|---|
| 0.1: Coverage tooling, preserved original floors and new module/lazy/testing floors | vitest.config.ts; current-coverage/; solo-final/coverage.log |
| 0.2: Final feature/API index and executed counts | tests/FEATURES.md; solo-final/{bun,node}-results.json |
| 0.3: Six characterization areas retained and passing | tests/bun/{core,server}; runtime/hot/inspect/interceptor/health/discovery suites |
| 0.4: Comparative benchmarks and tested explicit budget | scripts/bench*.ts; bench/results.json; solo-final/bench-*.log |
| 1.1: Token retention/reclamation and stable exposed/qualified identities | core/token-cache.test.ts; qualification.test.ts; cminject.token.ts |
| 1.2: React failures, retries, scope isolation and Suspense | tests/react/adapter.test.ts; solo-final/react-{bun,node}.log |
| 1.3: Root completion, retry/provenance and explicit context | cminject.construction.ts; core/completion.test.ts |
| 1.4: Deadlines, cancellation, stable signals and late cleanup | cminject.resolution.ts; core/build-timeout.test.ts |
| 1.5: Public generic/decorator contracts and negative compiler fixtures | tests/types; solo-final/{types-*,compiler-fixtures}.log |
| 2.1: Cohesive typed extraction; no runtime cycles/private escapes | core/{async,binding,view,construction,resolution,mutation,observe,host}; solo-final/dag.log |
| 2.2: Real explicit typed views and contextual/target receiver behavior | cminject.view.ts; core/view.test.ts; delivery.test.ts |
| 2.3: Per-root ambient attachments and neutral server import | server/ambient.test.ts; server/runtime.test.ts; cminject.context.ts |
| 2.4: Explicit exports, migrated imports and entry resolution | package.json; scripts/check-{imports,core-imports,browser}.ts |
| 3.1: Clean compiled inject distribution and declarations | tsconfig.build.json; solo-final/{clean-inject,types-dist-consumer,node22-dist,node24-dist}.log |
| 3.2: Clean decorate dependency build and dialect preservation | ../decorate; solo-final/{clean-decorate,decorate-bun,decorate-node}.log |
| 3.3: Direct Node minimum/current runtime test leg | tests/node/*.test.mjs; solo-final/node{22,24}-dist.log |
| 4.1: Root-owned module loading, transactions, import references and unload | cminject.module.ts; RegistrationView; delivery/module-loader suites |
| 4.2: Snapshots, safe restoration, LIFO overrides and test roots | cminject.testing.ts; Container snapshot/restore; delivery.test.ts; node/features.test.mjs |
| 4.3: Lazy/provider slots across factories/constructors/members/decorators | cminject.lazy.ts; construction/decorators; lazy-leaf/deferred/delivery suites |
| 4.4: Exact-arity typed factory tuples for all ten registration variants | contracts/container/view/mutation; tests/types/typed-factory*.test-d.ts |
| 4.5: Opt-in completeness, current-generation drift and observed cycles | graph/observe/inspect; delivery observed-only-cycle regression |
| 4.6: Interned qualified tokens and readable graph/injection names | cminject.token.ts; qualification.test.ts; node/features.test.mjs |
| 4.7: Ordered conditional candidates, fallback and conservative static validation | binding/mutation/resolution/graph; delivery condition/captive/ambiguity regressions |
| 4.8: Named module capability completeness and negative type checks | cminject.module.ts; tests/types/module-types.test-d.ts |
| 4.9: Native compiler metadata pass and split client/server integration | ../../tools/{ctsc,split}; solo-final/{ctsc,split,compiler-real}.log |
| 5.1: Thirteen guides/migrations and generated API reference | docs/*.md; docs/api; solo-final/typedoc.log |
| 5.2: Thirteen executed examples and concise README | examples/cminject.*.ts; tests/bun/docs/examples.test.ts |
| 6.1: Records reconciled with the actual final implementation/results | PRIO_ONE.md; DECISIONS.md; FIX-PLAN.md; CLAUDE.md; tests/FEATURES.md |

## Public IContainer surface

All ten lifetime/multi overload families: class-token/construct tests, delivery registration-facade test and exact tuple compiler fixtures. Source/runtime view conformance is checked by TypeScript and the view member inventory. New load/snapshot/restore/dependencyDrift/useAmbient members are exercised by delivery, module, inspect and server ambient suites. Resolve/get/tryGet/has/registration queries use core/static/cache/qualification suites. Scope creation and lifecycle use runtime/lifetimes/timeout/regression suites. Introspection, hooks, interceptors and health use their named characterization files. Type fixtures cover token/data/multi/decorator inference and negative cases rather than merely transpiling examples.

## Final measured coverage

All original per-file floors remain unchanged; extraction owners retain inherited floors. New lazy/module/testing entries have explicit floors. V8 totals:

| Statements | Branches | Functions | Lines |
|---:|---:|---:|---:|
| 92.29% | 85.59% | 94.39% | 96.32% |

<!-- FINAL-DELIVERY-INDEX-END -->

# cminject feature and verification baseline

Measured 2026-10-27 before Phase 0 characterization and later behavior work. Baseline source includes the previously completed React failure fix (1.2).

## Toolchain and commands

- Bun 1.3.0: package-local `.cache/bun/bun-darwin-aarch64/bun`.
- Node 24.14.1: compatible runner selected through `node` on PATH; default shell Node 23.11.0 was used only for an initial diagnostic run and is outside Vitest 5’s declared engine range.
- Vitest and coverage-v8 5.0.5; TypeScript 5.9.3.
- Added tools are installed in package `.cache/prio-one-tools` and linked into package `node_modules`; the existing workspace Vitest 5.0.5 runner is reused to keep one test API instance. A tooling-only coverage-provider resolution link is installed in workspace `node_modules`; workspace manifests and lockfile were not modified.
- Node/V8 baseline: `CMINJECT_COVERAGE=1 CMINJECT_EXPECT_RUNTIME=node node node_modules/vitest/vitest.mjs run --config vitest.config.ts --coverage --reporter=json --outputFile=.omx/evidence/prio-one/g00/coverage-tests.json` (Node 24 in PATH).
- Immutable initial reports: `.omx/evidence/prio-one/g00/coverage/` (HTML, JSON, JSON summary), `coverage-tests.json`, `coverage.log`, `coverage.exit`. Future runs write `.omx/evidence/prio-one/current-coverage/`.
- Bun scripts: `bun run test:bun`, `bun run test:bun:legacy`, `bun run test:react`. Worker setup fails if the requested runtime is not actually running; legacy script changes cwd to its tsconfig directory.

## Baseline results

Combined Node/V8: 116 registered cases, 113 passed, 2 failed, 1 pending/skipped. The failures are preserved, not hidden to manufacture a green baseline.

Known baseline failures: frozen thenable `resolve()` throws synchronously rather than returning a rejection (`lifetimes.test.ts:43`); the graph-diamond wall-clock assertion `<50ms` is contention/instrumentation-sensitive (`regressions.test.ts:189`). Both also failed under Bun. Bun core/standard/static: 88 passed / 90 cases; legacy: 7/7 passed; React: 19/19 passed including Bun-worker assertion.

### Executed cases by file

| Test file | Registered | Passed | Failed | Skipped |
|---|---:|---:|---:|---:|
| `tests/react/adapter.test.ts` | 19 | 18 | 0 | 1 |
| `tests/bun/core/class-token.test.ts` | 13 | 13 | 0 | 0 |
| `tests/bun/core/construct.test.ts` | 14 | 14 | 0 | 0 |
| `tests/bun/core/lifetimes.test.ts` | 6 | 5 | 1 | 0 |
| `tests/bun/core/regressions.test.ts` | 21 | 20 | 1 | 0 |
| `tests/bun/legacy/decorators.test.ts` | 7 | 7 | 0 | 0 |
| `tests/bun/static/container.test.ts` | 26 | 26 | 0 | 0 |
| `tests/bun/standard/decorators.test.ts` | 10 | 10 | 0 | 0 |

## Measured coverage and ratchet

Thresholds in `vitest.config.ts` are the measured per-file percentages below. They are starting floors, not desired final quality. No threshold was invented or lowered to avoid a failing test. New/moved responsibilities require fresh explicit thresholds. V8 does not run under Bun.

| Source file | Statements % | Branches % | Functions % | Lines % |
|---|---:|---:|---:|---:|
| **Total** | 85.61 | 76.86 | 82.3 | 91.25 |
| `src/cminject.hot.ts` | 0 | 100 | 0 | 0 |
| `src/cminject.inspect.ts` | 0 | 100 | 0 | 0 |
| `src/cminject.react.ts` | 95.16 | 90.9 | 93.33 | 100 |
| `src/core/cminject.container.ts` | 85.45 | 77.82 | 80 | 90.06 |
| `src/core/cminject.contracts.ts` | 100 | 100 | 100 | 100 |
| `src/core/cminject.core.ts` | 100 | 100 | 100 | 100 |
| `src/core/cminject.decorators.ts` | 88.6 | 82.35 | 89.28 | 95.23 |
| `src/core/cminject.errors.ts` | 85 | 75 | 72.72 | 84.61 |
| `src/core/cminject.graph.ts` | 90.33 | 82.57 | 91.83 | 97.17 |
| `src/core/cminject.token.ts` | 90.24 | 73.07 | 85.71 | 90.62 |
| `src/server/cminject.context.ts` | 77.77 | 50 | 75 | 83.33 |
| `src/server/cminject.runtime.ts` | 73.37 | 48.31 | 87.87 | 86.08 |
| `src/server/cminject.server.ts` | 100 | 100 | 100 | 100 |

The core/server barrel entries and declaration/enum mapping file `cminject.contracts.ts` produce no V8-mapped executable statements in this report; they are not claimed as independently covered. `coverage.include` still inventories every `src/**/*.ts` file. Their public exports/types also require the later distribution and compiler-backed type gates.

## Feature index

Counts below are executed parameter-expanded cases with a direct lexical reference in their own test call, based on the baseline JSON runner result and parsed TypeScript test bodies. They are not grep-line counts, file totals, or proof that all edge cases are covered. Cases may cover multiple rows; helper-only/indirect coverage is not guessed. Planned/new capability rows deliberately show gaps until their implementing stories land.

| Feature | Baseline test file(s) | Executed direct cases | Status |
|---|---|---:|---|
| dispose | `tests/react/adapter.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 60 | existing direct coverage |
| DiRuntime | `tests/bun/static/container.test.ts` | 3 | existing direct coverage |
| cycle detection | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/core/lifetimes.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 16 | baseline failure present |
| request leases | `tests/bun/static/container.test.ts` | 1 | existing direct coverage |
| registerInstance | `tests/react/adapter.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 13 | existing direct coverage |
| onInstance | `tests/bun/standard/decorators.test.ts` | 1 | existing direct coverage |
| multi registrations | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 11 | existing direct coverage |
| interceptors | `tests/bun/static/container.test.ts` | 1 | existing direct coverage |
| captive lifetimes | `tests/bun/core/lifetimes.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 7 | existing direct coverage |
| health checks | `tests/bun/static/container.test.ts` | 1 | existing direct coverage |
| member injection | `tests/bun/core/construct.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 13 | existing direct coverage |
| resource graph | `tests/bun/static/container.test.ts` | 2 | existing direct coverage |
| React useService | `tests/react/adapter.test.ts` | 18 | existing direct coverage |
| hot replacement | — | 0 | gap |
| tenant lifetime | `tests/bun/core/lifetimes.test.ts`<br>`tests/bun/static/container.test.ts` | 6 | existing direct coverage |
| inspect adapter | — | 0 | gap |
| resolveAll | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 5 | existing direct coverage |
| dependency graph | — | 0 | gap |
| async factories | `tests/react/adapter.test.ts`<br>`tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/core/lifetimes.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 84 | baseline failure present |
| sync/async resolution | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/core/lifetimes.test.ts` | 19 | baseline failure present |
| request lifetime | `tests/react/adapter.test.ts`<br>`tests/bun/core/lifetimes.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 9 | existing direct coverage |
| static graph validation | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 9 | baseline failure present |
| typed factories (planned) | — | 0 | gap |
| init/ready/disposal ordering | `tests/react/adapter.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 24 | baseline failure present |
| per-call data | `tests/react/adapter.test.ts`<br>`tests/bun/core/lifetimes.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 11 | existing direct coverage |
| introspection | `tests/bun/core/construct.test.ts`<br>`tests/bun/static/container.test.ts` | 4 | existing direct coverage |
| qualified/conditional bindings (planned) | — | 0 | gap |
| modules (planned) | — | 0 | gap |
| lazy/provider injection (planned) | — | 0 | gap |
| test snapshots/overrides (planned) | — | 0 | gap |
| TC39 decorators | `tests/bun/legacy/decorators.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 9 | existing direct coverage |
| decorator-free classes | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 9 | existing direct coverage |
| browser-safe core | — | 0 | gap |
| compiled Node runtime (planned) | — | 0 | gap |
| executable documentation (planned) | — | 0 | gap |

## IContainer surface index

Every member is listed once; overload families retain their declaration count. Readonly properties/settable policy members are indexed as well as methods. A zero is an explicit baseline gap, not a claim that the API can be removed.

| Member (declarations) | Baseline test file(s) | Executed direct cases | Status |
|---|---|---:|---|
| `scopeType` (1) | — | 0 | gap |
| `isDisposed` (1) | `tests/bun/core/regressions.test.ts` | 1 | existing direct coverage |
| `identity` (1) | `tests/bun/static/container.test.ts` | 2 | existing direct coverage |
| `createScope` (1) | `tests/react/adapter.test.ts`<br>`tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 10 | existing direct coverage |
| `createTenantScope` (1) | `tests/bun/core/lifetimes.test.ts`<br>`tests/bun/static/container.test.ts` | 4 | existing direct coverage |
| `createRequestScope` (1) | `tests/bun/core/lifetimes.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 5 | existing direct coverage |
| `singleton` (2) | `tests/react/adapter.test.ts`<br>`tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/core/lifetimes.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 70 | baseline failure present |
| `transient` (2) | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 5 | existing direct coverage |
| `scoped` (2) | `tests/react/adapter.test.ts`<br>`tests/bun/core/class-token.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 8 | existing direct coverage |
| `tenant` (2) | `tests/bun/core/lifetimes.test.ts`<br>`tests/bun/static/container.test.ts` | 4 | existing direct coverage |
| `request` (2) | `tests/react/adapter.test.ts`<br>`tests/bun/core/lifetimes.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 9 | existing direct coverage |
| `singletonMulti` (2) | `tests/bun/core/class-token.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 8 | existing direct coverage |
| `transientMulti` (2) | `tests/bun/core/regressions.test.ts` | 3 | existing direct coverage |
| `scopedMulti` (2) | — | 0 | gap |
| `tenantMulti` (2) | — | 0 | gap |
| `requestMulti` (2) | — | 0 | gap |
| `registerInstance` (1) | `tests/react/adapter.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 13 | existing direct coverage |
| `registerClass` (1) | `tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 11 | existing direct coverage |
| `resolve` (1) | `tests/react/adapter.test.ts`<br>`tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/core/lifetimes.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 52 | baseline failure present |
| `resolveSync` (1) | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/core/lifetimes.test.ts` | 18 | baseline failure present |
| `resolveAll` (1) | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 5 | existing direct coverage |
| `instantiate` (1) | `tests/bun/core/construct.test.ts`<br>`tests/bun/legacy/decorators.test.ts` | 5 | existing direct coverage |
| `instantiateAsync` (1) | `tests/bun/core/construct.test.ts` | 1 | existing direct coverage |
| `injectMembers` (1) | `tests/bun/core/construct.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 4 | existing direct coverage |
| `injectMembersAsync` (1) | `tests/bun/core/construct.test.ts` | 1 | existing direct coverage |
| `autoInject` (1) | `tests/bun/core/construct.test.ts` | 6 | existing direct coverage |
| `get` (1) | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 9 | existing direct coverage |
| `tryGet` (1) | `tests/bun/core/regressions.test.ts` | 1 | existing direct coverage |
| `isRegistered` (1) | `tests/react/adapter.test.ts`<br>`tests/bun/core/construct.test.ts` | 2 | existing direct coverage |
| `isRegisteredRecursive` (1) | — | 0 | gap |
| `has` (1) | — | 0 | gap |
| `hasRecursive` (1) | — | 0 | gap |
| `getRegisteredTokens` (1) | `tests/bun/core/construct.test.ts` | 1 | existing direct coverage |
| `replace` (1) | `tests/bun/static/container.test.ts` | 3 | existing direct coverage |
| `replaceBinding` (1) | — | 0 | gap |
| `unregister` (1) | `tests/bun/static/container.test.ts` | 2 | existing direct coverage |
| `unregisterBinding` (1) | — | 0 | gap |
| `clearInstances` (1) | `tests/bun/static/container.test.ts` | 3 | existing direct coverage |
| `initializeOwned` (1) | `tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 2 | existing direct coverage |
| `dispose` (1) | `tests/react/adapter.test.ts`<br>`tests/bun/core/regressions.test.ts`<br>`tests/bun/legacy/decorators.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 56 | existing direct coverage |
| `healthCheck` (1) | `tests/bun/static/container.test.ts` | 1 | existing direct coverage |
| `healthCheckAll` (1) | — | 0 | gap |
| `addInterceptor` (1) | `tests/bun/static/container.test.ts` | 1 | existing direct coverage |
| `removeInterceptor` (1) | — | 0 | gap |
| `getScopeIdentity` (1) | — | 0 | gap |
| `getScopeId` (1) | — | 0 | gap |
| `getScopeChain` (1) | — | 0 | gap |
| `getRegistryEpoch` (1) | — | 0 | gap |
| `epochSnapshot` (1) | — | 0 | gap |
| `bindingOf` (1) | `tests/bun/core/construct.test.ts` | 1 | existing direct coverage |
| `multiBindingsOf` (1) | — | 0 | gap |
| `instanceState` (1) | `tests/bun/static/container.test.ts` | 3 | existing direct coverage |
| `cachedBindings` (1) | — | 0 | gap |
| `dependencyEdges` (1) | `tests/bun/static/container.test.ts` | 1 | existing direct coverage |
| `ownershipRecords` (1) | — | 0 | gap |
| `getDependencyGraph` (1) | — | 0 | gap |
| `getDependencyGraphSnapshot` (1) | — | 0 | gap |
| `getDependencyTree` (1) | — | 0 | gap |
| `getResourceGraphSnapshot` (1) | `tests/bun/static/container.test.ts` | 2 | existing direct coverage |
| `graph` (1) | `tests/bun/core/regressions.test.ts`<br>`tests/bun/static/container.test.ts` | 5 | baseline failure present |
| `issuesObserved` (1) | `tests/bun/static/container.test.ts` | 1 | existing direct coverage |
| `validate` (1) | `tests/bun/core/class-token.test.ts`<br>`tests/bun/core/construct.test.ts`<br>`tests/bun/static/container.test.ts`<br>`tests/bun/standard/decorators.test.ts` | 7 | existing direct coverage |
| `onInstance` (1) | `tests/bun/standard/decorators.test.ts` | 1 | existing direct coverage |
| `discover` (1) | `tests/bun/standard/decorators.test.ts` | 2 | existing direct coverage |

## Remaining gates

G00 records the existing baseline and gaps. Phase 0 characterization, public type assertions, Node dist smoke, browser/import policy, compiler integration, benchmark semantics, docs examples and final independent review are still required by their later stories. Passing a source test is not a claim that compiled Node distribution support already exists.
