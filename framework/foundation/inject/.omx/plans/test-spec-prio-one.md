# PRIO_ONE acceptance and verification specification

Companion to `.omx/plans/prd-prio-one.md`; authoritative item requirements remain `PRIO_ONE.md`. Official toolchain evidence: `.omx/plans/prio-one-evidence.md`. This file defines work to execute after planning approval. No runs below are claimed to have happened unless existing evidence is explicitly identified.

## Runtime/toolchain preparation

- Work from `framework/packages/foundation/inject`. Bun is the package-local `.cache/bun/bun-darwin-aarch64/bun` (currently verified version 1.3.0); add its directory to this command environment's PATH rather than require a global install. Do not stage tools or fixtures in `/private/tmp`.
- Record `bun --version`, `node --version`, compiler/Vitest/dependency versions. Node's test-runner version must satisfy installed Vitest (>=22.12 for Vitest 5). Library smoke separately covers supported Node >=22 behavior; if minimum runtime is unavailable, install a package-local tool cache or report exact gap rather than imply it ran.
- All fixtures/build outputs are package-local cache/test fixture/dist paths. No production inputs. No state-changing git commands. Use read-only status/diff to detect unrelated changes.
- V8 coverage uses Node, not Bun. Bun functional suite must assert `process.versions.bun` in worker context so a script-name-only runtime switch cannot falsely pass.
- Preserve legacy cwd/config: run that leg from `tests/bun/legacy` with the intended config/root and explicit Bun binary. Confirm discovery and decorator settings before trusting case counts.
- Coverage baseline includes core, server and React adapters through correctly scoped projects/configs; uncovered source files remain included. Type-only files do not masquerade as covered runtime code. Pin coverage-v8 to the installed Vitest-compatible version; record real measured floors, then ratchet.

## Required scripts/contracts to land in G00–G07

| Script | Required meaning |
|---|---|
| `test:bun` | Bun worker execution of core, standard, static, server and docs tests when present, with no React/Node fixture accidental discovery |
| `test:bun:legacy` | Bun with legacy cwd-specific tsconfig; explicit config/root selection, real runtime assertion |
| `test:react` | Existing React config under explicitly selected runtime; executed once on Bun and once on Node at final gate |
| `test:node` | Existing relevant Vitest suites on Node, not source-only direct import claims |
| `test:coverage` | Node/V8 coverage across runtime source, measured per-file thresholds and machine-readable output |
| `test:types` | Vitest typecheck configured for `.test-d.ts`, explicit typecheck include/tsconfig, `ignoreSourceErrors:false`, `vitest run --typecheck.only`, and negative diagnostics; zero silently skipped files |
| `check:imports` | Package import-policy plus explicit export resolution and runtime import-cycle assertions; no `/src/` self-imports or forbidden relative source imports |
| `check:browser` | Browser bundles for every browser-safe public and internal entry; no Node/ALS dependencies |
| `build` | Declaration/JS emit from clean source conditions without pre-existing dist dependence |
| `docs:api` | TypeDoc generates actual public API docs and fails on bad entry/type/config errors |

Scripts may use existing shared tools. A newly created static checker is package-scoped and dependency-free. No claim that a nonexistent lint command ran: report typecheck, import policy and cycle checks by their actual names.

## Per-item acceptance matrix

### Phase 0: safety and measurements

**0.1 Coverage:** first execution saves JSON/text/HTML reports and per-file statement/branch/function/line values in `tests/FEATURES.md`, with runtime/version/commit-independent date and exact command. Set thresholds no greater than measured baseline; never lower them to hide regressions. Include React and server coverage; coverage-only worker executes on Node. Later file moves preserve or improve responsibility-level coverage and new files get explicit thresholds.

**0.2 Test index:** every capability in PRIO_ONE §4.3 and each IContainer member/overload family maps to test file, actual executed case count and status. At baseline gaps may be marked; final matrix has no untested required feature. Parameterized cases counted from runner results, not only grep lines. Newly added APIs are indexed too.

**0.3 Characterization:** at least five substantive cases for each named area, including all listed scenarios:
- runtime: start/state; once-per-tenant configure under concurrency; double release; eager/lazy request; drainTenant with live leases; drain then dispose; each onError phase and observer error isolation.
- hot: dependent rebuild order, successful replacement, old instance disposal, failed replacement rollback, external ownership preservation.
- inspect: snapshots, binding/resource graphs, dependency tree, ownership records, registry-only epoch mutation; resolving a service alone must not be mislabeled registry mutation.
- interceptors: parent/child order, requester/path, hook-error cause attachment, suppression, sync/async equivalence and cache-hit behavior.
- health: lifecycle hook, instance fallback, unhealthy/missing instances, scope traversal, async/error aggregation.
- discovery: tag and method lookup, onInstance registration/unsubscribe, scope inheritance, generated methods/metadata and no duplicate notifications.

