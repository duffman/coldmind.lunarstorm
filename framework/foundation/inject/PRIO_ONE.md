# cminject: priority-one remediation plan

Written 2026-10-27. Self-contained: an agent can start work from this file without first reviewing the package. Line
numbers are anchors as of that date; they shift as work lands, so search for the named symbol when one does not match.

Contents:

1. Working rules (read first)
2. Package context
3. Architecture: how a resolve works
4. Evaluation findings (the why behind every plan item)
5. The plan
6. Order, approvals, commands

---

## 1. Working rules

These are the owner's standing rules for this repository. They override agent defaults.

- **Git: hands off.** Run no state-changing git command (add, commit, branch, switch, stash, reset, restore, rebase,
  merge, push, pull, clean, rm, mv, tag, config, worktree) unless the owner's current message names it. Read-only
  commands (status, log, diff, show, blame) are fine. Subagents never change git state.
- **Never delete code.** Move a source file, test, config or doc to `<repo root>/.archive/<YYYY-MM-DD>/<original
  relative path>` with `mv`, and say what was archived. Only build output and caches may be removed outright.
- **Never downsize the library.** Do not remove, gate or move out a feature. Users are unknown. Replacing an
  implementation is fine; dropping a capability is not.
- **Imports go through package exports.** Import `@coldmind/...` subpaths, never `./relative` paths. Today the package
  imports itself as `@coldmind/inject/src/core/...`; item 2.4 moves that to explicit subpaths.
- **Package self-contained.** Package source and tests never name consumers. A task on this package edits only this
  package. Items that touch another package (3.2 `decorate`, 4.9 `ctsc`) are separate tasks.
- **Bun and Node, both.** Framework packages stay runtime-neutral. Node support (Phase 3) is intended, not optional.
- **The owner runs the suites.** Do not run full test suites, builds, benchmarks or full-codebase lint/typecheck
  yourself. Write the tests and scripts, then list the commands (section 6). Running a single focused test file to
  check one change is fine.
- **No attribution.** No `Co-Authored-By`, "Generated with", or any mention of AI tools in code, comments, docs, commit
  messages or PRs.
- **Approval.** New files and layout changes are listed in section 6 and need the owner's go-ahead before they are
  created. A change the owner orders by name counts as approved.
- **Style.** Match the surrounding code: dense one-line JSDoc on every declaration, no inline narration, `then`/`each`
  helpers for sync-or-async flow, terse error messages that name the token.

## 2. Package context

**What it is.** `@coldmind/inject` (internal name cminject) is a hierarchical dependency-injection container for Bun,
browsers and React. Location: `framework/packages/foundation/inject`. Version 1.0.0, pre-release.

**Entries** (`package.json` `exports`, all pointing at `.ts` source):

| Import | File | Contents | Runs in |
|---|---|---|---|
| `@coldmind/inject` | `src/core/cminject.core.ts` | `Container`, tokens, lifetimes, graph, decorators, errors | Bun, browser |
| `@coldmind/inject/server` | `src/server/cminject.server.ts` | `DiRuntime` (tenants, request leases, drain), `currentContext()` | Bun |
| `@coldmind/inject/hot` | `src/cminject.hot.ts` | `hotReplace` | Bun, browser |
| `@coldmind/inject/inspect` | `src/cminject.inspect.ts` | `inspect()` snapshot | Bun, browser |
| `@coldmind/inject/react` | `src/cminject.react.ts` | `ContainerProvider`, `ScopeProvider`, `useService`, `useScope` | React 18/19 |
| `@coldmind/inject/*` | any file | wildcard; exists so self-imports through `src/` resolve | n/a |

**Files and sizes (original planning snapshot; not current line anchors):**

| File | Lines | Role |
|---|---|---|
| `src/core/cminject.container.ts` | 1999 | `Container` (lines 358 to about 1750), `ResolutionView`, `Handle`, `Binding`, `StoredRecord`, async helpers |
| `src/core/cminject.contracts.ts` | 715 | every public type: `IContainer` (line 550), `ServiceLifetime`, `ServiceLifecycle`, `Interceptor`, graph and snapshot shapes |
| `src/core/cminject.graph.ts` | 369 | `DiGraph`: static validation (`issues`, `assertSound`, `order`, `dependents`) |
| `src/server/cminject.runtime.ts` | 272 | `DiRuntime`, `RequestScopeLease`, `DiRuntimeError` |
| `src/core/cminject.decorators.ts` | 242 | `describe(ctor)`, `defineDecorator`, `@Injectable`, `@Singleton`...`@Request`, `@Inject`, `@InjectOptional`, `@InjectAll` |
| `src/cminject.react.ts` | 127 | React adapter |
| `src/core/cminject.errors.ts` | 74 | `DiError` (with `code`), `LifecycleError`, `NotRegisteredError`, `CircularDependencyError`, `DiGraphError`, `MutationConflictError`, `ScopeMissingError` |
| `src/core/cminject.token.ts` | 72 | `createToken`, `normalizeToken`, `delay`, `tokenName`, id sequence |
| `src/server/cminject.context.ts` | 22 | `AsyncLocalStorage` ambient scope; installs the ambient resolution hook at import |

**Hard constraints** (package `CLAUDE.md`):

- Name files `cminject.<role>.ts`. A group gets its own directory only when it has more than two files.
- `src/core/`, `cminject.hot.ts` and `cminject.inspect.ts` import no `node:` module, because the browser bundle depends
  on them. `scripts/check-browser.ts` checks this.
- `src/server/` is server-only and reachable only through `./server`. `cminject.react.ts` imports only core and React.
- Resolution passes an explicit `Resolver`. Core holds no ambient async state.

**Decorators.** There are three dialects, and all of them write one `ClassDescriptor` (`describe(ctor)` in
`cminject.decorators.ts:69`):

- legacy, with `experimentalDecorators`, `reflect-metadata` and `emitDecoratorMetadata`;
- TC39 standard;
- decorator-free, with `static $injectable`, `$inject` and `$members`.

The dialect plumbing and per-class records live in the sibling package `@coldmind/decorate`
(`packages/foundation/decorate/src/decorate.*.ts`). Bun has no `Symbol.metadata`; `decorate.metadata.ts` polyfills it
with `Symbol.for('Symbol.metadata')`.

Bun facts:
- Bun reads decorator settings only from the tsconfig in the current working directory.
- Bun ignores `experimentalDecorators` set in an extended tsconfig.
- The legacy suite therefore has its own `tests/bun/legacy/tsconfig.json`.

