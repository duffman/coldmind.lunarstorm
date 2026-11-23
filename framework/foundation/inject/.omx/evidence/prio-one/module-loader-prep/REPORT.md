# Module identity/ownership engine preparation — G08 integration pending

## Prepared artifacts

- `engine.ts`: a root-local `ModuleRegistry`, intended to be integrated into `cminject.module.ts` alongside the existing definition/composition functions. No live source changed.
- `.omx/prepared-tests/module-loader.test.ts`: 26 deterministic behavioral tests with a typed, deliberately minimal host.
- Local Vitest and strict TypeScript configurations, logs and numeric exit files in this directory.

The registry has no Container import, ambient transaction, Proxy, declaration fabrication, `any` or double assertion. Only genuine errors are imported at runtime. `ModuleOwnedBinding` narrows a real BindingHandle to `unregister`, the only authority the engine needs; actual captured handles are structurally accepted. No global operation queue serializes independent registration callbacks.

## Responsibility boundary

One registry must be created per root; descendants delegate load to that root. The host supplies:

1. `active()` — reject work on an ending/disposed root.
2. `runRegistration(module)` — invoke the module's captured/current validated registration callback through an explicit transaction-bound IContainer facade. The host must synchronously enter that callback before its first await, capture every registration path (including the direct fast path), retain the module diagnostic label, and return exact captured handles after success. **Before rejecting, the host must roll back that callback's captures and owned instances.** Unrelated interleaved registrations must never enter this list. Conflicting mutations require a core-owned identity/generation guard.
3. `cycleError(path)` — construct the real `DiGraphError` with genuine `module-cycle` kind after core extends `DiIssueKind`. The preparation does not lie to the current union with a cast.

The host remains responsible for factory/disposal ownership and late transaction rollback if root shutdown occurs during an async callback. `invalidate()` invalidates handles and rejects new loads without starting a second root-wide cleanup; pending transactions belong to root shutdown. An already-running exact-handle removal may finish; no further owned removal callbacks are started after invalidation.

Module callbacks should register bindings; their import graph belongs in `imports`. The transaction facade should reject structural/nested module loading. Awaiting a captured bare root's load of the callback's own pending module is an unsupported user-level self-await, analogous to awaiting one's own promise; this engine cannot infer asynchronous caller provenance in browser-safe core.

## Identity, concurrency and mutation rules

- Identity is the definition object, never its name. Imported modules use dependent-entry edges, not a hidden explicit-reference counter.
- Repeated/concurrent explicit load of an active identity returns one active handle. Calling unload releases that one explicit owner; duplicate imports/loads do not accumulate invisible references.
- A released handle stays inert. Explicitly acquiring an imported-but-still-live module again creates a fresh handle, so the old handle cannot remove the new explicit owner.
- Import graphs are snapshotted before asynchronous work. Cycle preflight runs before any pending waits are created. Imports load depth first and in source order. Shared import pending work is joined.
- Changing a loaded/loading definition's name, register identity or ordered imports is rejected on subsequent load and around its own callback. Unload uses the captured ownership graph. Definitions can change after the prior generation has fully unloaded.
- A load arriving during teardown waits for that generation's teardown and then creates a fresh generation. Its old teardown error remains the old unload caller's error, not a permanent poison pill.
- `load(A, B)` is **independent per-identity acquisition**, not an undocumented atomic batch. If B fails, successfully acquired A remains explicitly owned and can be retrieved/unloaded by another `load(A)`. This matches each module's individual transaction boundary; final API documentation/review should confirm this contract.
- Unloading one module removes its own exact bindings in reverse registration order, then releases imported dependencies in reverse order. Other explicit/dependent import owners prevent removal.
- Failed callback loading releases only its own import ownership and retries with fresh entries. Cleanup errors are aggregated while all remaining cleanup is attempted; the original registration error is first in rollback AggregateError.errors.

## Fresh verification

All checks ran in this workspace, with no installs and no git/lock/source/package changes.

- Bun **1.3.0**, `.cache/bun/bun-darwin-aarch64/bun node_modules/vitest/vitest.mjs run --config .omx/evidence/prio-one/module-loader-prep/vitest.config.ts`, `CMINJECT_EXPECT_RUNTIME=bun`: **26/26 passed**; actual runtime setup assertion active. `bun.log`, exit 0.
- Node **24.14.1**, same sole workspace Vitest **5.0.5** runner and config, `CMINJECT_EXPECT_RUNTIME=node`: **26/26 passed**. `node.log`, exit 0.
- TypeScript **5.9.3** strict noEmit: `node node_modules/typescript/bin/tsc -p .omx/evidence/prio-one/module-loader-prep/tsconfig.json`: zero diagnostics, `typecheck.log`, exit 0.
- Bun browser bundle and Node bundle: both build successfully with no server/Node dependency in the browser engine. `build-{browser,node}.log`, exits 0.
- Exact minimum Node **22.0.0** executes bundled engine identity/duplicate-load/exact-once unload smoke: `node22-smoke.log`, exit 0.
- AST static policy: zero explicit any syntax, double assertions, relative live-source imports or trailing whitespace. `static.log`, exit 0.

Tests cover identity/name separation, deterministic import order, joining, overlapping independent loads, preflight cycles/concurrent cycles, shared/dependent/explicit owners, duplicate import/load, rollback/retry, concurrent unrelated parent preservation, exact reverse binding removal, disposal error aggregation, unload/reload generation races, stale handle inertness, active definition mutation, invalidation, late host rejection, root isolation, nested rollback diagnostics, failed batch sibling preservation and post-error reload.

## Explicit integration gaps

This proves the ownership engine, **not G08 feature completion**. It does not implement or verify the real IContainer transaction facade, direct/general registration capture, root/child loader wiring, module label DTO/graph/inspect/error propagation, root-disposal integration, live conflict guards, package exports or snapshot restoration. Prepared code must be archived/applied by the core owner, adapted to canonical G06 import routes, and tested against real registrations/caches/lifecycle behavior. G09 snapshot persistence will need a deliberate registry ownership state capture/restore contract; none is fabricated here.