**0.4 Benchmarks:** assert equivalent work before timing: values resolved, service counts, disposal counts. Measure cached get, cold 100-node graph, depth-10 async chain, 10k scope create/dispose, 1000 request scopes ×5 services. Every library is supported or explicitly N/A per scenario. Record versions/runtime/hardware, warmup, sample count and statistic in markdown/JSON. Comparative results are informational, not flaky pass gates. Existing budget benchmark accepts a documented explicit budget; inject a deliberately impossible budget to prove nonzero exit. Preserve real measured baseline and rerun after all features.

### Phase 1: correctness

**1.1 Tokens:** stress 10,000 distinct strings across 100 tenant scopes and assert live unpinned count returns to start. Cover direct factory fast path and general install, multi, duplicate failed registration, same name across roots, declared dep retained while provider removed, dynamic cache retained/replaced, successful/failed replacement, unregister handle and container disposal. Missing resolve/get/has/binding queries do not grow the registry. Public normalize/getGlobal keys remain identical after unregister/clear/re-register. Snapshot/qualified-token work later reruns these tests. Do not depend on nondeterministic GC to pass the core reclamation test.

**1.2 React:** preserve existing implemented adapter cases: async and sync failure retained before retry, StrictMode one factory call, falsy rejections, explicit reset then success, reset-pending dedupe, per-scope/token isolation, synchronous reads and Suspense success. Run same adapter file on Node and Bun; Bun worker assertion must run, not skip, in Bun leg. Current historical evidence is 19 Bun passed, 18 Node passed +1 runtime-only skipped; refresh after changes.

**1.3 Completion:** same instance injected in two roots gets each root's member/hooks; child shares root completion; second same-tree call no-op. Concurrent async calls await one completion, not a partially injected object. Member rejection and hook rejection reject all waiters then permit retry. Successful prior wrapper stages are not duplicated on retry; two roots do not accidentally stack generated wrappers. Sync injection stays synchronous and async member throws the documented LifecycleError from sync API. Test reentrant completion to ensure no self-await hang.

**1.4 Timeout/cancellation:** deferred promises avoid flaky wall-clock assertions. All affected callers (including initial/shared/transient) reject DI_BUILD_TIMEOUT; hanging onInit covered. Inherited timeout, child override, registration override, disabled/default, zero, invalid values. Error includes relevant pending builds/path/waits edges and token. Signal aborts on timeout and before owner disposal waits. Abort-aware and ignoring factories both allow disposal to settle. Late factory success, late onInit success and late rejection do not cache/revive; owned disposal occurs once, external never. Cancelled transient cleanup disposes only never-returned owned late values; successfully returned ordinary transients remain outside container lifecycle ownership. Immediate retry after timeout cannot be overwritten by old completion. Replacement timeout preserves original registration and disposes only candidate. Timers and waits-for edges cleaned; no unhandled rejections. Sync factories/get remain immediate. Runtime loop exits naturally once pending timers are otherwise the only work.

**1.5 Types:** positive constructor/class token inference for constructors with actual required parameters; abstract tokens accepted but not instantiated. Factory data type correctness; multi element inference; all legacy/TC39 placement overloads; invalid placements use expected compile errors. Compile public consumer fixtures with no source-path imports. No public-path double assertions; inventory remaining internal assertions and their one-line invariant explanation. Tests must typecheck, not merely transpile.

### Phase 2: structure

**2.1 Extraction:** run core/decorator/server/React characterizations after each move, not only final extraction. Compare exported API and runtime behavior. Runtime dependency graph has no cycles; view/host type imports do not emit edges. No bracket-private calls or new double casts. Record readable Container line count against under-900 target; do not minify to pass. Archive replaced material under exact original path with no overwrite of earlier backups.

**2.2 View:** type assignment to IContainer and explicit runtime member inventory cover every method/getter/setter and overload family. Instanceof Container remains true; identity id matches target; mutating methods act on real host. Resolution methods retain active context and stop retaining it after completion; retained factory view does not create stale-cycle errors. Signal reads build control; autoInject/buildTimeout setters have intended receiver. Names cannot silently shadow host contract.

**2.3 Ambient:** importing server changes neither standalone root's behavior. Two runtime-owned roots have independent ALS; captured-container cycles are detected only under the correct attached context. Cross-root calls never borrow another root's build frame. Child `useAmbient` rejected. Runtime disposal/reattachment behavior documented and tested. `currentContext/runInContext` public request scope behavior remains.

**2.4 Exports:** inventory all source/re-exports/type imports/tests/scripts. No inject `/src/` import or wildcard remains; all declared public/internal/server entries resolve. Browser public/internal entries reject any Node built-in dependency, not only text containing ALS. Server entry is never pulled into browser core. Tests imported through approved subpaths stay runnable. Capture package export-map snapshot and source/runtime import graph.