**Tests:**
- Runner: vitest, with shared config at `framework/vitest.shared.ts`. `vitest.config.ts` excludes React; `vitest.react.config.ts` uses happy-dom with `tests/react/setup.ts`.
- Suites: `tests/bun/core` (class-token, construct, lifetimes, regressions), `tests/bun/standard`, `tests/bun/legacy`, `tests/bun/static`, `tests/react`. About 105 cases in all.
- Benchmark: `scripts/bench.ts`, budget output only. The original `test:bun` omitted `tests/bun/server` and did not guarantee a Bun runtime. G00/G01
  now include server discovery and assert actual runtime; see the status/evidence table below.

**Other docs in the package:**
- `README.md`: the user guide, 120 lines.
- `DECISIONS.md`: design decisions. Read it before changing ownership, multi shadowing, the factory view, or `clearTokenCaches`.
- `FIX-PLAN.md`: an older defect plan. It is mostly implemented and partly stale; item 6.1 reconciles it.

## 3. Architecture: how a resolve works

You need this model before touching `cminject.container.ts`. Original line anchors below are historical; search
for symbols. G02/G03 corrections are noted explicitly; extraction and ambient changes remain pending.

- **Token → key.** G02 separates internal lookup/intern/retain/release from public `normalizeToken` pinning.
  Missing internal string reads do not intern entries; bindings and bounded diagnostics own retained keys. Token mapping:
  - a symbol stays as it is;
  - a registered string is interned in a process-global retained-key map; issued public keys remain pinned;
  - a class maps through a `WeakMap`;
  - a `DelayedConstructor` is unwrapped first.
- **Binding.** A registration is a `Binding`; its shape is `BindingShape`, `container.ts:58`. It holds:
  - `key`, `graphId`;
  - `factory(c, data)`, an optional `lifecycle`;
  - declared `deps`, plus `declared`, which is true only when deps were given or the registration is a class;
  - `ownerContainer`, `owner.ownership` (`owned`/`external`) and `kindTag`.
  `install()` (about line 538) puts it in the registry.
- **Record.** A built or building instance is a `StoredRecord` (`StoredShape`, line 82):
  - `state` is one of `creating`, `initializing`, `ready`, `disposing`, `disposed`, `failed`;
  - it also holds `data`, `binding` and `container`.
  Each container keeps `records: Map<Binding, StoredRecord>`, `pending: Map<Binding, Promise<StoredRecord>>`, and a
  `disposalOrder`.
- **Routing.** `route(binding)` (line 495) picks the container that holds the instance:
  - a singleton lives on the binding's owner;
  - tenant and request services live on the nearest tenant or request scope;
  - a scoped service lives on the resolving scope.
- **Resolver** (line 141) is the explicit context: `frame` (the innermost build), `build` (the build whose factory is
  running), `staged` replacements, and `suppressInterceptors`. `ROOT` (line 152) is the shared empty context.
- **Frame** (line 130) links `binding`, `target`, `record` and `parent`. Walking `parent` gives the resolution path.
- **Build** (line 125) has `waitsOn: Map<Build, count>`. A cross-chain cycle is a path through `waitsOn` edges back to
  the asker (`reaches()`, line 179).
- **`entry(binding, data, ctx)`** (line 940) is the heart of resolution. In order:
  1. A captive check. `lifespan()` (line 54) ranks Singleton 3, Tenant 2, Request 1, Scoped/Transient 0. A tenant or
     request binding resolved while a longer-lived frame is building throws `DiGraphError` `captive`.
  2. `route`.
  3. The `data`-mismatch check on a cached record.
  4. A same-chain cycle check through the frames.
  5. The scoped-under-singleton captive check.
  6. A ready record is returned. A pending one is awaited through `settled()`, which records a `waitsOn` edge, after
     checking `reaches()`.
  7. Otherwise it makes a record, creates a `ResolutionView` and calls `materialize()`.
  Synchronous builds return a record; asynchronous ones go through `share()`, which publishes them to `pending`.
- **Sync or async.** Every step returns a value or a promise. The helpers `then`, `each` and `gather` (lines 194–280)
  stay synchronous until a step returns a promise. That is why `resolveSync` and `get` work for synchronous graphs, and
  why `useService` can render without suspending.
- **`ResolutionView`** (line 1750) is the object a factory receives as `c`. Its `resolve`, `resolveSync` and `resolveAll`
  carry the current resolution. G04 replaced generated forwarding with an explicit `ContainerView implements
  IContainer` boundary and typed context subclasses, actual-target receivers/accessors and stable signals. One-way
  prototype attachment preserves `instanceof Container`; G05 source extraction and final coverage mapping are verified.
- **Class construction.** `registerConstructor`, `classFactory` (line 711), `construct` (line 721), then `slot` for each
  parameter and `complete`. G02 replaced the original module-level `completed` WeakSet with root-owned weak
  pending/success state, staged completion progress and explicit attempt-local hook contexts. See `DECISIONS.md`.
- **Ambient hook.** `useAmbientResolution` (line 297) sets a module-level source. Importing `cminject/server` calls it
  (`cminject.context.ts:22`), so a resolve made through a container that a factory captured joins that factory's
  resolution.
- **Observation.** `observe()` (line 844) logs binding-to-binding edges, typed `declared` or `dynamic`, and
  instance-to-instance resource edges. Each log is bounded at 10k entries. Snapshots, `getDependencyTree` and
  `issuesObserved()` read from them.
- **Lifecycle:**
  - `onInit` runs after the factory.
  - `initializeOwned()` builds a scope's own registrations, then runs `onReady` once per instance.
  - `dispose()` disposes owned records in reverse build order and throws an `AggregateError` on hook failures.
  - `clearInstances()` (line 1471) awaits pending builds, disposes built instances, and keeps `registerInstance` and
    external records.

## 4. Evaluation findings

The plan item that addresses each finding is in brackets.

### 4.1 Engineering weaknesses (verified by reading the code)

1. **Historical unbounded string-token registry** [1.1, resolved in G02]. The original process-global string map
   never freed entries. Internal non-interning lookup, balanced binding/staging/diagnostic retention and exact public
   raw-key pinning now preserve identity while collecting unowned keys. `clearTokenCaches` collects unpinned
   zero-reference entries; independent review approved the implementation. Publicly issued keys remain pinned by contract.
