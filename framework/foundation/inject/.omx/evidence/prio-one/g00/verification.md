# G00 verification receipt

Initial measurements are immutable under `g00/coverage/`; subsequent coverage goes to `current-coverage/`.

- Functional Bun 1.3.0 baseline scripts: core/standard/static 88 passed / 90 registered, known frozen-thenable and diamond timing failures; legacy 7/7; React 19/19 including explicit Bun worker assertion.
- Node 24.14.1 V8 combined baseline: 116 registered, 113 passed, 2 failed, 1 Bun-only assertion skipped. Both failures are pre-existing and disclosed in FEATURES. Coverage still completed (`reportOnFailure:true`).
- Source TypeScript5.9.3 check: `node node_modules/typescript/bin/tsc --noEmit -p tsconfig.json`, exit0.
- Exact per-file thresholds populated from coverage-summary.json, not hand-chosen floors.
- API index: TypeScript parser extracts IContainer member/overload families and test-call bodies, joins parameter-expanded runtime JSON cases, exposes lexical evidence and explicit gaps. Regenerate helper retained as build-index.mjs.
- Root monorepo lock unchanged by this lane. Package-local npm install required --legacy-peer-deps after default npm peer resolution failed in arborist; cached dependencies are linked into package node_modules. The exact workspace Vitest runner is reused; a provider-only workspace node_modules symlink makes its V8 provider discoverable without lockfile edits. Stale generated test transforms referencing the temporary runner were selectively cleared; stable direct-runner commands are now used instead of bun x. No source behavior edits.
- Archived original package.json/vitest.config.ts/vitest.react.config.ts to framework/.archive/2026-10-27/packages/foundation/inject/ without overwriting any prior backup.

## Measured total

```json
{
  "lines": {
    "total": 1429,
    "covered": 1304,
    "skipped": 0,
    "pct": 91.25
  },
  "statements": {
    "total": 2030,
    "covered": 1738,
    "skipped": 0,
    "pct": 85.61
  },
  "functions": {
    "total": 503,
    "covered": 414,
    "skipped": 0,
    "pct": 82.3
  },
  "branches": {
    "total": 1193,
    "covered": 917,
    "skipped": 0,
    "pct": 76.86
  },
  "branchesTrue": {
    "total": 0,
    "covered": 0,
    "skipped": 0,
    "pct": 100
  }
}
```

## Stable toolchain follow-up

A temporary duplicate Vitest installation caused stale transformed imports in the shared Vitest cache. The workspace Vitest 5.0.5 installation is now the single authoritative runner; source test imports and package script entrypoints resolve it. Only generated cache entries containing the obsolete temporary-runner path were cleared. No source was changed for this repair.

- Post-repair Bun legacy script: 7/7, exit0 (`bun-legacy-verified.log`).
- Post-repair Bun React script: 19/19, exit0 (`bun-react-verified.log`).
- New G01 runtime/hot/inspect files: 27/27 on Bun and 27/27 on Node24, both exit0 (`g01-bun-runner.log`, `g01-node-runner.log`).
- Original G00 coverage remains immutable; later verification with added G01 files wrote current-coverage and retained disclosed baseline failures.
