# PRIO_ONE complete-delivery execution plan

## Outcome, authority and completion unit

Deliver **every item 0.1–6.1 in `PRIO_ONE.md`**, preserving the working library and completing packaging, features, tests, examples and documentation. This is an execution-ready Ralplan draft for sequential Architect then Critic review, not implementation evidence or a claim of completion.

Requirements handoff: `.omx/specs/deep-interview-prio-one.md`; grounded context: `.omx/context/prio-one-20261027.md`; detailed acceptance inventory: `PRIO_ONE.md`; verification matrix: `.omx/plans/test-spec-prio-one.md`; official version-aware packaging/coverage/typecheck evidence: `.omx/plans/prio-one-evidence.md`.

The latest user directive is `$oh-my-codex:ralplan FINISH THE COMPLETE PRIO_ONE.md`, within the active Autopilot request. The leader's handoff includes plan-listed files/dependencies, sibling `decorate`/`ctsc` and necessary `split` integration, and automated local verification. Do not ask again about routine steps or stop after one feature. No git state changes, publishing, production actions, unrelated consumer modifications or feature removals. Preserve the existing dirty work. Only planning artifacts may change until sequential reviews and the bound execution handoff exist.

**Terminal success:** every requirement row below has implemented artifacts and passing fresh evidence; all aggregate gates pass; records match reality; independent final architecture/code review is clean. A real external blocker is recorded with evidence and exact remaining requirements; it is not silently relabeled complete.

## Grounded evidence and current baseline

- `package.json` exports source TypeScript plus `./*`; `tsconfig.build.json` already uses NodeNext but dist is unpublished. Self-imports occur in source, tests and server entries.
- `src/core/cminject.core.ts` re-exports all token helpers, including **public `normalizeToken`**. Reclaiming symbols already returned to callers would break identity.
- `cminject.container.ts` has both `install` and a direct fast registration path. Declared edges and `Binding.keyOf` retain keys. Replacement, failed registration and unregister paths must participate in retention.
- `ResolutionView` inherits `Container.prototype` through a forwarding layer and already passes `instanceof`. Its private-method bracket access must become a typed host contract, not migrate unchanged.
- Completion is a module-global WeakSet marked before members/hooks finish. Cached builds and transient builds take different paths. `share()` does not cover the first caller or transients; disposal waits on pending work.
- Existing root/server imports install global resolution ALS as a side effect. `DiRuntime` needs root-owned attachment without importing Node facilities into browser core.
- Phase 1.2 is implemented in `src/cminject.react.ts`, `tests/react/adapter.test.ts`, README. Prior evidence: package-local Bun 1.3.0, 19 adapter cases; Node, 18 passed and Bun-only assertion skipped. Rerun after shared changes.
- `../../../vitest.shared.ts` chooses runtime from the invoking process, not the script name. Current `test:bun` omits server cases. V8 coverage requires a **Node coverage leg**, separate from Bun functional tests. The installed Vitest 5 runner needs a compatible Node version (>=22.12); the published library retains Node >=22 and receives direct smoke coverage independently.
- `../decorate` has source-only exports, internal `/src/` self-imports and no build config. It is a real Node prerequisite.
- `../../tools/ctsc/src/pass.ts` exposes analyze/visit/checker/original-node APIs; its package pins TypeScript **7.1.0-dev.20261024.1**, not inject's 5.9. Its `rootDir` is `src`, so a top-level `passes/` file is not emitted.
- `../../tools/split/src/split.build.ts` uses `Bun.build` and has no ctsc hook. Integration is real work, not just adding the pass filename.
- No package lint script exists. Verification must distinguish typechecking/import-policy/static cycle checks from lint rather than fabricate a lint result.

## RALPLAN-DR deliberation

### Principles
1. Preserve behavior first: lock ownership, sync resolution, cycles, decorator dialects and public surface before structural work.
2. One source of identity and ownership: tokens, builds, module transactions and snapshots cannot each invent independent lifetimes.
3. Explicit context and bounded transactions: browser core has no ambient async state; async work carries its own context.
4. Small verified increments: fixes before moves; single writer for container/contracts; independently owned test/tooling/sibling lanes.
5. Completion is evidence: actual baseline data, executable examples, compiled Node smoke, runtime-asserted Bun tests and final review.

### Decision drivers
- Compatibility/correctness of the existing feature-rich container, including failure and late-completion paths.
- Delivery throughput without shared-file conflicts or a rewrite that obscures regressions.
- Real package portability and all-plan acceptance, not nominal APIs or documentation-only checkmarks.