2. **`useService` retries failed builds forever** [1.2, implemented]. Original `cminject.react.ts:105-125`:
   - The render throws the `scope.resolve` promise.
   - When that promise rejects, `.finally` clears the in-flight entry and React re-renders.
   - The re-render starts a new build and suspends again.
   The error never reaches an error boundary. Fixed with retained per-scope failures and explicit `resetService`;
   regression coverage now includes sync/async failures, falsy rejections, StrictMode, pending resets and isolation.
3. **Historical global `completed` WeakSet** [1.3, resolved in G02]. Root-owned weak completion state now separates
   roots, joins pending work and supports retry without duplicate successful hooks or stacked wrappers. Explicit live
   hook contexts support reentry after `await`; captured bare-root async self-await remains an unsupported user cycle.
   Independent review approved this bounded contract; see `DECISIONS.md`.
4. **Historical missing build timeout** [1.4, resolved in G03]. Baseline factories could hang every waiter without
   a dependency cycle. Verified owner-routed factory-plus-initialization deadlines now reject all callers, and disposal
   cancels owned subtree builds before awaiting teardown. Late unreturned owned results are cleaned when user work
   settles; arbitrary JavaScript is not forcibly stopped. See the G03 evidence and `DECISIONS.md`.
5. **Historical type leaks** [1.5, resolved in G04]. Constructor tokens now use `never[]` while preserving
   required-argument classes; decorators have placement-specific overloads; production source has no explicit `any`
   syntax or double assertions. Typed operational binding storage and explicit context facades remove fake handle/
   container assertions. The legacy two-argument property-decorator checker limit remains documented, not hidden.
6. **Historical god file** [2.1, source extraction verified in G05]. `cminject.container.ts` is now 861 normally
   formatted lines after eight cohesive extractions. Source/behavior, both independent reviews and final measured
   responsibility-level coverage-floor mapping pass; original floors were not lowered.
7. **Historical implicit container forwarding** [2.2, in-place boundary implemented in G04]. Explicit typed
   `IContainer` views now preserve actual receivers, accessors, context, stable signals and `instanceof Container`
   without generated forwarding. `identity.id` remains the logical identity test. G05 extracted the explicit view
   and verified its runtime/type boundary and final extraction coverage mapping; later Phase 2 work remains pending.
8. **Ambient hook installed by import** [2.3]. Importing `./server` changes core's behaviour for the whole process.
9. **Packaging** [2.4, 3.1]:
   - `exports` point at `.ts` source, so plain Node cannot load the package;
   - `tsconfig.build.json` emits a `dist` that nothing references;
   - the `"./*"` wildcard exposes every internal file, and it exists only because core imports itself through
     `@coldmind/inject/src/...`.

### 4.2 Feature gaps (ranked by adoption impact)

1. **No Node runtime** [3.x]. It is a README TODO. Source exports, `enum`s and decorators rule out Node's type
   stripping, so a `dist` build is required.
2. **No modules** [4.1]. There is no unit that groups registrations, loads them once, or unloads them.
3. **No test-override API** [4.2]. `replace` with staged rollback exists, but there is no snapshot, restore or override
   helper.
4. **No build timeout or cancellation** [1.4].
5. **Static validation sees only declared `deps`** [4.5]. Factory registrations without `deps` are opaque to
   `graph().issues()` and `assertSound()`, and they are the common case.
6. **No compile-time dependency typing** [4.4, 4.8]. Typed tokens type the result of `resolve`, but not whether the
   graph is complete.
7. **No lazy or provider injection** [4.3]. `delay()` only breaks import cycles.
8. **No named or conditional bindings** [4.6, 4.7].
9. **Documentation completion gap** [5.x]. Thirteen guide/migration drafts now exist; final API reconciliation,
   TypeDoc and executable example verification remain pending. Draft presence is not a completed docs deliverable.
10. **ctsc distribution integration remains pending** [4.9, separate package]. Compiler/split preparation and
    independent review are complete, but a real inject-distribution fixture must run after G07.

### 4.3 Measured test coverage

These counts are grep line hits in `tests/`, not test cases. They show the shape of coverage, not its quality.

| Feature | Refs | Feature | Refs |
|---|---|---|---|
| dispose | 85 | `DiRuntime` | 4 |
| cycle | 32 | `acquireRequest` | 1 |
| registerInstance | 16 | `onInstance` | 1 |
| Multi | 15 | interceptors | 3 |
| captive | 14 | `healthCheck` | 3 |
| injectMembers | 13 | `getResourceGraphSnapshot` | 3 |
| useService | 11 | `hotReplace` | **0** |
| Tenant | 11 | `inspect()` | **0** |
| resolveAll | 10 | `getDependencyGraph` | **0** |

Test effort runs opposite to differentiation. The features that set cminject apart (`DiRuntime`, hot rebuild,
introspection) are the least tested. Phase 0 fixes that before anything moves.

### 4.4 Competitive position

The competitor cells come from training knowledge as of mid-2026 and were not re-checked; verify any cell before
quoting it externally.

✓ full · ~ partial · ✗ absent · – not applicable

