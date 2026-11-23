# G02 token, completion and promise correctness

Implemented PRIO_ONE 1.1/1.3 while preserving the existing 1.2 React fix. Also repaired the G01 discovery-order defect and the two baseline failures (unshieldable thenables escaping promise APIs, and an instrumentation-sensitive diamond timing assertion).

## Changes and invariants

- String keys have separate lookup/intern/retain/release/public-pin operations. Registration construction holds only its own temporary key refs, including reentrant dependency getters; it never sweeps another operation's zero-owner keys.
- Fast/general registration, declared keys, last dynamic factory key, replacements/rollback, multi presence across roots, unregister and disposal balance ownership. Missing ordinary reads do not intern names. Public raw-key exposure pins exactly the issued keys. Interceptor contexts are deliberate exposures, including missing-token contexts.
- Public graph queries use an exposure callback; internal traversal uses non-exposing paths. Sound name-only inspection does not pin registrations. Historical diagnostics remain available through a bounded log whose keys are retained until eviction/disposal; replacement snapshots retain old history through staging eviction and rollback.
- Disposal temporarily retains actual result keys across child/parent teardown handoff. Public tracking pins only actual returned entries, not cold definitions; inherited scoped records and late joins preserve their issued aliases. Late calls after disposal return an unexposed empty result.
- Completion state is root-owned and weakly keyed, with shared pending work and successful member/wrapper/hook stages retained for retries. Separate roots apply their own completion without stacking generated wrappers.
- Hook containers carry explicit attempt-local lineage through injection, instantiation and resolution. Links expire when each hook settles and retries have distinct attempt identities. Bare external callers join pending work; asynchronous reentry uses the provided hook container. Captured bare-root self-await after suspension remains an unsupported user-promise cycle, not a claimed ambient-context feature.
- Completion and factory views share the existing target-backed forwarder. Accessor getters/setters use the actual target receiver, including setter-only target accessors. No proxy or synthetic binding/record was added.
- Promise entry points reject rather than synchronously throwing when thenable shielding fails. Sync reads retain the actual frozen instance. Diamond validation now checks bounded dependency reads plus dependency-first topological order instead of wall-clock contention.

## Red evidence

- `red-node.log`: first 19 focused regressions, 17 failed / 2 passed before implementation.
- `review-ab-red.log`: nested registration and exact disposal-key probes, 4 failed / 27 passed.
- `review-c-red.log`: explicit async hook contexts, 4 failed / 12 passed before contextual lineage.
- G01 discover fix red was recorded by the characterization lane; `discover-node.log` and `discover-bun.log` show 19/19 green afterward.

## Final verification

- Node 24.14.1, authoritative package `node_modules/vitest/vitest.mjs`: 198/198 non-React cases, 16 files, exit 0 (`all-node.log`).
- Bun 1.3.0, package-local `.cache/bun/bun-darwin-aarch64/bun`: 191/191 main cases + 7/7 legacy + 19/19 React, exit 0 (`all-bun.log`, `legacy-bun.log`, `react-bun.log`). Runtime setup verifies the workers are actually Bun.
- Node/V8 combined coverage: 216 passed / 1 intentionally skipped Bun-only assertion, 17 files, exit 0 (`coverage.log`). Totals: statements 91.82%, branches 85.24%, functions 91.74%, lines 95.48%. All initial per-file floors pass without reduction. The immutable G02 report is copied to `coverage/`.
- New focused G02 files: 55/55 on Bun, exit 0 (`review-final-bun.log`); token cache 33, completion 19, promise boundaries 3.
- Source TypeScript check: exit 0 (`typecheck.log`). Strict explicit compilation of all five new/modified correctness fixture files: exit 0 (`tests-typecheck.log`).
- Browser gate: 4 outputs, exit 0 (`browser.log`). Diff whitespace check: exit 0 (`diff-check.log`).

All final commands use the stable workspace Vitest instance, never the alternate cached runner. Distribution clean-build support remains the later G07 gate; this stage does not claim the Node package distribution is finished.

## Archives

Original container, token, graph and contracts files plus modified static/regression tests were copied without overwriting prior backups under repository `.archive/2026-10-27/packages/foundation/inject/`. No git state, package configuration, dependency or ledger mutations were made by this lane.