### Options
| Option | Advantages | Costs/risks | Decision |
|---|---|---|---|
| A. Characterize, repair in place, extract typed boundaries, then add feature groups with serialized core ownership | Local regression isolation; preserves optimized sync path; fits existing plan; parallel tests/siblings/tools | More intermediate verification; some temporary adapters | **Selected** |
| B. Replace the container wholesale with a new graph/transaction engine, then port APIs | Can simplify architecture globally and optimize new features together | High hidden compatibility risk, difficult bisecting without git changes, too much simultaneous behavior/layout change | Rejected for this run; unnecessary to satisfy requirements |
| C. Deliver Node packaging and docs first, defer core refactor/features | Fast externally visible progress | Violates complete-deliverable directive and documents APIs before behavior is stable | Not a completion strategy; sibling packaging preparation may run independently within A |

### Deliberate pre-mortem
1. **Late async results resurrect disposed/replaced instances.** Mitigate with per-build terminal state, cancellation before waits, one disposal claim per record, identity-guarded publication and deterministic races tested with deferred promises. Evidence includes initial waiter, joined waiter, transient, onInit, replacement and scope disposal.
2. **Refactoring/export changes pass source tests but Node or browser bundles fail.** Mitigate by explicit import inventory, type-only host dependencies, conditional-export ordering (`types`, `bun`, `default`), clean dependency-first dist builds, direct Node imports and browser bundles for every browser-safe entry.
3. **New transactions remove someone else's bindings or revive disposed resources.** Mitigate explicit module/snapshot/override ownership and generation checks, serialized conflicting mutations, rollback identity checks, shared import references and LIFO overrides. Inject interleaved registrations, concurrent loads and disposal failures in tests.

## Architecture decisions to implement and review

### A. Token lifetime and public identity

Use separate internal **lookup / retain / release** operations rather than routing every call through public interning. Missing lookups (`resolve`, get/has/peek/binding queries) never create string entries. Every installed/staged binding owns a deduplicated retention set for its own token, declared deps and cached dynamic keys. Transfer or release that set on replacement, rollback, unregister and tree disposal, including the fast registration path.

Because public `normalizeToken` and `getGlobalToken` return freely retained symbols, both pin externally issued string keys; document this compatibility rule. Internal registration uses non-pinning retention APIs. `clearTokenCaches` collects only zero-retention unpinned entries and never re-keys a live dependency or externally returned symbol. `getTokenCacheSize` measures live interned entries. Qualified-token interning must not call public pinning as an accidental internal lookup; test the bounded string registry independently of deliberately pinned returned tokens.

Snapshots store registration definitions and original token values, not unowned normalized-key caches; rebuild their normalized structures on restore. If implementation instead stores live bindings, explicit snapshot retention/release semantics must be reviewed before use—do not leak every snapshot forever.

### B. Completion and asynchronous materialization

Root-owned weak completion records distinguish in-progress from successful completion. Pending calls share work; failure permits retry. Track completed member/wrap/hook stages or restore original property descriptors so retries never stack wrappers and never mark unrun hooks complete. Each separate root applies its own members/hooks to a shared instance; retain the true original method for wrapper composition rather than wrapping another root's generated wrapper accidentally. Document that a shared mutable instance's current injected fields reflect the last successful root injection, not independent object copies.

Every materialization owns a build control with abort signal, terminal outcome and cleanup ownership, including transient work. Deadline covers async factory and onInit, all callers, and replacement builds; sync work stays sync. Deadline inherits from owning resolution scope unless registration `timeoutMs` overrides it; reject negative/nonfinite settings, document zero as immediate async deadline, and allow explicit disabled value without conflating it with inheritance. Timer cleanup is unconditional. Timeout is `DI_BUILD_TIMEOUT`; disposal cancellation receives a distinct documented `DI_BUILD_CANCELLED` error. Both abort before waiting and settle callers without waiting forever for uncooperative user promises. Late outcomes are consumed; late owned instances are disposed exactly once; externally owned instances are never disposed. For transients, cancellation cleanup covers only owned values that materialize after termination and were never returned to a caller; successfully returned ordinary transients do not become container lifecycle-owned. An onInit still running cannot publish readiness after termination. Cancellation does not claim to forcibly stop arbitrary user code.

`IContainer.signal` is present for the factory context: views expose build signal; ordinary containers expose lifetime signal. Initialization receives the appropriate build context without losing existing resolver/lifecycle behavior. Track pending transients for diagnostics and cancellation without caching their ready values.