| # | Capability | cminject | Inversify | tsyringe | NestJS | Awilix | Angular | Effect |
|---|---|---|---|---|---|---|---|---|
| 1 | Async factories | ✓ | ✓ | ✗ | ✓ | ✗ | ✗ | ✓ |
| 2 | Sync and async resolve on one container | ✓ | ✓ | ✗ | ~ | ✗ | ✗ | ~ |
| 3 | Request lifetime | ✓ | ~ | ~ | ✓ | ~ | ✗ | ~ |
| 4 | Tenant lifetime | ✓ | ✗ | ✗ | ~ (durable) | ✗ | ✗ | ✗ |
| 5 | Captive-lifetime detection | ✓ | ✗ | ✗ | ~ (bubbles) | ✓ (strict) | ✗ | ✓ |
| 6 | Cycle detection across `await` | ✓ | ~ | – | ~ | – | – | – |
| 7 | Static graph validation | ✓ | ✗ | ✗ | ~ | ✗ | ✗ | ✓ |
| 8 | Compile-time dependency typing | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |
| 9 | Ordered init and dispose | ✓ | ✓ | ~ | ✓ | ✓ | ✓ | ✓ |
| 10 | Health checks | ✓ | ✗ | ✗ | ~ (terminus) | ✗ | ✗ | ✗ |
| 11 | Per-call data or assisted injection | ✓ | ~ | ✗ | ✗ | ~ | ✗ | ✗ |
| 12 | Hot replace with dependent rebuild | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 13 | Interceptors or AOP | ✓ | ✓ | ✓ | ~ | ✗ | ✗ | ✗ |
| 14 | Graph introspection | ✓ | ~ | ✗ | ~ (devtools) | ✗ | ✗ | ✗ |
| 15 | Named, tagged or conditional bindings | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 16 | Modules or composition units | ✗ | ✓ | ✗ | ✓ | ✓ | ✓ | ✓ |
| 17 | Lazy or provider injection | ~ | ✓ | ~ | ~ | ✓ | ✓ | ✓ |
| 18 | Test overrides, snapshot and restore | ~ | ✓ | ~ | ✓ | ~ | ✓ | ✓ |
| 19 | TC39 standard decorators | ✓ | ✗ | ✗ | ✗ | – | – | – |
| 20 | Decorator-free classes | ✓ | ~ | ~ | ✗ | ✓ | ✓ | ✓ |
| 21 | React Suspense integration | ✓ | ~ | ~ | ✗ | ✗ | ✗ | ✗ |
| 22 | Browser-safe core | ✓ | ✓ | ✓ | ✗ | ✓ | ✓ | ✓ |
| 23 | Node runtime | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 24 | Docs and ecosystem | ✗ | ✓ | ~ | ✓ | ✓ | ✓ | ✓ |
| | **✓ / ~ / ✗** | **17/2/5** | 11/6/7 | 3/7/13 | 7/9/8 | 8/3/11 | 8/0/14 | 12/2/8 |

The row set is biased toward cminject, because most rows come from its own feature list. Rows 16, 18, 23 and 24 decide
adoption, and cminject loses all four. After this plan, every ✗ and ~ in the cminject column is ✓ except row 24's
ecosystem half, which the docs work starts but cannot finish.

**Where cminject leads, and must keep leading:**
- cycle detection between factories that `await` before resolving;
- tenant and request lifetimes with captive errors, where Nest widens scopes silently;
- a sync fast path on an async container;
- all three decorator dialects;
- built-in health checks, hot rebuild of dependents, and resource and ownership graphs.

Every plan item must keep these behaviours. The Phase 0 suites pin them.

## 5. The plan

Sizes: S under 1 day, M 1–3 days, L over 3 days. Each item lists the change, files, API, tests and done condition.

### Phase 0: baseline and safety net

**0.1 Coverage baseline** (S)
- Add `@vitest/coverage-v8` and a `test:coverage` script.
- Set per-file thresholds in `vitest.config.ts` from the first run. Ratchet them up only.
- Done when: the baseline numbers are recorded in `tests/FEATURES.md`.

**0.2 Feature matrix as a test index** (S)
- `tests/FEATURES.md` gets one row per feature (the section 4.3 list plus the `IContainer` surface), with columns: test
  file, case count, status.
- Done when: every row names at least one test file.

**0.3 Characterization tests for thin areas** (M). These lock in current behaviour before Phase 2 moves code.
- `tests/bun/server/runtime.test.ts`:
  - `start`;
  - `withTenant`, with `configureTenant` running once per tenant;
  - `acquireRequest` with `release` called twice;
  - `eagerRequest`;
  - `drainTenant` with live leases;
  - `drain`, then `dispose`;
  - `onError` phases.
- `tests/bun/core/hot.test.ts`: `hotReplace` rebuilds dependents and disposes old instances.
- `tests/bun/core/inspect.test.ts`:
  - the `inspect()` snapshot shape;
  - `getDependencyGraph` and `getDependencyTree`;
  - ownership records;
  - epoch increments on every mutation.
- `tests/bun/core/interceptors.test.ts`:
  - order across scopes;
  - `path` and `requester`;
  - an error from a hook is attached to the resolution error;
  - the suppressed flag.
- `tests/bun/core/health.test.ts`: `onHealthCheck` from the lifecycle versus from the instance, and `healthCheckAll`
  across scopes.
- `tests/bun/core/discovery.test.ts`: `onInstance` unsubscribe, and `discover` by tag and by method decorator.
- Include `tests/bun/server` in `test:bun`; invoke Vitest through `bun --bun` (including the legacy script) and
  assert the Bun runtime in that test leg. Preserve the legacy suite's cwd-specific tsconfig handling.
- Done when: each feature named above has at least 5 cases, and the Bun script discovers the server cases under Bun.

**0.4 Comparative benchmark** (M)
- `scripts/bench.compare.ts`, with `inversify`, `tsyringe` and `awilix` as devDependencies.
- Scenarios, run on each library where it applies:
  - cached singleton `get`;
  - cold build of a 100-node graph;
  - async chain of depth 10;
  - scope create and dispose ×10k;
  - 1,000 request scopes with 5 services each.
- Output: a markdown table to stdout plus `bench/results.json`.
- Keep this comparative work nonblocking for Phase 1 correctness fixes; competitor dependencies, budget baselines
  and timing variability are not correctness gates. Record unsupported scenarios rather than comparing unlike work.
- `scripts/bench.ts` may exit non-zero on an explicitly established budget breach; comparative results are
  informational. CI wiring is repo-level work outside this package.

### Phase 1: correctness fixes (no layout change)

**1.1 Bounded string-token registry** (M) — `cminject.token.ts`, `cminject.container.ts`
- Inventory every retention path before adding refcounts: `install`, the plain-factory `register` fast path that
  bypasses it, replacements/rollback, declared dependency keys and `Binding.keyOf` cached dynamic dependency keys.
  Each live binding retains its token and dependency keys until its final release; `unregister` and container
  `dispose` release those references. Drop only unpinned keys with no remaining binding/cache references.
- Preserve key identity when A caches a dependency key for B, B is unregistered, and B is registered again. Failed
  registration and replacement rollback must neither leak references nor release another live binding's key.
- Resolving a string that has no registration uses a lookup that does not intern it.
- `getGlobalToken(name)` pins its key permanently (documented).
- `clearTokenCaches` releases unpinned keys that have a zero count.
- Tests: register and unregister 10,000 `tenant:${i}` tokens across 100 tenant scopes; the internal size returns to
  the pinned count. Cover both registration paths, shared keys across roots, declared and dynamic dependency-key
  caches, unregister/re-register resolution through an existing dependent, failed registration and replacement rollback.
