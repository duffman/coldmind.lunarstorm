# Deferred dependency leaf preparation

Scope: new pure leaf and isolated prepared tests only. This is **not G10 integration or full 4.3/4.4 completion**. No container, contracts, token, graph, decorators, barrel, package, or active-suite changes.

## Verified commands (from inject workspace)

- `CMINJECT_EXPECT_RUNTIME=bun .cache/bun/bun-darwin-aarch64/bun node_modules/vitest/vitest.mjs run --config .omx/evidence/prio-one/lazy-prep/vitest.config.ts`: Bun **1.3.0**, 12/12 passed (`bun13.log`). Existing runtime setup rejects the wrong worker runtime.
- `CMINJECT_EXPECT_RUNTIME=node /Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/vitest/vitest.mjs run --config .omx/evidence/prio-one/lazy-prep/vitest.config.ts`: Node **24.14.1**, 12/12 passed (`node24.log`). Only workspace Vitest 5.0.5 used.
- `/Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/typescript/bin/tsc -p .omx/evidence/prio-one/lazy-prep/tsconfig.json`: TypeScript **5.9.3**, exit 0 (`typecheck.log`). Isolated compiler fixture; excludes intentionally red G04 fixtures.
- Same compiler/config with `--noEmit false --declaration --emitDeclarationOnly --rootDir . --outDir .omx/evidence/prio-one/lazy-prep/declarations`: exit 0 (`declarations.log`), private unique-symbol brand emitted correctly.
- In-place Bun 1.3.0 `Bun.build` with leaf entry, browser target, output under `browser/`: passed; output checked for `node:`, `async_hooks`, and `AsyncLocalStorage` (`browser.log`).
- In-place `.cache/node22-min/node-v22.0.0-darwin-arm64/bin/node --input-type=module`: generated browser bundle imports and lazy/provider/peek/sync resolution smoke passed (`node22-smoke.log`).

No lint script/config is supplied by this package; strict isolated compiler and browser build provide static/syntax checks. This lane did not run or claim a full package build or active test-suite pass.

## G10 integration handoff

- Public factories/types: `lazy(token)`, `provider(token)`, `Lazy<T>`, `Provider<T>`, `LazyToken<T>`, `ProviderToken<T>`.
- Future dependency-slot contracts: `DependencySlot<T>`, `DependencyValue<Slot>`, `DependencyValues<Tuple>`.
- Recognition: `isDeferredToken`, `isLazyToken`, `isProviderToken`; immutable, private-symbol branded and WeakSet-issued. Copying marker properties does not forge a valid marker.
- Graph preparation: `unwrapDependencySlot` and `describeDependencySlot` yield the ordinary token and `eager | lazy | provider` classification. G10 must retain availability/captive checking for deferred edges and decide eager-cycle traversal explicitly.
- Value materialization: `bindDeferredSlot(scope, marker)` returns the appropriate lazy handle/provider. Pass the actual public resolving scope/view (which may itself be root when legitimately resolving there); never substitute global root or copy resolver private fields. Calls invoke that same receiver's public methods, preserving its active-frame and completed-view fresh-resolution behavior.
- `get`/provider call ordinary `resolve`; `getSync` calls ordinary `resolveSync`; `peek` calls ordinary `tryGet`. No additional service cache. Disposed scope and resolution failures propagate.
- Normal `InjectionToken` remains unchanged. Markers are not ordinary `.resolve()` arguments. No current constructor/member/decorator/factory/graph path accepts these declarations until G10 is integrated.

## Process note

Before the leader supplied exact already-present runtime paths, isolated `npm exec` retrieved Bun 1.3.0/Node 22.0.0 runtimes and a local Homebrew Node 24.5.0 was used for preliminary checks. No second Vitest/compiler was installed or used. Those preliminary runs are not the final proof above; all final evidence was rerun using the provided in-place binaries. No fetch processes remain.
