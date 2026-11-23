# PRIO_ONE — superseding delivery plan

## Authority and scope

This plan supersedes every earlier execution plan, stage sequence, per-story validation gate, and delegation arrangement for this task. Earlier evidence remains historical evidence, not permission to resume the old workflow.

Deliver ALL 28 requirements in PRIO_ONE.md. Preserve completed implementation and integrate useful existing drafts; do not redo work merely to follow old milestone ordering. Do not remove requirements or treat prepared code as integrated functionality.

**Execution is solo: no subagents, delegated reviews, interviews, or additional planning cycles.** Writing this plan does not restart the cancelled implementation run.

Preserve unrelated dirty work. Archive replaced material without overwriting previous archives. No git mutations, publishing, or production changes. Use Bun; Node is allowed where needed, with final Bun verification in this workspace as well.

## Mandatory sequence

### 1. Write ALL code and basic tests — execute NONE

Complete one integrated implementation pass over the entire scope. Work in dependency order where necessary, but do not stop at feature milestones to validate or review.

- Finish per-root ambient context and explicit exports/import migration.
- Complete decorate/inject dual distribution and Node support.
- Integrate modules, transactional registration capture and ownership/unload behavior.
- Implement snapshots, restoration, overrides and testing utilities.
- Integrate lazy/provider injection and typed dependency factories.
- Implement graph completeness/drift, qualified tokens and conditional bindings.
- Complete typed module composition and ctsc/split integration.
- Finish benchmark code, documentation, executable examples and record reconciliation.
- Write basic behavioral tests and relevant negative/type fixtures alongside implementation. Reuse existing tests and prepared fixtures rather than inventing another preparation phase.

**Prohibited during this phase:** running tests, typechecks, compilation/transpilation checks, builds, lint/static-check scripts, examples, smoke checks, coverage or benchmarks. Read-only inspection and editing are allowed.

Exit: every requirement below has integrated implementation and basic test coverage written; no known required feature remains a draft, placeholder or unconnected helper. This is not a claim that the code passes.

### 2. TYPECHECK

Only after the entire implementation pass is complete, run the applicable source, public-consumer, declaration and type-fixture checks for inject and the affected sibling packages. Compile type fixtures without running runtime tests. Collect actual diagnostics.

Exit: a concrete list of type errors, or passing typecheck output.

### 3. FIX

Fix type errors across the integrated implementation without deleting required behavior, weakening public contracts, suppressing legitimate diagnostics or disabling tests. Repeat affected typechecks until clean. Do not run runtime tests yet.

Exit: required typechecks pass and known type errors are resolved.

### 4. CRITICALLY REVIEW — COMPARE TO TASKS

Perform the review directly, without subagents. Compare the actual code and written tests against EVERY requirement in the checklist below, not just the previous milestone ledger.

Check missing integration, API compatibility, lifecycle/ownership, concurrency, rollback, cancellation, browser/server separation, distribution paths, documentation accuracy and test assertions. Fix omissions and defects before starting tests. Re-typecheck source changed by these fixes as needed. Do not mark a requirement complete merely because its file exists.

Exit: every row maps to concrete implementation and written tests; identified review defects are fixed. Runtime success remains unclaimed.

### 5. RUN TESTS AND FINAL VALIDATION

Now run the written and existing tests. Execute the Node and Bun legs locally in this workspace; do not substitute one runtime for the other or test a separate private copy.

Then perform final checks: clean decorate-to-inject builds, compiled Node minimum-version/current-runtime smoke tests, compiler/split integration, browser/import-policy/runtime-cycle checks, coverage with preserved floors, runnable examples, TypeDoc and benchmarks. Use existing installed tools; do not create competing runner installations.

Fix actual failures, re-typecheck affected code, review affected requirement rows, rerun affected tests, then rerun the final integrated test matrix. These are failure-driven fixes, not a return to the old per-feature validation workflow.

Exit: all requirements implemented, critical findings fixed and final checks passing. Explicitly report any real validation gap; never relabel incomplete work as complete.

## Complete requirements checklist

| ID | Required delivery |
| --- | --- |
| 0.1 | Coverage tooling, measured baseline and preserved coverage floors |
| 0.2 | Feature/public API test index with accurate final counts |
| 0.3 | Runtime, hot, inspect, interceptor, health and discovery characterization |
| 0.4 | Comparative benchmarks and explicit performance-budget mode |
| 1.1 | String-token retention/reclamation and exposed-key identity |
| 1.2 | Preserve React adapter behavior and regressions |
| 1.3 | Root-owned completion, retry and wrapper correctness |
| 1.4 | Build timeout, cancellation, signals and late-result cleanup |
| 1.5 | Public typing and decorator/type-fixture correctness |
| 2.1 | Cohesive typed extraction and readable Container |
| 2.2 | Explicit real IContainer views with correct context/receivers |
| 2.3 | Root-owned ambient resolution and neutral server import |
| 2.4 | Explicit exports, migrated self-imports and import policy |
| 3.1 | Inject dual build, declarations and conditional distribution |
| 3.2 | Decorate dual build and compatible exported paths |
| 3.3 | Compiled Node test leg and truthful runtime support records |
| 4.1 | Modules, transactional capture, import ownership and unload |
| 4.2 | Snapshots, safe restoration, overrides and testing utilities |
| 4.3 | Scope-bound lazy/provider injection across supported slot forms |
| 4.4 | Typed dependency factories across all ten registration variants |
| 4.5 | Static completeness, observed-edge validation and drift |
| 4.6 | Interned qualified tokens, injection and graph names |
| 4.7 | Ordered conditional bindings, fallback and ambiguity diagnostics |
| 4.8 | Compile-time module capability completeness |
| 4.9 | ctsc parameter metadata pass and split integration |
| 5.1 | Complete guides, migrations and generated TypeDoc reference |
| 5.2 | Runnable examples, example tests and concise accurate README |
| 6.1 | Reconcile PRIO_ONE, FIX-PLAN, DECISIONS and CLAUDE with reality |

## Completion report

Report what changed, the final requirement checklist, actual final command results and any remaining risks. No fabricated counts, unsupported success claims or partial completion presented as the requested complete delivery.