- Done when: `getTokenCacheSize()` reports live keys.

**1.2 `useService` surfaces failures** (S) — `cminject.react.ts`
- Use discriminated pending/failed entries, not error truthiness: rejection reasons can be `undefined`, `null`, `0`
  or an empty string. Check for a retained failure before `resolveSync`, otherwise a re-render starts another build
  before seeing the error. Preserve synchronous success without suspension.
- Keep failures per scope/token until a new export `resetService(scope, token)` clears them; a different scope has
  its own state. Remove successful entries with an identity guard.
- A reset during a pending build must not create a competing build or let an old settlement clear/overwrite a newer
  entry. Define reset as clearing settled failures while preserving the existing pending entry for deduplication.
- Tests:
  - a rejecting factory runs once and the error boundary catches the error;
  - after a reset, a retry succeeds;
  - under StrictMode the factory still runs once;
  - synchronous throws and falsy async rejection reasons reach the boundary without another factory attempt;
  - reset while pending preserves the shared attempt, and scope/token failures stay isolated.

**1.3 Per-tree completion state** (M) — `cminject.container.ts:262`
- Replace the module-level `WeakSet` with root-owned weak completion state shared by children: pending work and
  successful completion are distinct. Mark success only after members, wraps and hooks finish; concurrent callers
  join pending work and a failure clears pending state so an explicit retry can run. Preserve the synchronous path.
- Define partial-failure retry semantics so previously applied wraps are not stacked or hooks accidentally marked
  complete; retain per-stage progress or restore mutated state as needed, using the smallest tested approach.
- Tests: two roots inject the same instance independently; same-tree success is a no-op; concurrent async injections
  await one completion; member/hook failure rejects every waiter and retry succeeds without double wrapping.

**1.4 Build timeout and cancellation** (M) — `cminject.container.ts`, `cminject.contracts.ts`
- `container.buildTimeout?: number`, inherited by children, off by default. `RegisterOptions.timeoutMs` overrides it
  per registration.
- Apply the deadline to the complete asynchronous materialization, including `onInit`, the first caller, shared
  waiters and transients; putting a race only inside shared-cache `share()` is insufficient. Keep synchronous
  factories synchronous. Clear timers on every terminal path and call `unref?.()` when supported.
- On expiry, all affected callers reject with `DiError` `DI_BUILD_TIMEOUT`. The message lists pending builds, their
  waits-for edges and the resolution path. Track transient work as well as cached pending builds.
- Factory context: `c.signal` is an `AbortSignal` that aborts on timeout and owning-container dispose. Abort active
  builds before `dispose()` waits on pending work, so a cooperative factory can settle instead of deadlocking.
  Define deterministic disposal cancellation for an uncooperative factory without waiting forever.
- Terminal-state guards prevent timeout/dispose races and late `onInit` completion from publishing ready records or
  resurrecting disposed containers. Dispose a late owned instance exactly once, never an external instance; consume
  late rejections and clean pending/waits-for state on every terminal path.
- Tests:
  - a factory that never settles times out;
  - two factories that each wait on an external promise report both pending builds;
  - the signal aborts on timeout and on `dispose()`;
  - the first caller and shared waiters reject together; transient factories and never-settling `onInit` time out;
  - dispose aborts before waiting, including an abort-aware and an uncooperative build;
  - late factory/`onInit` completion is disposed once, late rejection is handled, and no record is cached after termination.

**1.5 Type tightening** (M) — `cminject.token.ts`, `cminject.contracts.ts`, `cminject.container.ts`
- `Constructor<T> = new (...args: never[]) => T`, and the same change for `AbstractConstructor`.
- `UniversalDecorator` becomes overloads per placement, legacy and standard.
- Remove every public-path `as unknown as`. Internal casts that remain each get a one-line reason.
- Tests: `tests/types/*.test-d.ts` with `expectTypeOf` through `vitest typecheck`, covering token inference, factory
  `data` typing, decorator placement errors, and `resolveAll` element type.

### Phase 2: structure

**2.1 Split `cminject.container.ts`** (M). Establish an explicit internal boundary, then extract one responsibility
per step with characterization tests unchanged. This is not a pure file move: views, records, graph access and
observation currently depend on private container members. Inventory those dependencies and define minimal typed
internal host operations before extracting; do not replace private access with bracket casts or expose public internals.
- `src/core/cminject.async.ts`: `then`, `each`, `gather`, `settle`, `syncValue`, `shield`, `guarded`.
- `src/core/cminject.binding.ts`: `Binding`, `StoredRecord`, `Handle`.
- `src/core/cminject.view.ts`: `ResolutionView`.
- `src/core/cminject.observe.ts`: edge and resource observation, snapshots, `OBSERVATION_LIMIT`.
- `cminject.container.ts`: `Container` only, with a target under 900 lines.
- Done when: there are no runtime import cycles or new private-member escape casts. The view depends on the typed
  internal host boundary and contracts, plus `Container` as a type if needed. Add explicit internal package subpaths
  for extracted files as part of each approved extraction; keep package-only imports throughout.

**2.2 `ResolutionView` as a real `IContainer`** (M) — `cminject.view.ts`
- Replace `installViewForwarding` with explicit `implements IContainer` methods, so a missing member is a compile error.
  Archive the old code.
- Preserve the existing `instanceof Container` result through a deliberate prototype arrangement while using the
  typed host boundary. Do not add `Symbol.hasInstance` merely to fix an instance check that already works.
- Keep host unwrapping explicit and limited to genuine internal views, not any object with a `target` property.
- Update the `DECISIONS.md` section "Factory parameter is a resolution-bound view".
- Tests: a compile-time interface conformance check plus an explicit runtime member inventory (interfaces have no
  runtime keys); forwarding uses the host receiver, resolution carries context, `instanceof Container` remains true,
  and `identity.id` is equal. Cover getters, overloads and name collisions.

**2.3 Ambient resolution per root** (S) — `cminject.container.ts`, `cminject.context.ts`, `cminject.runtime.ts`
- Remove the module-level `ambient` and `useAmbientResolution`.
- Add `root.useAmbient(source)`, which throws on a child scope. Children read it from their root.
- The `DiRuntime` constructor calls `root.useAmbient(...)` with its own `AsyncLocalStorage`. Importing `./server`
  installs nothing.
- Remove `cminject.context.ts` from `sideEffects`.
- Test: two roots in one process; only the one under `DiRuntime` detects a cycle made through a captured container.