### C. Extraction and views

Before moving code, inventory private dependencies into an implementation-only typed host contract (planned `src/core/cminject.host.ts` if a separate file is needed). Container supplies callbacks bound to its private methods; view and helpers consume that contract. No new bracket-private access or public `as unknown as` escapes.

Extract `async`, `binding` (records/handles and their lifecycle bookkeeping), `view`, and `observe` one at a time; Container remains orchestration. View explicitly implements all `IContainer` members and overloads. Container may establish the deliberate view prototype link at module initialization; this preserves `instanceof` without runtime imports from view back to Container or generated member forwarding. Verify runtime member inventory, getters/setters and receivers. The final Container target is under 900 readable lines; do not hit a line target by minifying declarations or moving unrelated code into arbitrary helpers. If the listed extraction is insufficient, Architect must approve a cohesive additional internal responsibility before extending the file inventory.

### D. Ambient and packaging boundaries

Root holds an optional ambient source; child attachment is rejected. Runtime owns ALS and attaches it to its root. Unrelated roots cannot adopt another root's active resolution. Importing server installs nothing. Preserve explicit `runInContext/currentContext` public behavior; distinguish request ambient context from build resolution context.

All source imports use package exports. Maintain explicit public and implementation-only entries for every extracted/server file. Types-first conditional exports avoid runtime-dependent declaration resolution: `types` -> dist declarations, `bun` -> source, `default` -> dist JS. First use TypeScript self-name output-to-input remapping with explicit rootDir/outDir and build decorate before inject; `rewriteRelativeImportExtensions` does not rewrite package imports. Add a source condition used only by build/test tools only if demonstrated necessary; no build may depend on stale pre-existing dist. Configure TypeScript custom conditions or build-time paths precisely and prove clean bootstrap. Source conditions are not a substitute for direct Node dist tests. Include emitted side-effectful decorator files in `sideEffects`; remove obsolete import-installed context entry. Do not remove decorate's existing public exports gratuitously; add explicit required build-resolvable routes and cover compatibility.

### E. Modules, snapshots and overrides

Tree-wide module identity uses a root-owned module registry and root-owned module bindings: calling `load` on a descendant delegates to the tree root. This makes once-per-tree deterministic and avoids caching a module on a short-lived request scope. Tenant/request service registrations still use their declared lifetime routing. Document this choice and test descendant calls. A repeated explicit load by the same module identity yields the same logical handle without accumulating invisible references; imported dependencies hold separately tracked edges. Concurrent loads join one pending transaction; dependency cycles are diagnosed before deadlock. Shared imports survive until no explicit ownership or import edge remains.

A module registration facade carries an explicit transaction across awaits. Both registration paths capture through one binding ownership hook. Unrelated mutations are never captured. Failed load removes only registrations still owned by that transaction and reverses owned resource creation; conflicting modifications are rejected with a mutation conflict, not undone silently. Unload is idempotent, removes dependents before imports, and aggregates disposal errors after attempting all cleanup. Root disposal invalidates module state and handles.

A `RegistrySnapshot` is opaque, tied to the originating container/tree and records local registration definitions, registry epochs and module ownership state relevant to that container. Restore rejects a foreign/disposed owner and concurrent conflicting mutation. Before any mutation, preflight the complete restore atomically: if a captured owned `registerInstance` value has since been disposed, reject with `MutationConflictError` and leave the current registry, epochs and resources unchanged. Its registration factory merely closes over the supplied object and cannot rebuild it. Preserve unchanged, still-live supplied records by identity; external supplied values can be reinstated without disposal. For factory-backed definitions, dispose post-snapshot owned records (including affected descendant caches) and rebuild as needed. Reinstate registrations/epochs without reusing any disposed object. Snapshot restore does not resurrect disposed scopes. Module handles restored by definition must agree with registry state, not claim unloaded bindings still exist. Define fresh generations for recreated bindings even if visible registry epoch is restored, so observation/cache identities cannot confuse old resources with new ones.

`override` is async and only exposes a handle after replacement succeeds. Reject an owned `registerInstance` original with `MutationConflictError` before calling replace or disposing anything: no recreatable factory exists, and silently retaining the original would violate `retainPrevious:false`. External supplied values remain restorable without disposal; factory-backed originals are rebuilt. Nested restore is LIFO; out-of-order restore rejects. Restoration reinstates a definition and rebuilds normally, not an object disposed by `retainPrevious:false`. Function-valued services must have an unambiguous value/factory route; preserve the requested convenience overload and supply explicit tagged value/factory overloads if required to distinguish them. `createTestContainer(...modules)` is async when loading modules, returns a root with 2000ms timeout and enabled observation; no floating module-load promises.

