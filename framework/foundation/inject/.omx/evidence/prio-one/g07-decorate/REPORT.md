# G07 decorate / PRIO_ONE 3.2 verification

Completed 2026-10-27. No inject, compiler, consumer, lockfile, goal, state, or git mutations.

## Implementation

- `../decorate/package.json`: ordered `types` → `bun` → `default` condition maps, compiled main/types, shipped dist and source, built metadata sideEffects, build and compiled-node test scripts.
- Root runtime API preserved: classChain, createRecords, metadataKey, placementPhrase, siteOf. All original type exports preserved.
- Inventoried workspace deep imports: `@coldmind/decorate/src/decorate.{metadata,records,site}.ts`. Added explicit compatibility for these and original core source entry. Canonical implementation routes: `./internal/{core,metadata,records,site}`. Removed arbitrary wildcard; `./package.json` explicit.
- Source self-imports use canonical export routes, no relative source paths. No paths/customConditions workaround. Build is standalone NodeNext + rootDir/src + outDir/dist with declarations.
- Prebuilt isolated bootstrap and installed outputs before atomic package export switch, then switched source imports. Active consumers retained working old routes throughout.
- Existing TS5.9 typing defect fixed: `StandardContext.access` function properties rejected native decorator contexts under strict variance. Method signatures now match native TypeScript context shape; existing standard suite is regression/type coverage. No runtime behavior changed.
- Vitest explicitly selects standard/legacy sources (not standalone Node tests); runtime setup asserts requested worker runtime. Legacy script now runs in its intended cwd/config.
- Replaced files archived without overwriting under repository `.archive/2026-10-27/packages/foundation/decorate/`.

## Executed checks

All paths below relative to inject unless noted. Node24: `/Users/patrik/.nvm/versions/node/v24.14.1/bin/node`; Bun: inject `.cache/bun/bun-darwin-aarch64/bun`. TypeScript 5.9.3, Vitest 5.0.5, Bun 1.3.0.

- Baseline Node Vitest standard+legacy: exit 0, 17 tests (`baseline-node.log`).
- From decorate: Node24 `../inject/node_modules/typescript/bin/tsc -p tsconfig.build.json`: exit 0 (`build.log`).
- From decorate: Node24 `../inject/node_modules/typescript/bin/tsc -p tsconfig.check.json`: exit 0 (`typecheck.log`).
- Clean final bootstrap: copied only final package.json, tsconfig.build.json and src into decorate `.cache/g07-clean-final`; asserted dist absent, ran TS5.9 build, then direct Node24 self-import. Exit 0 (`clean-final.log`). Main dist intentionally remained available to concurrent inject tests.
- From decorate: `DECORATE_EXPECT_RUNTIME=node|bun <explicit runtime> ../inject/node_modules/vitest/vitest.mjs run --config vitest.config.ts tests/standard`: both exit 0, 10 tests each (`node-standard.log`, `bun-standard.log`).
- From decorate/tests/legacy: `DECORATE_EXPECT_RUNTIME=node|bun <explicit runtime> ../../../inject/node_modules/vitest/vitest.mjs run --root ../.. --config vitest.config.ts tests/legacy`: both exit 0, 7 tests each (`node-legacy.log`, `bun-legacy.log`).
- From decorate: Node24 `--test tests/node/*.test.mjs`: exit 0, 3 tests. Compiled root API/standard+legacy record semantics, all explicit aliases/modules, declaration/JS targets and export order (`node-distribution.log`).
- Same direct compiled tests on Node22.22.1: exit 0, 3 tests (`node22-distribution.log`). Exact Node22.0 minimum was not exercised in this lane.
- Direct Bun import asserts Bun process, source resolution, public API and identity of all four compatibility aliases: exit 0 (`bun-import.log`).
- Package scripts `bun run build`, `bun run typecheck`, `bun run test` with Node24 + workspace bin PATH: exit 0, 17 record tests (`package-scripts.log`). The generic test script uses Node shebang; separately explicit Bun worker runs above prove Bun, not script naming.
- Bun browser bundle of all four source entries: exit 0, 7 modules (`browser.log`).
- Dependency-free source scan: explicit routes exist, no relative source imports, no `/src/` self-imports, no source import cycles: pass (`import-policy.log`).
- `git diff --check -- ../decorate`: exit 0 (`diff-check.log`). No package lint script exists; no lint claim.

All lane acceptance checks pass. No installed API interop blocker remains. The injected library's own distribution integration and exact minimum runtime validation belong to leader G07 integration.