**2.4 Explicit exports and self-imports** (M) — `package.json`, source, tests, scripts and configs
- Inventory all package self-imports and re-exports, including dynamic type imports, server-to-server imports, test
  imports and the files extracted by 2.1. Keep the package-only import rule; do not substitute relative paths.
- Add public subpaths `./contracts`, `./token`, `./errors`, `./graph`, `./decorators`, `./container` and explicit internal
  subpaths for `./internal/async`, `./internal/binding`, `./internal/view`, `./internal/observe` and the typed host
  boundary if it needs a file. Server imports need `./internal/server/context` and `./internal/server/runtime`.
  Internal exports are implementation-only, not new supported public APIs; server entries stay server-only.
- Rewrite every inventoried `/src/` self-import to its approved explicit subpath. Remove `"./*"` only after every
  importer resolves; cover all explicit entries in the Phase 3 condition map.
- Tests: browser bundles still exclude server code; a package-wide import check rejects `/src/` self-imports and
  relative source imports, and entry-resolution checks cover source, server, tests and built conditional exports.

### Phase 3: Node runtime

**3.1 Dual build with conditional exports** (M) — `package.json`, `tsconfig.build.json`
- Every subpath gets conditions: `"bun"` → `./src/....ts`, `"types"` → `./dist/....d.ts`, `"default"` →
  `./dist/....js`.
- `tsconfig.build.json`: `rootDir: src`, `outDir: dist`, `rewriteRelativeImportExtensions`, `verbatimModuleSyntax`.
  Self-imports resolve through the conditions, so `dist` needs no rewriting.
- `files` adds `dist`.
- Done when: `node -e "import('@coldmind/inject').then(m => m.Container)"` runs with no transpiler.

**3.2 `@coldmind/decorate` dual build** (S). This is a separate package: `packages/foundation/decorate`. It gets the same
condition scheme, because inject's Node build imports it.

**3.3 Node test leg** (M)
- `tests/node/*.test.mjs` run under `node --test` against `dist`:
  - core resolve and lifetimes;
  - the decorator-free mode;
  - a `DiRuntime` tenant and request round trip;
  - `AsyncLocalStorage` cycle detection.
- The existing vitest suites also run with the Node pool as a `test:node` script.
- `engines.node` stays `>=22`.
- Update README: Node becomes supported and the TODO section goes. Update the package `CLAUDE.md` first line to match.

### Phase 4: features

**4.1 Modules** (L) — new `src/core/cminject.module.ts`
- API:
  - `defineModule({ name, imports?: Module[], register(c): void | Promise<void> })`;
  - `container.load(...modules): Promise<ModuleHandle[]>`;
  - `handle.unload(): Promise<void>`.
- A module loads once per container tree, by identity. Imports load depth-first, and an import cycle is a
  `DiGraphError` of kind `module-cycle`.
- Capture every registration path, including plain-factory `register` that bypasses `install`. Use an explicit
  module registration facade/transaction whose capture survives `await` without ambient state in core. Unrelated
  registrations during async module loading must not be attributed to the module.
- Failed async load rolls back only that transaction's registrations and owned instances. Concurrent loads of the
  same identity join one pending load; a failed load can be retried. Define/reject conflicting mutations rather than
  silently rolling back unrelated work.
- Track shared imported-module ownership: unloading A must not unload an import still required by B or an explicit
  load. Unload in dependency-safe order, removing only owned bindings and disposing owned instances.
- `BindingDescriptor.module?: string`, shown in graph, inspect and validation messages.
- Tests:
  - load is idempotent;
  - import order;
  - unload removes only that module's bindings;
  - a module cycle is reported;
  - async `register`, fast-path registrations, concurrent loads and unrelated interleaved registrations;
  - failed async load rolls back and retry succeeds;
  - shared imports survive until their last dependent/explicit owner unloads;
  - module names appear in `issues()`.

**4.2 Test overrides** (M) — new entry `src/cminject.testing.ts` (`./testing`)
- `container.snapshot(): RegistrySnapshot` and `container.restore(snapshot): Promise<void>`. Restore disposes owned
  instances built after the snapshot and reinstates registrations and epochs.
- `override(container, token, valueOrFactory): Promise<OverrideHandle>` awaits the existing async
  `replace(..., { retainPrevious: false })` transaction before exposing the handle. `handle.restore(): Promise<void>`
  restores the prior registration definition, not a disposed instance; rebuild it through normal lifecycle rules.
  Failed replacement preserves the original and returns no handle; nested overrides restore in defined LIFO order.
- `createTestContainer(...modules)`: a root with `buildTimeout: 2000` and observation on.
- Tests:
  - snapshot and restore across singleton, scoped and multi registrations;
  - a nested override;
  - restore disposes instances built inside the test;
  - interaction with modules.

**4.3 Lazy and provider injection** (M) — new `src/core/cminject.lazy.ts`
- Slot markers `lazy(token)` and `provider(token)`. They are accepted in `deps`, `$inject`, `$members`, `@Inject(...)`
  and lifecycle declared deps.
- `Lazy<T>` has `get(): Promise<T>`, `getSync(): T` and `peek(): T | undefined`. A `Provider<T>` is
  `() => Promise<T>` and builds per call for transients.
- Both are bound to the resolving scope, not the root.
- Graph: lazy edges are flagged `lazy: true`, so they are excluded from cycle detection but still checked for captive
  lifetimes.
- Tests:
  - an A↔B cycle broken by `lazy` resolves;
  - a captive lazy edge is still reported;
  - a provider builds a new transient each call;
  - `getSync` throws on an async build.

**4.4 Typed dependency factories** (M) — `cminject.contracts.ts`, `cminject.container.ts`
- New overload: `singleton(token, { inject: [A, B] as const, factory: (a, b) => ... })`. Parameter types come from
  the token tuple, and `lazy()`/`provider()` markers map to `Lazy<T>`/`Provider<T>`.
- `inject` doubles as declared `deps`, which feeds 4.5.
- The same overload applies to every lifetime method and `*Multi`.
- Type tests: a wrong parameter type and a wrong tuple length are both compile errors.

**4.5 Static graph completeness** (M) — `cminject.graph.ts`, `cminject.container.ts`
- New `DiIssueKind` values:
  - `undeclared`: a factory binding with no `deps` or `inject`;
  - `undeclared-edge`: an observed edge missing from the declared deps.