### F. Lazy slots, conditional selection and graph completeness

Unify dependency slots across constructors, members, decorators, lifecycle deps and typed factory `inject`. `lazy`/`provider` defer resolution and retain the actual resolving scope, not a completed factory frame. Calling after scope disposal fails normally. Lazy edges are omitted only from eager-cycle traversal; both lazy/provider edges still receive lifetime/availability validation. Providers resolve per call (new transient each time); lazy get/getSync follows the token's normal lifetime rather than introducing an undocumented singleton cache. `peek` does not build.

Conditional single bindings remain distinct binding identities under one token. Evaluate predicates in registration order, then at most one unconditional fallback regardless of its position. Ready caches are keyed by selected binding, never token alone. Preserve hierarchy/multi-shadowing and explicit replacement/unregister-by-ID semantics. Predicate context is requester/path/resolving-scope; cached gets without requester use the documented root context. Two unconditional singles remain rejected rather than silently accepted; `DiGraph` diagnoses ambiguous unconditional candidate sets and the mutation path exposes the corresponding diagnostic without committing invalid registrations. Static analysis does not pretend to execute arbitrary predicates or prove mutually exclusive conditions.

Observed drift is generation/scoped binding-identity aware, bounded and invalidated on mutations; it cannot merge a stale binding into the current graph. `requireDeps:false` preserves old validation behavior; explicit empty deps counts as declared. `includeObserved` uses the existing log and must find a dynamic cycle, not only append a warning list.

### G. Public typing and compiler pass

Implement tuple inference and arity validation for all five lifetimes and their multi variants; old container/data factory overloads remain valid. Type tests must exercise both positive inference and deliberate errors under the actual supported TypeScript version, not only transpile test files.

Typed modules use explicit named capability maps for `Provides`/`Requires`, so a diagnostic can name missing keys and verify compatible value types. Ordinary `Token<T>` symbols are structurally typed by service and cannot magically encode runtime identity; do not claim compile-time distinction between two same-type ordinary symbols. `defineModule<Provides, Requires>()` is the opt-in curried typed layer over the same runtime module, and `composeModules` checks the complete union, allowing dependencies provided by any sibling/import as specified. Add tests for missing names, incompatible provided types, multiple providers and correct inferred maps.

The ctsc pass lives at `../../tools/ctsc/src/passes/cminject.paramtypes.ts` (inside current rootDir), exported through an explicit pass subpath. Follow the pinned native compiler API, not assumed TypeScript 5 transformer APIs. Resolve decorator symbols/aliases/re-exports to inject class decorators. Preserve explicit static `$inject` and decorator deps. Emit class value expressions only when accessible at runtime; type-only imports may be rewritten to safe value imports when a runtime export exists, otherwise emit `undefined` plus precise `ctx.error`. Interface/union/primitive/generic-unresolvable slots get diagnostics naming class and parameter. Parameter properties and inherited/no-constructor cases must not synthesize wrong positions. The pass remains opt-in through ctsc configuration and is installed in split's compiler stage without adding imports from inject into its consumers' source.

## Execution stories and item-by-item traceability

Each story is a durable Ultragoal unit; it may contain bounded native lanes, but only the leader checkpoints it. Product files are owned by one writer at a time. Dependencies below are gates, not permission to stop after a story.

