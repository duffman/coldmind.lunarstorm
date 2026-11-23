# PRIO_ONE final delivery receipt

Completed solo under the superseding user-directed sequence: write all code/basic tests without running checks; typecheck; fix; directly review against requirements; then run tests and final validation. No subagents were used in this execution pass. Earlier cancelled orchestration receipts remain historical, not current completion status.

Recorded: 2026-10-27T18:15:46.597Z.

## Fresh verification

- Bun1.3.0: **356/356** functional cases, **26 files**, including all13 executable guide examples.
- Node24.14.1: **356/356** same functional cases.
- React: Bun19/19 with runtime guard; Node18 passed and1 intentionally Bun-only skipped.
- Direct compiled distribution: **13/13** on exact Node22.0.0 and Node24.14.1, including new feature/override/module cases.
- Compiler-backed contracts:19/19; source, delivery and separate unaliased dist consumer checks pass.
- Clean decorate→inject builds, explicit imports, runtime DAG and browser gate pass.
- Decorate:17 cases each runtime; inject legacy current-working-directory leg7/7.
- ctsc:11 Node cases; split:13 Bun cases/36 assertions; actual emitted inject fixture executes on Node and Bun.
- TypeDoc: generated successfully with no warnings/errors.
- Benchmarks:20 cells,17 supported and3 explicitly N/A; configured3us/ratio4 budget passes and impossible1e-9us budget exits1.
- V8: statements **92.29%**, branches **85.59%**, functions **94.39%**, lines **96.32%**. All original floors retained.
- Package dry-run includes source, JS and declarations; no publishing or git mutations.

## Requirement reconciliation

| ID | Delivered | Implementation/evidence |
|---|---|---|
| 0.1 | Coverage tooling, preserved original floors and new module/lazy/testing floors | vitest.config.ts; current-coverage/; solo-final/coverage.log |
| 0.2 | Final feature/API index and executed counts | tests/FEATURES.md; solo-final/{bun,node}-results.json |
| 0.3 | Six characterization areas retained and passing | tests/bun/{core,server}; runtime/hot/inspect/interceptor/health/discovery suites |
| 0.4 | Comparative benchmarks and tested explicit budget | scripts/bench*.ts; bench/results.json; solo-final/bench-*.log |
| 1.1 | Token retention/reclamation and stable exposed/qualified identities | core/token-cache.test.ts; qualification.test.ts; cminject.token.ts |
| 1.2 | React failures, retries, scope isolation and Suspense | tests/react/adapter.test.ts; solo-final/react-{bun,node}.log |
| 1.3 | Root completion, retry/provenance and explicit context | cminject.construction.ts; core/completion.test.ts |
| 1.4 | Deadlines, cancellation, stable signals and late cleanup | cminject.resolution.ts; core/build-timeout.test.ts |
| 1.5 | Public generic/decorator contracts and negative compiler fixtures | tests/types; solo-final/{types-*,compiler-fixtures}.log |
| 2.1 | Cohesive typed extraction; no runtime cycles/private escapes | core/{async,binding,view,construction,resolution,mutation,observe,host}; solo-final/dag.log |
| 2.2 | Real explicit typed views and contextual/target receiver behavior | cminject.view.ts; core/view.test.ts; delivery.test.ts |
| 2.3 | Per-root ambient attachments and neutral server import | server/ambient.test.ts; server/runtime.test.ts; cminject.context.ts |
| 2.4 | Explicit exports, migrated imports and entry resolution | package.json; scripts/check-{imports,core-imports,browser}.ts |
| 3.1 | Clean compiled inject distribution and declarations | tsconfig.build.json; solo-final/{clean-inject,types-dist-consumer,node22-dist,node24-dist}.log |
| 3.2 | Clean decorate dependency build and dialect preservation | ../decorate; solo-final/{clean-decorate,decorate-bun,decorate-node}.log |
| 3.3 | Direct Node minimum/current runtime test leg | tests/node/*.test.mjs; solo-final/node{22,24}-dist.log |
| 4.1 | Root-owned module loading, transactions, import references and unload | cminject.module.ts; RegistrationView; delivery/module-loader suites |
| 4.2 | Snapshots, safe restoration, LIFO overrides and test roots | cminject.testing.ts; Container snapshot/restore; delivery.test.ts; node/features.test.mjs |
| 4.3 | Lazy/provider slots across factories/constructors/members/decorators | cminject.lazy.ts; construction/decorators; lazy-leaf/deferred/delivery suites |
| 4.4 | Exact-arity typed factory tuples for all ten registration variants | contracts/container/view/mutation; tests/types/typed-factory*.test-d.ts |
| 4.5 | Opt-in completeness, current-generation drift and observed cycles | graph/observe/inspect; delivery observed-only-cycle regression |
| 4.6 | Interned qualified tokens and readable graph/injection names | cminject.token.ts; qualification.test.ts; node/features.test.mjs |
| 4.7 | Ordered conditional candidates, fallback and conservative static validation | binding/mutation/resolution/graph; delivery condition/captive/ambiguity regressions |
| 4.8 | Named module capability completeness and negative type checks | cminject.module.ts; tests/types/module-types.test-d.ts |
| 4.9 | Native compiler metadata pass and split client/server integration | ../../tools/{ctsc,split}; solo-final/{ctsc,split,compiler-real}.log |
| 5.1 | Thirteen guides/migrations and generated API reference | docs/*.md; docs/api; solo-final/typedoc.log |
| 5.2 | Thirteen executed examples and concise README | examples/cminject.*.ts; tests/bun/docs/examples.test.ts |
| 6.1 | Records reconciled with the actual final implementation/results | PRIO_ONE.md; DECISIONS.md; FIX-PLAN.md; CLAUDE.md; tests/FEATURES.md |

## Explicit boundaries

- The original Phase2 extraction reached861 readable Container lines. The completed feature pass adds registry/module orchestration; current Container is1041 lines. The under900 figure is a historical extraction target, not a claim about the final file. No minification was used to hide growth.
- `composeModules` statically certifies finite inferred/named module tuples, not dynamically erased imports or runtime token identity. Ordinary load remains the runtime route for dynamic graphs.
- Repeated explicit module loads share an owner handle; callback loading through a captured root can create an ordinary user-promise self-await. Use declared imports and the explicit registration facade.
- Source tests/typechecks deliberately select source routes; the separate dist consumer and direct Node22/24 tests use actual compiled exports without those aliases.
- No separate lint script exists; the actual static checks are TypeScript, import policy, cycle/browser checks and whitespace validation.

Evidence commands and per-command exit files: `run-final.sh`, `*.log`, `*.exit` in this directory. Immutable coverage copy: `coverage/`.