- `validate({ requireDeps?: boolean, includeObserved?: boolean })`. `includeObserved` merges the edges `observe`
  already logs into the static graph.
- `inspect()` reports declared-versus-observed drift.
- Tests:
  - a factory without deps is reported only when `requireDeps` is set;
  - an observed edge missing from deps is reported;
  - `includeObserved` finds a cycle through an undeclared edge.

**4.6 Qualified tokens** (S) — `cminject.token.ts`
- `qualify<T>(token: InjectionToken<T>, name: string): Token<T>` returns an interned derived token keyed by
  `(token, name)`. `resolve`, the decorators and `deps` accept it unchanged.
- `tokenName` renders it as `Db#replica`.
- Tests: two qualified Db bindings; `@Inject(qualify(Db, 'replica'))`; the graph shows both.

**4.7 Conditional bindings** (M) — `cminject.container.ts`, `cminject.graph.ts`
- `RegisterOptions.when?: (ctx: { requester?: ResolutionPathStep; path; scope: ScopeIdentity }) => boolean`.
- Several single registrations of one token are allowed when all but at most one have `when`. Resolution picks the
  first matching registration in registration order; the one without `when` is the fallback.
- The static graph reports `ambiguous` when two unconditional singles collide.
- Tests:
  - an inject-into-parent selection;
  - the fallback;
  - no match and no fallback gives `NotRegisteredError` naming the conditions;
  - the ambiguity issue.

**4.8 Compile-time module completeness** (L) — `cminject.module.ts`
- `defineModule<Provides, Requires>()` carries phantom types.
- `composeModules(...modules)` typechecks only when the union of `Requires` is covered by the union of `Provides`.
  Otherwise the type error names the missing tokens.
- Runtime behaviour is unchanged. This is an opt-in typed layer over 4.1.
- Type tests: a missing provider is a compile error naming the token; a complete composition infers the provided map.

**4.9 ctsc parameter-type pass** (FIX-PLAN §2). This is a separate package, `packages/tools/ctsc`. It is tracked here
and planned and built as its own task.

### Phase 5: documentation

**5.1 Docs set** (L) — new `docs/`
- `getting-started.md`, `lifetimes.md` (tenant, request and captive rules), `async-and-cycles.md`, `modules.md`,
  `testing.md`, `decorators.md` (three dialects), `server.md`, `react.md`, `introspection.md`, `errors.md` (every
  error `code`).
- Migration guides with mapping tables: `migrate-inversify.md`, `migrate-tsyringe.md`, `migrate-nest.md`.
- A TypeDoc API reference in `docs/api/`, built by a `docs:api` script.

**5.2 Executable examples** (S)
- `examples/*.ts`, one per doc page.
- `tests/bun/docs/examples.test.ts` imports and runs each example, so a docs drift fails the tests.
- README shrinks to a pitch, install, one example, the entries table and links.

### Phase 6: reconcile records

Update `DECISIONS.md` alongside each behavior change, especially token identity (1.1), completion/retry (1.3), timeout
and cancellation (1.4), the view boundary (2.2), ambient ownership (2.3), module ownership (4.1), lazy edges (4.3) and
conditional fallback (4.7). Phase 6 verifies these records; it is not permission to defer them.

**6.1** (S)
- Update `FIX-PLAN.md`: mark §1, §6.1, §6.2, §6.5, §6.6 and §6.8 done; point §6.4 to 1.4; point the §2 ctsc pass to
  4.9; remove the stale defect "classFactory is async whenever a class injects anything", which the current
  `construct`/`gather` path no longer has.
- Verify the contemporaneous `DECISIONS.md` entries for:
  - per-root ambient (2.3);
  - token refcounting and pinning (1.1);
  - timeout semantics (1.4);
  - the lazy-edge cycle rule (4.3);
  - the conditional-binding fallback (4.7).
- Update the package `CLAUDE.md` for Node support and the new layout.
- Mark finished items in this file.

## 6. Order, approvals, commands

### Order

| Step | Items | Depends on |
|---|---|---|
| 1 | 0.1–0.3; 0.4 is an independent, nonblocking benchmark lane | none |
| 2 | 1.1–1.5 | relevant 0.3 characterization tests, not 0.4 |
| 3 | 2.1 → 2.2 → 2.3, then 2.4 | 1.x (fixes land before the files move) |
| 4 | 3.1–3.3 | 2.4, plus 3.2 in `decorate` |
| 5 | 4.1 → 4.2; 4.3 → 4.4 → 4.5; 4.6 → 4.7; then 4.8 | 2.x. 4.2 uses 4.1. 4.5 uses 4.4. 4.8 uses 4.1 and 4.4 |
| 6 | 5.1–5.2 | 4.x (the docs describe the final API) |
| 7 | 6.1 | everything |

Sizes are planning estimates, not execution or approval gates.

### File inventory and approval history

- Original approved `src/core/` inventory: `cminject.async.ts`, `cminject.binding.ts`, `cminject.view.ts`,
  `cminject.observe.ts`, `cminject.module.ts`, `cminject.lazy.ts`.
- G05 expanded design inventory approved in [architect preflight](.omx/evidence/prio-one/g05-design/architect-inventory.md):
  `cminject.construction.ts` (construction/completion), `cminject.resolution.ts` (resolution/build control),
  `cminject.mutation.ts` (shared mutation transactions), and type-only `cminject.host.ts` (narrow capability facets).
  All eight G05 extraction/host files were source-complete and independently reviewed at that checkpoint. Container was861 lines before the feature pass; the final file is1041 lines;
  The earlier extraction counts are historical; final source and runtime import checks are in the delivery receipt.
  Final measured responsibility-level coverage-floor mapping has passed: all original floors unchanged, with the
  same pre-extraction Container floors inherited by all seven runtime helpers and 100% for the type-only host.
- `src/cminject.testing.ts` as the new `./testing` entry.
- New subpath exports: `./contracts`, `./token`, `./errors`, `./graph`, `./decorators`, `./container`, `./testing`;
  implementation-only `./internal/async`, `./internal/binding`, `./internal/view`, `./internal/observe`,
  `./internal/server/context`, `./internal/server/runtime`. G05 design approval additionally covers
  `./internal/construction`, `./internal/resolution`, `./internal/mutation`, `./internal/host`; these are implementation-only,
  not new public APIs. All eight G05 implementation routes are added and resolution-checked. Full package self-import
  migration, wildcard removal and all explicit export integration are now implemented and verified.
  Inventory and approve any further internal entries before creating them. The `"./*"` export is
  replaced only after all self-imports resolve.
