# G07 Node smoke preparation — 2026-10-27

Status: preparation only; **G07 and PRIO_ONE 3.1–3.3 are not complete**.

## Scope

Read PRIO_ONE 3.1–3.3, the G07 PRD row and distribution test-spec acceptance criteria, current core/decorator/server APIs, existing static/legacy/build-cancellation characterization, and sibling decorate package/dist entries.

Added ten direct `node:test` cases, using public package exports only:

- `tests/node/core.test.mjs`: five cases for synchronous singleton/scoped/transient ownership, asynchronous lifetime identity/construction sharing, parent-first multi ordering and caching, child-first disposal/external ownership, and signal-based cancellation of an in-progress build.
- `tests/node/static.test.mjs`: two cases for decorator-free `$injectable`/`$inject`/`$members`, initialization ordering, explicit dependency validation, and ordinary JavaScript calls to legacy decorator functions (constructor/member/optional/multi).
- `tests/node/server.test.mjs`: three cases for tenant/request lifecycle and shared tenant configuration, overlapping/nested/rejected ALS contexts, and a captured-container cycle after `await` under `DiRuntime`.

Each file checks that public exports resolve to `dist/*.js` before dynamically importing them. Static mode also requires compiled decorate. Tests contain no TypeScript syntax, loader, transpiler, source aliases, or fallback. Cancellation and cycle tests have runner watchdogs; cancellation requires `DI_BUILD_CANCELLED`, and cycle requires `CircularDependencyError`, not timeout success. Root/runtime resources have test cleanup hooks.

No existing file was replaced, so no archive revision was needed. No core, package, configuration, sibling, lockfile, or git state was changed by this lane.

## Fresh evidence

- `versions.log`: exact minimum Node **v22.0.0**, compatible runner Node **v24.14.1**, available Bun **1.3.0**.
- `syntax.log`: all three `.mjs` files pass `node --check` on both Node versions (six checks).
- `node22-core-prerequisite.log`: focused `node --test tests/node/core.test.mjs`, exit **1** before cases execute. Guard reports `@coldmind/inject` resolving `src/core/cminject.core.ts`, not compiled JS.
- `node24-core-prerequisite.log`: same focused command, same prerequisite failure, exit **1**. This prevents Node 24 type stripping from accidentally being counted as distribution support.
- `git diff --check -- tests/node`: exit **0**; new-file contents separately checked for trailing whitespace and terminal newlines with Python (see `whitespace.log`).

No test behavior is claimed passing. No build, aggregate suite, benchmark, full typecheck/lint, runtime support claim, or documentation claim was attempted in this preparation lane. Source/runtime APIs are existing APIs, but actual compiled behavior still needs execution.

## Prerequisites and owner/integration commands

1. Complete the G05/G06 source/internal-export work; clean-build decorate, then inject, with real `.js` and `.d.ts` for every exported path. Inject currently has no dist directory and exports point to source. Sibling decorate already has compiled entries, but that is not proof of inject distribution.
2. Make public default conditions resolve emitted JS without any `--conditions=bun`, `--conditions=source`, loader, or transpiler. Keep Node minimum `>=22`.
3. Run the prepared smoke on both runtimes from the inject package root:

```sh
.cache/node22-min/node-v22.0.0-darwin-arm64/bin/node --test tests/node/*.test.mjs
/Users/patrik/.nvm/versions/node/v24.14.1/bin/node --test tests/node/*.test.mjs
.cache/node22-min/node-v22.0.0-darwin-arm64/bin/node -e "import('@coldmind/inject').then(m => { if (!m.Container) process.exit(1) })"
```

4. Separately execute existing Node Vitest suites on the compatible runner; these direct tests do not replace that gate. Refresh Bun source/browser/build/type-consumer/tarball gates as required by G07. Only after compiled smoke passes should README/CLAUDE claim Node support.

Stop condition reached: meaningful distribution-only smoke files and prerequisite evidence prepared, without claiming the blocked G07 acceptance gates passed.