| Story | Plan items | Resources and work | Completion evidence | Dependencies / lane |
|---|---|---|---|---|
| G00 Baseline and inventory | 0.1, 0.2 | `package.json`, `vitest.config.ts`, React/type coverage config, `tests/FEATURES.md`; add matching coverage-v8, Node coverage script, runtime-specific functional scripts; record pre-change measured per-file floors and feature/API test index | Actual coverage reports + exact version/runtime/command; every feature and IContainer member has a row and existing/gap status; no invented counts | First; tooling owner |
| G01 Characterization | 0.3 | New `tests/bun/server/runtime.test.ts`; core hot/inspect/interceptors/health/discovery files; runtime assertions; script discovery of server and docs; correct registry-only epoch assertions | >=5 meaningful cases per named area; tests prove all listed behaviors; Bun and Node runs, legacy cwd preserved | G00; tests can split by file |
| G02 Token/completion correctness | 1.1, 1.3; retain 1.2 | Token retention APIs and every registration/mutation/read path; root completion state with retry progress; tests in core/static/construct and React regression | 10k token/100-scope reclaim; re-register dependency identity; public pinned symbols; concurrent completion/failure retry/cross-root wrapper safety; React still green | Relevant G01 tests; one core writer |
| G03 Build control | 1.4 | container/contracts/errors; build cancellation/deadline tracking; abort signals; disposal/late-result guards; dedicated timeout tests | All first/shared/transient/onInit/disposal/late result tests; zero unhandled rejections; exact-once cleanup; sync no-promise assertions | G02; same core writer |
| G04 Type tightening | 1.5 | token/contracts/container/decorators; real legacy/standard overloads; `tests/types`, typecheck configs/scripts | Inference/negative placement/data/multi cases; source typecheck; inventory of remaining justified internal casts; no public double casts | G03; type lane prepares tests, core writer integrates |
| G05 Typed extraction and view | 2.1, 2.2 | async/binding/view/observe and optional host files, incremental internal exports; explicit IContainer forwarding; archive replaced material; DECISIONS | Full existing/characterization suite after each extraction; no runtime cycles/private escapes; member conformance; instanceof/identity/getters; readable Container size target | G04; serialized structural writer |
| G06 Root ambient and exports | 2.3, 2.4 | context/runtime/container and package import inventory; explicit public/internal/server subpaths; source/test/script import migration; import-policy checker | Two-root isolation, captured-cycle check, import-only neutrality; no wildcard or `/src/` inject imports; all subpaths resolve; browser gate | G05; core and package owners coordinate serially |
| G07 Dependency and Node distribution | 3.1, 3.2, 3.3 | decorate build/exports/internal self-import paths first; inject conditions/build/files/sideEffects; `tests/node/*.test.mjs`; Node Vitest script; CLAUDE/README runtime status | Clean decorate→inject dist build; direct Node import no transpiler; core/static/runtime/ALS smoke; all existing suites on compatible Node; minimum library runtime smoke; Bun source and browser gates | decorate prep may start after G00; final inject after G06 |
| G08 Modules | 4.1 | module file/contracts/container/view/graph/inspect; transactional facade, root identity registry, import refs, rollback and diagnostic module labels | Idempotent and concurrent loads, cycle, async capture, fast-path capture, interleaved unrelated mutation, rollback/retry, shared-import and root-disposal tests | G06; serialize core integration |
| G09 Testing utilities | 4.2 | testing entry, snapshots/contracts/core/view; async overrides/createTestContainer; tests/testing | Singleton/scoped/multi snapshots; foreign owner rejection; live supplied identity retention; atomic conflict for disposed owned supplied originals; owned supplied override rejected before disposal; external restoration; factory rebuild; nested LIFO; module state integration | G03 + G08 |
| G10 Deferred and typed dependencies | 4.3, 4.4 | lazy file, token/slot contracts/decorators/graph/container/view; tuple overloads all ten methods; tests/types + all dialect runtime tests | Lazy broken cycle/captive rejection/getSync; provider transient counts and scope lifetime; all slot paths; positive/negative tuple types and data overload preservation | G06; serialize core; isolated type tests lane |
| G11 Graph, qualification and conditions | 4.5, 4.6, 4.7 | graph/observe/token/container/contracts/view/inspect; qualified intern map; candidate selection identities | requireDeps opt-in; dynamic-cycle merge and drift; stale edge pruning; qualified names/identity/decorators; predicate order/fallback/no-match/ambiguity/cached candidate isolation/hierarchy/multi tests | G02 + G10; 4.6 may prepare separately, merge before 4.7 |
| G12 Typed module composition | 4.8 | module/contract public types, typed composition tests and examples | Missing capability named by compile error; type incompatibility rejected; complete union inferred; runtime module behavior unchanged | G08 + G10 |
| G13 Compiler integration | 4.9 | ctsc `src/passes/cminject.paramtypes.ts`, public export/tests/fixtures; split build/config and minimal compiler bridge/tests | Pinned-native-API typecheck/build; emitted class deps; explicit metadata precedence; aliases/value imports/unresolvable diagnostics; split-produced client and server fixture prove pass ran | Parallel research/prep after G00; integration after stable exports/decorators G06/G07 |
| G14 Benchmarks | 0.4 | `scripts/bench.compare.ts`, explicit budget mode in `scripts/bench.ts`, `bench/results.json`; approved competitor devDeps | Every specified scenario runs or is explicitly unsupported per library; semantic assertions, warmup/sample/runtime versions; real result table/JSON; budget failure test | Independent after G00, rerun after G12; nonblocking interim correctness but required final deliverable |
| G15 Docs and examples | 5.1, 5.2 | thirteen named guide/migration docs, TypeDoc output/script, example per guide, examples test, concise README | All named files, all error codes, verified migration claims; TypeDoc succeeds; every runnable example executed; links/imports/API consistent | Draft independently after API contracts, final after G07–G13 |
| G16 Reconcile and release gate | 6.1 + every row | FIX-PLAN/DECISIONS/CLAUDE/PRIO_ONE status; feature index/coverage ratchet; archived-file manifest; final review artifacts | Full verification matrix fresh, clean static policy/cycles/types/build/tests, all story evidence, independent cleaner+code-reviewer+architect approve; no partial status presented as complete | All preceding stories |

