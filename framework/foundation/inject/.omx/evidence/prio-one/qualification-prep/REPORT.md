# PRIO_ONE 4.6 qualification preparation (not integrated)

## Exact proposal

`qualification.patch` adds only `qualify<T>(token: InjectionToken<T>, name: string): Token<T>` and its intern map to the current token module. `cminject.token.original.ts` captures the read baseline; `cminject.token.proposed.ts` is the isolated patched copy. No live source, package, config, or documentation edits were made in this preparation lane.

Canonical map keys are raw string names or the existing symbol/class canonical keys. Existing normalized string symbols are mapped back through `stringKeys`, ensuring qualification agrees with the original string. This avoids calling public normalizeToken (which pins) or even interning absent string bases. Current `tokenName` needs no changes: derived symbols carry `Db#replica` descriptions. Existing public star export picks up the function after integration.

## Tradeoffs / boundaries

- Public derived symbols are permanently interned. The map therefore retains qualification names and base string names/symbols, but does NOT retain class constructor objects or add/pin string-key registry entries. Public arbitrary-retention identity implies retained metadata; future bounded qualification reclamation would need a separately approved contract.
- A delayed class getter executes when `qualify` is called, once per call, and the result uses the direct class identity. A plain returned symbol cannot defer evaluating the base until container resolution. This must be documented; do not claim preserved import-cycle laziness for `qualify(delay(...), name)`.
- Normalized keys publicly issued by normalizeToken stay pinned under the existing policy. Internal unpinned string-key aliases must not be reused after release/collection; re-interning the raw string still recovers the same derived token.
- Distinct symbol/class identities with identical descriptions remain distinct qualified tokens, though displayed names may coincide (as existing tokens already can). Delimiter-containing base/qualifier names can also display equally, but nested map keys prevent identity collisions.
- Lazy/provider declarations are not InjectionToken; qualify the underlying token first, then wrap it. The compiler fixture verifies wrapper inference and rejects wrapping a marker directly with qualify. Runtime deferred integration belongs to G10/G11 and is not claimed here.
- Runtime decorated field/constructor tests invoke the actual legacy decorator signatures directly, avoiding any live tsconfig mutation. The compiler fixture includes actual standard field decorator syntax.

## Tests and evidence

Prepared runtime suite: `.omx/prepared-tests/qualification.runtime.test.ts`: 14 tests, 49 assertions passed under the in-workspace Bun 1.3.0 binary. It imports proposed token code only from the isolated .omx copy; container/decorator/graph integration uses existing live implementations and derived symbols. It is outside the configured normal discovery glob.

Prepared compiler fixture: `.omx/prepared-tests/qualification.types.ts`: strict tsc passed using Node 24.14.1 and installed cached TypeScript. Covers class/abstract/typed/string/symbol/delayed inference, explicit generic, resolve/resolveSync/resolveAll/binding inference, Lazy/Provider wrapped inference, field decorators, wrong resolved type, symbol qualifier rejection, marker rejection. The isolated tsconfig also checks the runtime suite and full dependency closure.

Commands:

```
.cache/bun/bun-darwin-aarch64/bun test ./.omx/prepared-tests/qualification.runtime.test.ts
/Users/patrik/.nvm/versions/node/v24.14.1/bin/node .cache/prio-one-tools/node_modules/typescript/lib/tsc.js -p .omx/evidence/prio-one/qualification-prep/tsconfig.json --pretty false
git apply --check .omx/evidence/prio-one/qualification-prep/qualification.patch
```

All three passed. Logs: `runtime.log`, `typecheck.log` (empty on success).

## Required integration work later

Review/apply the patch against the then-current G11 token implementation. Promote runtime tests into the appropriate live suites, replacing isolated imports with live exports and adapting bun:test to the repository's Vitest convention. Promote inference checks to existing tests/types. Re-run the complete token-cache suite, full typecheck/lint/build, and Bun/Node suites on integrated code. This preparation does not claim a live feature, full regression pass, or final build proof.