### Phase 3: distribution

**3.1 Inject build:** archive or remove build output only, build clean with no previously emitted declarations. Check `.js` and `.d.ts` for every export, correct source/build conditions and sideEffects, files includes dist. `node -e "import('@coldmind/inject').then(m => { if (!m.Container) process.exit(1) })"` succeeds without loader/transpiler/source condition. A Node TypeScript consumer resolves dist declarations. Bun resolves source and browser bundles pass. Package tarball inspection (no publish) proves all referenced outputs/dependencies included.

**3.2 Decorate build:** clean emitted main and internal entries, same condition ordering and built metadata sideEffects. Standard/legacy record tests unchanged. Direct Node import before inject build succeeds; no source-only decorate path leaks into inject dist. Preserve existing public surface.

**3.3 Node runtime:** `tests/node/*.test.mjs` run via node --test against compiled exports, covering sync/async lifetimes, decorator-free classes, tenant/request roundtrip, cancellation and ALS captured cycle. Existing Vitest suites run on Node separately. Run smoke on declared minimum library major/version and current compatible runner; distinguish the two in evidence. README/CLAUDE Node claim appears only after compiled smoke passes.

### Phase 4: APIs and interactions

**4.1 Modules:** identity idempotence (not name equality), depth-first imports and cycle diagnostic; descendant calls share root-owned registration semantics; async register + direct fast registration capture; concurrent same-module join; unrelated interleaved registration excluded; failed load rolls back and retries; conflicting mutation rejected without undoing unrelated work; shared imports survive remaining explicit/dependent ownership; unload twice; disposal errors aggregate but remaining cleanup runs; root dispose invalidates handles; module labels appear in graph/inspect/issues.

**4.2 Testing:** snapshot/restore singleton/scoped/multi and affected descendant caches; externally supplied/owned values distinguished; post-snapshot instances disposed; disposed instances/scopes not revived; foreign snapshot rejected; restore registry epochs and internal generations coherently. An unchanged still-live owned supplied record retains exact object identity through restore. If a captured owned supplied original was disposed, restore preflight rejects atomically with MutationConflictError before changing any registration, epoch, identity or disposal count. Owned `registerInstance` overrides reject before replacement/disposal, with original identity and disposal count unchanged. External supplied values restore by identity with zero disposal; factory-backed originals rebuild through their factory rather than reusing disposed objects. Failed replacement returns no override handle and keeps original. Nested override LIFO restore, out-of-order error, function-valued service ambiguity route. Restored original rebuilt rather than disposed-object reuse. Module snapshots restore ownership consistently. createTestContainer awaits async modules, timeout 2000 inherited, observation enabled.

**4.3 Lazy/provider:** constructor deps, lifecycle deps, `$inject`, `$members`, and Inject in both decorator dialects all accept markers. A→lazy(B)→A works after A completes; eager cycles still fail; lazy and provider captive dependencies still rejected. Provider yields new transient per call and honors scoped/tenant/request lifetimes; lazy follows normal token lifetime. Scope-bound resolution uses originating scope; disposed scope access rejects. getSync on async build errors correctly; peek never builds; missing deps diagnose clearly. Do not accidentally treat markers as registered class tokens.

**4.4 Typed factories:** all ten registration variants accept readonly inject tuples with service/lazy/provider value inference, sync/async return and appropriate lifecycle typing. Wrong element type and wrong tuple length fail compilation. Declared graph edges derive from inject, including empty tuple. Existing `(container,data)` forms still work and do not become ambiguous overloads. Runtime tests count exact args/order/instances.

**4.5 Graph:** undeclared only when requireDeps=true; empty deps declared; declared-versus-observed missing edge; observed-only cycle; multi/conditional binding identity distinguishes providers; stale generations and removed records excluded; bounded log remains bounded. includeObserved=false preserves old behavior. Inspect returns drift matching validate, module/token names retained.

**4.6 Qualification:** repeated same base/name returns same symbol; different name/base differs; class, string, symbol and delayed-base equivalence documented; two qualified Db bindings resolve and inject correctly; graph names `Db#replica`; no accidental underlying string pinning by internal lookup; diagnostics distinguish qualifiers.

**4.7 Conditions:** first matching predicate in registration order; unconditional fallback only after predicates; no match/no fallback errors name token/conditions; candidate cache isolated per binding (two requesters get intended instances); predicate failures surface; duplicate unconditional candidates produce ambiguous graph diagnostic without corrupting committed registry; hierarchy and multi shadowing unchanged; replacement/unregisterBinding target one candidate; token unregister removes local candidates; graph never pretends predicates are statically evaluable. Test use from typed factories and modules.