### Dependency/parallel schedule

- Start G00, then G01 test files in up to three bounded lanes. In parallel, inspect/prepare decorate packaging, compiler pass evidence and comparative benchmark adapters without touching shared inject sources.
- Core lane serial sequence: G02 → G03 → G04 → G05 → G06 → G08 → G09 → G10 → G11 → G12. G07 packaging integration pauses source writers to avoid export churn; it may precede feature work as PRIO_ONE orders.
- One tooling/package owner edits inject package.json/tsconfigs/vitest configs/lockfile; other lanes report required entries rather than racing edits. One core owner edits container/contracts/view/decorators at a time. Dedicated sibling owner may edit decorate/ctsc/split in disjoint phases, respecting their local AGENTS rules.
- Cheap bounded executor lanes: characterization files, examples, feature index, basic token qualification tests, benchmark adapters. Frontier/core executor: timeout/ownership/snapshots/conditional selection; architect reviews invariant boundaries. Specialist research: version-specific TypeScript native API, conditional exports, Vitest coverage and competitor migration APIs, using official upstream evidence only.
- Four simultaneous slots maximum in this surface including leader. Native role model failures are recovered with a supported role/model, not repeated failed spawns. No nested orchestration by leaf executors. Team/tmux is optional only if durable coordination adds value; do not block native delivery on missing tmux.

## Per-story handoff protocol

1. Leader assigns exact files and acceptance cases, includes current approved architecture and explicitly says other writers exist.
2. Before refactor, archive replaced source/test/doc versions in repository `.archive/2026-10-27/<original-relative-path>` without overwriting earlier archive evidence; then lock behavior with failing/passing characterization as appropriate. No code deletion outside caches/build output.
3. Implement smallest complete behavior; use existing helpers (`then`, `each`, etc.). No new runtime dependencies beyond plan scope.
4. Run focused verification, then applicable aggregate gates; record command, runtime version, exit code, actual test count and artifact paths. Fix failures in lane; report cross-file dependency to leader.
5. Update DECISIONS with behavior changes at integration, not only at end. Leader audits requirements and checkpoints story in Ultragoal; user-facing update states result/evidence, not asks to proceed.

## Verification and stop rules

Use `.omx/plans/test-spec-prio-one.md` as executable acceptance matrix. Maintain evidence under `.omx/evidence/prio-one/`; generated benchmark results remain `bench/results.json`, public test index remains `tests/FEATURES.md`. Baseline commands run before behavior changes; full suites may be run in the active execution stage per handoff. No implementation occurs during this planning stage.

Do not call completion when only compilation passes, a benchmark prints, a doc exists, a checker is skipped, a user callback was left TODO, or all ledger rows were merely marked done. Do not reduce the plan because it is large. Re-enter planning only when code/review proves the architecture invalid; otherwise continue safe implementation and verification automatically.

## Review handoff

Architect: validate token exposed-key pinning, completion retry/wrapping, deadline/disposal lifecycle, host/prototype acyclicity, conditional identity/caching, root module ownership, snapshot generations and source/dist clean bootstrap; challenge the module root-owner and named capability-map decisions against the requested APIs. Provide steelman alternative, real tradeoff and approving/iterate verdict.

Critic after Architect: enforce all 28 original item IDs, testable coverage of negative/recovery paths, dependency order and realistic file ownership, no invented baseline/benchmark values, no missing sibling integration, no blanket compatibility claims, and the final all-plan stop condition. Planning completes only after both sequential approvals and the session/review-cycle-bound Autopilot execution handoff exist.
