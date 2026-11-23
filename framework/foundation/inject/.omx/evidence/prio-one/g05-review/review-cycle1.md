# G05 extraction — independent code review

Native role: code-reviewer (`g05_code_review`). Scope: PRIO_ONE 2.1/2.2 extraction only, not final all-plan approval. Compared the pre-extraction archived Container at `framework/.archive/2026-10-27/revisions/g05-step1/packages/foundation/inject/src/core/cminject.container.ts` with the current Container and eight cohesive helper/capability modules. Also inspected the source-DAG checker and the incremental internal package exports.

## Verdict

**APPROVE — zero remaining confirmed findings.** The initial missing explicit internal-subpath finding was repaired during this review; all eight routes now resolve under the actual in-place Bun binary. Full import migration/wildcard removal and per-root ambient ownership remain intentionally scheduled G06 work, not falsely claimed done here.

## Reviewed contracts

- Container remains 861 normally formatted lines. Async/binding/construction/resolution/mutation/observation implementations are cohesive, not arbitrary line-count shuffling. Lifecycle orchestration remains Container-owned.
- Stable typed host facets preserve exact binding, record, pending-map and scope-facet identities. Container alone translates opaque owner/scope capabilities through WeakMaps. No helper runtime-imports Container or the barrel; host is type-only and view has type-only imports.
- Explicit `ContainerView implements IContainer, Scope` forwarding uses the actual target receiver, including setters and existing runtime extras. The sole one-way prototype attachment preserves `instanceof Container`; contextual resolution/completion expire independently from stable build signals.
- String-token retain/release ownership, dynamic-key caching, historical issue-key snapshots, staged candidate identities, terminal cancellation and rollback cleanup remain single-owned. Replacement engine reuses normal resolution/lifecycle authority.
- Root completion uses the exact root weak progress-map identity, retains successful retry stages, and preserves original method provenance. Manual construction uses slot-only facades; unknown internal scopes fail explicitly.
- Sync-or-Promise behavior remains explicit; no Promise-only fallback was introduced. Error paths preserve primary diagnostics and attach cleanup causes. No newly introduced broad failure-swallowing workaround, Proxy, generated forwarder, private bracket escape, source `any` type or double assertion found.

## Fresh independent validation

All commands ran in the actual inject workspace using the sole `node_modules/vitest/vitest.mjs`, Node 24.14.1 and `.cache/bun/bun-darwin-aarch64/bun` 1.3.0:

| Check | Result | Evidence |
| --- | --- | --- |
| Source TypeScript 5.9.3 | exit 0 | source-typecheck.log |
| Node core/standard/static/server/legacy discovery | 241/241, exit 0 | runtime-node.log |
| Bun same suite, runtime assertion enabled | 241/241, exit 0 | runtime-bun.log |
| Compiler-backed type fixtures | 10/10, no type errors, exit 0 | types-node.log |
| Bun React, `INJECT_EXPECT_BUN=1` | 19/19, exit 0 | react-bun.log |
| Source runtime DAG | 17 modules / 52 edges, acyclic, exit 0 | import-dag.log |
| Browser gate | exit 0 | browser.log |
| Eight explicit internal source routes | all resolved, exit 0 | internal-exports.log |
| Scoped whitespace/diff check | exit 0 | diff-check.log |

The performer owns the final measured responsibility-level coverage floor mapping and immutable coverage evidence. This review does not waive that G05 checkpoint gate or later phase/full-release gates. No product code, tests, package configuration, git state or goal metadata was changed by the reviewer; only this review and validation logs were written.
