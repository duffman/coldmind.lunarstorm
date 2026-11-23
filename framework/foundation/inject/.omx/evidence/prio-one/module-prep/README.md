# Module definition and composition preparation

## Implemented scope

`src/core/cminject.module.ts` provides identity-preserving `defineModule`, curried `defineModule<Provides, Requires>()`, named-capability module/handle types, and `composeModules`. It does not execute registration or implement container loading, unloading, transactions, or registry ownership. Root exports and active-suite registration remain unchanged.

Definitions and their import arrays retain identity and are not deep-frozen, so a mutable import array can construct an actual cycle for the future loader to diagnose. Composition creates a registration-free importing definition, preserving import order and each module identity.

Capability maps are explicit author declarations, not inferred from the register body or nominal token identities. Type composition includes statically inferred transitive import tuples, verifies every named requirement against all offered provider types, and returns the combined provided map. Duplicate providers contribute a union: an incompatible provider cannot disappear through a `never` intersection. An explicitly `never` provider cannot satisfy a requirement.

## Verified evidence

- Actual Node 24 and Bun 1.3 worker assertions through `tests/runtime.setup.ts`.
- Pure runtime fixtures: **9/9 on Node and 9/9 on Bun**, `runtime-{node,bun}.log` and `.exit`.
- Compiler-backed Vitest fixture: **6/6 passed**, no type/source errors, `type-tests.log` and `.exit`; TypeScript 5.9.3.
- Direct strict TypeScript fixture checks: `types-final.log`, `runtime-typecheck.log`; exit 0.
- An unsuppressed missing-Db probe exits 2 and reports `ModuleCompositionError`/`missingCapabilities` with the required Db declaration: `missing-probe.log`. The fixture also asserts the computed missing map is exactly `{ Db: Db }`.
- Both node and browser leaf bundles build with no Node/ALS imports; emitted Node leaf runs on **Node 22.0.0**: `build.log`.
- Source contains no `any` types, double assertions, relative imports or Proxy; whitespace check passes.

Commands from the package root:

```sh
CMINJECT_EXPECT_RUNTIME=node node node_modules/vitest/vitest.mjs run --config .omx/evidence/prio-one/module-prep/vitest.config.ts --typecheck.enabled=false
CMINJECT_EXPECT_RUNTIME=bun .cache/bun/bun-darwin-aarch64/bun node_modules/vitest/vitest.mjs run --config .omx/evidence/prio-one/module-prep/vitest.config.ts --typecheck.enabled=false
node node_modules/vitest/vitest.mjs run --config .omx/evidence/prio-one/module-prep/vitest.config.ts --typecheck.only
node node_modules/typescript/bin/tsc --noEmit -p .omx/evidence/prio-one/module-prep/tsconfig.json
```

Use compatible Node 24 on PATH for the Vitest toolchain. The checked-in preparation fixtures remain under `.omx/prepared-tests/`, outside the active core suites.

## Explicit typing boundaries

- Static completeness requires a finite inferred module tuple and statically known transitive import tuples. Dynamic or erased imports are rejected instead of being falsely certified; ordinary future `container.load` remains the runtime route for such graphs.
- Each key declared in `Requires` is required, including a key whose value type includes undefined. Author-declared capability maps are trusted; register bodies are not verified.
- Under TypeScript 5.9, a direct nested generic call such as `composeModules(defineModule({...}))` can widen the module rest inference and be rejected conservatively. A named definition constant works; it is also the intended reusable module-identity pattern. The bounded `NoInfer` attempt did not repair this corner and was not retained as a broad fallback. `nested-inference.log` preserves the known limitation. Refine during G12 if needed without weakening missing-provider checks.
- The compiled leaf smoke is not the full package distribution gate. No actual module loader, unload behavior, runtime module graph, or container integration has been verified here.

## Integration handoff

G08 owns root module loading/transactions and may extend the module handle implementation. G06 must migrate the temporary package `/src/` import to the canonical contracts/internal-module export routes. G12 should move/adapt the isolated compiler fixtures after runtime integration, preserve the missing/incompatible checks, and decide whether to refine the inline generic inference corner. Active examples/barrel exports and feature/coverage index updates remain leader-owned integration tasks.

Experimental type-inference revisions were preserved at repository `.archive/2026-10-27/revisions/module-prep/types-experiments/packages/foundation/inject/src/core/cminject.module.ts`; no existing source or user work was overwritten.