- `dist/` becomes a published output.
- `tests/`: `bun/server/`, `node/`, `types/`, `bun/docs/`, the new core suites, and `FEATURES.md`.
- `scripts/bench.compare.ts`, `bench/results.json`, `docs/`.
- New devDependencies: `@vitest/coverage-v8`, `inversify`, `tsyringe`, `awilix`, `typedoc`.

Nothing is removed. Replaced code goes to `.archive/<date>/`.

### Status — final solo delivery

The user-specified write-all → typecheck → fix → direct critical review → tests sequence supersedes the old per-phase execution gates. Requirements below/above remain the original scope. [Full final evidence](.omx/evidence/prio-one/solo-final/README.md).

| Item | Final status/evidence |
|---|---|
| 0.1 | Implemented and verified: Coverage tooling, preserved original floors and new module/lazy/testing floors. vitest.config.ts; current-coverage/; solo-final/coverage.log. |
| 0.2 | Implemented and verified: Final feature/API index and executed counts. tests/FEATURES.md; solo-final/{bun,node}-results.json. |
| 0.3 | Implemented and verified: Six characterization areas retained and passing. tests/bun/{core,server}; runtime/hot/inspect/interceptor/health/discovery suites. |
| 0.4 | Implemented and verified: Comparative benchmarks and tested explicit budget. scripts/bench*.ts; bench/results.json; solo-final/bench-*.log. |
| 1.1 | Implemented and verified: Token retention/reclamation and stable exposed/qualified identities. core/token-cache.test.ts; qualification.test.ts; cminject.token.ts. |
| 1.2 | Implemented and verified: React failures, retries, scope isolation and Suspense. tests/react/adapter.test.ts; solo-final/react-{bun,node}.log. |
| 1.3 | Implemented and verified: Root completion, retry/provenance and explicit context. cminject.construction.ts; core/completion.test.ts. |
| 1.4 | Implemented and verified: Deadlines, cancellation, stable signals and late cleanup. cminject.resolution.ts; core/build-timeout.test.ts. |
| 1.5 | Implemented and verified: Public generic/decorator contracts and negative compiler fixtures. tests/types; solo-final/{types-*,compiler-fixtures}.log. |
| 2.1 | Implemented and verified: Cohesive typed extraction; no runtime cycles/private escapes. core/{async,binding,view,construction,resolution,mutation,observe,host}; solo-final/dag.log. |
| 2.2 | Implemented and verified: Real explicit typed views and contextual/target receiver behavior. cminject.view.ts; core/view.test.ts; delivery.test.ts. |
| 2.3 | Implemented and verified: Per-root ambient attachments and neutral server import. server/ambient.test.ts; server/runtime.test.ts; cminject.context.ts. |
| 2.4 | Implemented and verified: Explicit exports, migrated imports and entry resolution. package.json; scripts/check-{imports,core-imports,browser}.ts. |
| 3.1 | Implemented and verified: Clean compiled inject distribution and declarations. tsconfig.build.json; solo-final/{clean-inject,types-dist-consumer,node22-dist,node24-dist}.log. |
| 3.2 | Implemented and verified: Clean decorate dependency build and dialect preservation. ../decorate; solo-final/{clean-decorate,decorate-bun,decorate-node}.log. |
| 3.3 | Implemented and verified: Direct Node minimum/current runtime test leg. tests/node/*.test.mjs; solo-final/node{22,24}-dist.log. |
| 4.1 | Implemented and verified: Root-owned module loading, transactions, import references and unload. cminject.module.ts; RegistrationView; delivery/module-loader suites. |
| 4.2 | Implemented and verified: Snapshots, safe restoration, LIFO overrides and test roots. cminject.testing.ts; Container snapshot/restore; delivery.test.ts; node/features.test.mjs. |
| 4.3 | Implemented and verified: Lazy/provider slots across factories/constructors/members/decorators. cminject.lazy.ts; construction/decorators; lazy-leaf/deferred/delivery suites. |
| 4.4 | Implemented and verified: Exact-arity typed factory tuples for all ten registration variants. contracts/container/view/mutation; tests/types/typed-factory*.test-d.ts. |
| 4.5 | Implemented and verified: Opt-in completeness, current-generation drift and observed cycles. graph/observe/inspect; delivery observed-only-cycle regression. |
| 4.6 | Implemented and verified: Interned qualified tokens and readable graph/injection names. cminject.token.ts; qualification.test.ts; node/features.test.mjs. |
| 4.7 | Implemented and verified: Ordered conditional candidates, fallback and conservative static validation. binding/mutation/resolution/graph; delivery condition/captive/ambiguity regressions. |
| 4.8 | Implemented and verified: Named module capability completeness and negative type checks. cminject.module.ts; tests/types/module-types.test-d.ts. |
| 4.9 | Implemented and verified: Native compiler metadata pass and split client/server integration. ../../tools/{ctsc,split}; solo-final/{ctsc,split,compiler-real}.log. |
| 5.1 | Implemented and verified: Thirteen guides/migrations and generated API reference. docs/*.md; docs/api; solo-final/typedoc.log. |
| 5.2 | Implemented and verified: Thirteen executed examples and concise README. examples/cminject.*.ts; tests/bun/docs/examples.test.ts. |
| 6.1 | Implemented and verified: Records reconciled with the actual final implementation/results. PRIO_ONE.md; DECISIONS.md; FIX-PLAN.md; CLAUDE.md; tests/FEATURES.md. |

### Historical verification command reference (execution order superseded)

```bash
cd packages/foundation/inject
# Functional scripts invoke Vitest through Bun and assert the actual worker runtime; include server cases.
bun run test:bun && bun run test:bun:legacy && bun run test:react && bun run typecheck
CMINJECT_COVERAGE=1 CMINJECT_EXPECT_RUNTIME=node node node_modules/vitest/vitest.mjs run --config vitest.config.ts --coverage # Node/V8
bun scripts/bench.ts           # budget enforcement only when explicitly enabled/configured
bun scripts/bench.compare.ts   # informational, nonblocking; Phase 0 and after Phase 4
bun run build && node --test tests/node   # from Phase 3
bun scripts/check-browser.ts   # after 2.4 and 3.1
```
