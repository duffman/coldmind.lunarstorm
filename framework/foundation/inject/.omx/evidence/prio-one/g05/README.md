# G05 — verified staged core extraction

Status: COMPLETE, independently approved by code reviewer and architect (zero remaining findings), parent checkpointed before G06. Source `cminject.container.ts`: **861 ordinary source lines**, down from 2740, without minification or feature deletion. The full eight-module architecture was completed, not stopped at async extraction.

## Cohesive owners

| Module | Responsibility moved from original container |
| --- | --- |
| async | Sync-or-native-Promise chaining, sequential work, value shielding, settlement, error composition, shared clock, abort-signal composition. Export is `chain`, avoiding dynamic-import namespace `then` assimilation. |
| binding | Typed allocation-free handles/definitions, temporary key leases, dependency/dynamic-key retention, multi presence counts, pure definition preflight, stored instance identity/control fields. |
| view | Explicit compiler-checked factory/completion facades, all generic overloads/setters and public extras, contextual resolution/construction and stable signals. All imports are type-only; no runtime back-import. |
| construction | Constructor/slot/member completion, weak root progress, attempt/lineage expiry, method provenance and hook sequencing. |
| resolution | Cache/pending/build ownership, routing/control, timeout/cancellation/late cleanup orchestration, interception and ambient frame tracking. Core cache fields alias the exact maps, preserving direct warm get. |
| mutation | One shared registration/install/preflight/commit/replacement/rollback/unregister engine, including terminal rollback cleanup of detached originals. |
| observe | Runtime edges, bounded historical-key ownership, declared graph indexing and read-only DTO/validation projections. |
| host | Type-only narrow capability facets; no runtime initialization or imports. |

Container retains hierarchy, policy, public orchestration, direct cached reads, lifecycle/health and the sole one-way instanceof prototype attachment. Helpers never import Container at runtime or receive its entire mutable implementation as a nominal host. Binding/record/manual-scope ownership uses stable narrow facets and core-private weak routing tables. Capabilities are bound once per scope, not allocated per resolution. No Proxy, generated method forwarding, private bracket escapes, double assertions or own any syntax.

## Incremental archives and gates

Every responsibility's prior source version is retained under framework `.archive/2026-10-27/revisions/g05-step1` through `g05-step9/packages/foundation/inject/`. Per-stage source snapshots and logs were not overwritten as a later stage.

- Steps 1–8: each full Node and Bun runtime run **241/241**, actual compiler **10/10** both, source tsc both, browser import gate and runtime DAG gate pass. Logs are `stepN-*.log/.exit`.
- React accounting: early stage harness ran all 18 functional cases on Bun but omitted the optional Bun-only guard flag, giving18+1skip; this was transparently corrected. Stage6 onward and final step8 use INJECT_EXPECT_BUN=1: **Bun19/19**; Node18+1 intentionally Bun-only skip.
- Final V8: **259 passed / 1 skipped**, all unchanged original per-file floors pass (`coverage-final.log`, exit0; immutable output `coverage/`).
- Final emitted/source audit: **17 modules / 52 runtime edges**, acyclic; host emits no runtime code, view has no runtime imports. The independent architect also audited emitted imports rather than trusting source labels alone.
- Static checker was negatively exercised: a runtime cycle fails; the same back-edge expressed as type-only passes; host runtime import fails (`checker-{cycle,erased,host}.log`).

## Coverage responsibility mapping

Every one of the seven runtime extraction modules inherits the **unchanged original Container floor**: statements85.45%, branches77.82%, functions80%, lines90.06%. Container and every other existing file keep their original floors, with no lowering. Host is statically type-only with100% empty-unit contract. Prepared, unexported module/lazy leafs remain separate later work.

| Runtime owner | Measured statements / branches / functions / lines |
| --- | --- |
| async | 92 / 81.48 / 93.10 / 98.41 |
| binding | 93.33 / 95.68 / 83.01 / 91.07 |
| construction | 97.97 / 92.20 / 100 / 100 |
| resolution | 91.61 / 89.49 / 97.05 / 97.26 |
| mutation | 87.27 / 79.71 / 93.33 / 91.10 |
| observe | 95.55 / 86.73 / 91.89 / 100 |
| view | 98.43 / 98.27 / 97.59 / 98.13 |
| remaining container | 94.52 / 86.64 / 96.38 / 97.34 |

## Explicit internal entries

Added all eight planned `./internal/{async,binding,view,observe,construction,resolution,mutation,host}` export routes to their current source targets. Root/public entries and the existing wildcard were deliberately preserved for the later G06 migration. Node24 transform-types and Bun each resolve/import all eight once, with namespace identity equal to their existing source route; host has0 runtime exports (`internal-resolution-{node,bun}.log`, exit0).

G06 handoff: replace global ambient installation with root-owned attachment; migrate all inject source-path self imports to explicit exports and only then remove wildcard. The source DAG checker must resolve package export targets after that migration, not silently drop explicit aliases; this limitation is already assigned in G06. No G06 product edits were made before G05 independent approval/checkpoint.