**4.8 Module types:** named provided/required maps infer composition output; missing requirement diagnostic includes capability name; wrong provided type rejected; sibling/import provided requirements satisfied; ordinary runtime module API unchanged; duplicate same-service symbols are not falsely claimed statically distinct. Type tests compiled under supported TS versions.

**4.9 ctsc/split:** build pass with pinned TS7 native checker/AST APIs. Fixtures for direct/aliased/re-exported inject decorators, unrelated same-name decorator ignored, constructor parameter properties, inherited/no ctor, explicit `$inject`, decorator deps precedence, namespace/value imports and type-only import handling. Class tokens emitted in order; interface/union/primitive/unresolved generic slots emit undefined and precise diagnostics. No invalid emitted runtime references. Fixture output is typechecked and executed with inject. Split integration invokes pass before bundling both reached client classes and generated/referenced server classes; inspect emitted `$inject` and run a client/server fixture. Existing split module discovery/security boundaries unchanged. Repeated builds do not duplicate metadata or overwrite source. No changes to unrelated application packages.

### Phase 5 and 6

**5.1 Docs:** all getting-started/lifetimes/async-and-cycles/modules/testing/decorators/server/react/introspection/errors docs and three migration guides exist. Every public error code described, including new timeout/cancel/module/condition diagnostics. Migration API claims verified against official installed/upstream library docs and actual adapter examples; no unverified competitor table promoted as fact. TypeDoc builds from public entrypoints with no unresolved public links.

**5.2 Examples:** one runnable, deterministic example per named guide/migration guide (API reference generated rather than separately handwritten); test imports and executes each, checks results, and disposes all resources. No example requires production credentials/network. React example renders in configured DOM/server fixture as appropriate rather than being skipped. README has pitch/install/working example/exports/links. All docs commands execute from stated directories.

**6.1 Reconcile:** FIX-PLAN completed entries and obsolete async-class claim corrected; ctsc/timeout links accurate; DECISIONS records all changed invariants contemporaneously; CLAUDE matches new runtime/layout; PRIO_ONE every item status references evidence; feature matrix counts refreshed; no outstanding required feature silently marked done. Archived files listed. Final code-reviewer and architect verdicts clean after cleanup and rerun.

## Aggregate command gate

After required scripts exist, run from package root with the local Bun directory in PATH. Record outputs, not just command strings. Example command sequence (adapt binary paths to observed workspace tools without changing meaning):

```sh
export PATH="$PWD/.cache/bun/bun-darwin-aarch64:$PATH"
bun --version
node --version
bun run test:bun
bun run test:bun:legacy
bun --bun run test:react
bun run test:node
node ../../../node_modules/vitest/vitest.mjs run --config vitest.react.config.ts
bun run test:types
bun run typecheck
bun run test:coverage
bun run check:imports
bun run check:browser
# Build decorator dependency before inject; both must also pass a clean-output run.
(cd ../decorate && bun run build && bun run typecheck && bun run test)
bun run build
node --test tests/node/*.test.mjs
node -e "import('@coldmind/inject').then(m => { if (!m.Container) process.exit(1) })"
(cd ../../tools/ctsc && bun run typecheck && bun run build && bun run test)
(cd ../../tools/split && bun run typecheck && bun run test)
bun scripts/bench.ts
bun scripts/bench.compare.ts
bun run docs:api
# docs examples are included in test:bun; keep an explicit focused rerun in final evidence.
bun --bun x vitest run tests/bun/docs/examples.test.ts
git diff --check -- . ../decorate ../../tools/ctsc ../../tools/split
```

Package scripts for siblings are added only where needed and run under their documented compiler/runtime, not assuming Bun supports every native compiler API. If ctsc native compiler requires Node invocation, run its tests under Node and retain Bun execution for split/inject fixtures. No benchmark result or full suite gate is waived because a different runtime passed.

## Static and observability evidence

- Capture explicit export inventory and zero emitted runtime cycles; source checks distinguish `import type` from runtime imports.
- Snapshot pending-build diagnostics at timeout and compare expected tokens/path/waits edges; use counters for exactly-once disposal and observer/hook calls.
- Module and snapshot tests report registration generations/epochs before/after interleavings to establish ownership, not merely final get() values.
- Coverage reports, executed test manifests, benchmark JSON and docs/API output have runtime/version metadata where appropriate.
- Test timeouts guard against hanging tests; they do not count as the expected DI timeout behavior. Verify expected error codes before the runner's timeout fires.

## Completion audit

Leader links every item above to concrete artifacts and fresh passing results in Ultragoal/PRIO_ONE. The final audit verifies no placeholders, skipped mandatory tests, API stubs, unhandled promises, fake interfaces, undocumented breaking behavior or unrelated changes. Run bounded cleanup review, rerun affected and aggregate gates, then independent code-reviewer and architect. Only then mark the aggregate Codex goal and all-plan ledger complete.
