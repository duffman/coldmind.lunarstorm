# G03 prepared build-control regressions

Status: **PREPARED ONLY; runtime/typecheck/build UNVERIFIED.** G02 is still owned by the core writer. No active suite, source, configuration, manifest, goal, ledger, or git operation was changed. Do not integrate or execute until G02 passes and the G03 owner introduces the contracts.

## Inputs and intended move

Read PRIO_ONE.md §1.4, test-spec-prio-one.md §1.4 and async testing guidance, prd-prio-one.md architecture B, current contracts/container/errors, and existing completion/promises/hot tests. Move `.omx/prepared-tests/build-timeout.test.ts` to `tests/bun/core/build-timeout.test.ts` only when G03 starts. Imports use public `@coldmind/inject` exports and Vitest, with no private APIs or compatibility casts. There are 24 focused cases.

## Coverage

- First/shared singleton waiters and independent transient deadlines; full onInit deadline and its signal.
- Scoped inheritance, owning-child override, registration override, default off, explicit child/registration off, immediate async zero; negative/NaN/infinite settings.
- Factory signal timeout; child owner abort before disposal waits; ignoring factory cancellation without any configured deadline; DI_BUILD_CANCELLED distinct from DI_BUILD_TIMEOUT.
- Late owned factory result exactly-once disposal; immediate retry protected from old success; late onInit not cached; late disposal-cancelled factory result; cancelled versus returned transient ownership.
- External ownership uses existing registerInstance(..., { ownership: 'external' }) and asynchronous instance onInit; no fictional registration ownership flag.
- Diagnostics snapshot includes A waiting for independently started B, pending transient, path and waits-for edge. Only A has a timer: B cannot disappear due to an earlier timer before snapshot capture.
- Timed-out replacement rollback preserves binding id/generation, epoch and original instance; only candidate cleaned until final owner disposal.
- Late factory and onInit rejection consumption, plus synchronous get/resolveSync under zero deadline.

## Policies requiring source-owner confirmation before integration

1. **Explicit disabled value is not yet specified in current public types.** Two clearly labelled tests propose `null` for `buildTimeout` and registration `timeoutMs`; this is a proposed contract, not a claim that nullable types were approved/implemented. Replace with the selected explicit disable sentinel if different. Do not use a cast to pretend the API exists.
2. Invalid settings are asserted to throw at property assignment/registration, without demanding an undocumented error class/code. If the owner chooses deferred validation, adjust the tests and document the decision; invalid input must not silently disable deadlines.
3. Zero tests cover **never-settling async** work and immediate sync success. They intentionally do not decide whether an already-fulfilled Promise beats the zero deadline (microtask policy is not specified).
4. Diagnostic assertions require names/path/waits-for text and an arrow, not exact wording/format. Adjust textual markers if a documented equally expressive format is chosen; preserve assertions proving all pending tokens, path and the A→B edge.
5. Late onInit owned cleanup is asserted after onInit settles, avoiding a policy commitment about disposal before user initialization exits. Abort-aware disposal must still settle caller with DI_BUILD_CANCELLED even if its abort handler resolves the factory.
6. Replacement uses the existing BindingHandle.replace lifecycle API and `{ rebuild: true }`; global timeout supplies the deadline. No invented ReplacementOptions.timeoutMs is required.
7. Tests inspect pending cache only after terminal timeout/late flush. If the cache API intentionally exposes failed records, assert absence of **ready/pending** records instead of changing lifecycle semantics solely for an empty-array assertion.

## Safety and verification

Every observed build has a rejection handler attached immediately. One-second bounded watchdogs fail with a distinct non-DI error and clear their timers; they do not count as expected product timeouts. No global afterEach disposal is installed. Never-settling tests only dispose after verified product timeout, or explicitly test bounded disposal. Pending deferred promises alone do not keep the event loop alive. Vitest workers are expected to remain alive for product unref timers; standalone natural-exit behavior needs a separate child-process integration check later.

Executed only TypeScript `transpileModule` (ES2022/ESNext, reportDiagnostics): 24 cases, zero syntax diagnostics. This does **not** resolve imports, typecheck future APIs, or execute any regression. No broad or hanging negative suite was run.

Remaining G03 integration evidence: semantic typecheck, all 24 runtime cases under Bun/Vitest and Node as required, timer creation/clear/unref instrumentation or child-process natural-exit regression, stale waits-for cleanup regression beyond diagnostic presence, late rejection after disposal specifically, and full lint/build/regression gates. Those are not claimed complete by preparation.
