# G03 — asynchronous build control

## Implemented

- Public inherited `buildTimeout`, per-registration `timeoutMs`, and readonly contextual `signal`. `undefined` inherits (root default off); `null` disables; finite nonnegative milliseconds accepted. Zero schedules next-turn async expiration, never wraps a synchronous graph.
- Every materialization, including transient work, owns terminal state and active tracking. Factory and onInit share deadline and build signal. First and joining callers share controlled terminal outcomes. Deadline timers are unrefed and cleared on success, failure and cancellation.
- `DI_BUILD_TIMEOUT` diagnostics include all pending materializations, resolution path and current waits-for edges. Owning scope controls policy: a singleton uses its root, not a requesting child.
- Disposal first marks/cancels/aborts the full subtree and rejects `DI_BUILD_CANCELLED`, then tears down ready records. Ignored user promises cannot hold disposal open. A shared teardown promise is published before abort handlers, so synchronous reentrant disposal callers receive the same failure result.
- Terminal identity guards prevent old completion publishing into retry records. Late rejections are consumed; produced, unreturned, owned values clean up exactly once when the user operation settles. External values are never disposed. Successful ordinary transients remain caller-owned.
- Factory, initialization and completion-hook contexts retain originating signal identity after resolver/lineage expiry. Ready and successful transient signals follow owner disposal. Manual completion uses scope-lifetime signals. Native AbortSignal.any avoids per-transient owner listeners; weak-reference/finalizer fallback removes source listeners when combined signals abort or become unreachable.

## Regression/evidence

Prepared 24-case suite integrated into `tests/bun/core/build-timeout.test.ts`, with invalid preparation assumptions corrected against the existing API. Initial Node execution: **21 failed / 3 passed** (`red-node.log`, exit 1). Expanded final suite: **40 cases** including stable signals, registered/manual hooks, fallback, late cleanup rejection, owner routing, class overload/replacement policy, reentrant disposal, timer instrumentation, and natural child-process exit with only an unrefed build deadline remaining.

The old three pending-disposal tests and initialization shutdown expectation now require cancellation rather than waiting for arbitrary user promises. Captured additional red (`reentry-red.log`): an abort listener's nested disposal incorrectly swallowed a teardown error because shared identity was assigned after the abort callback. Fixed before final validation.

## Final validation

Authoritative runner: workspace `node_modules/vitest/vitest.mjs`; Node24.14.1 and local Bun1.3.0, asserted using CMINJECT_EXPECT_RUNTIME.

- Bun full runtime: **238/238**, 17 files (`full-bun.log`, exit 0).
- Bun React: final **19/19** (`react-bun-final.log`, exit 0). The earlier loaded-host run exposed the preexisting 10ms/30ms suspense timing fixture (`react-bun.log`: 18 pass/1 fail); its separate owner replaced elapsed sleeps with an explicit gate, preserving fallback and success assertions. No remaining timing retry is needed.
- Node combined V8: **256 passed / 1 Bun-only skipped**, 18 files (`coverage-terminal-node.log`, exit 0).
- All unchanged per-file coverage floors pass. Container: **92.67% statements / 87.19% branches / 92.01% functions / 95.90% lines**. Final terminal-rollback reviewed output copied to `coverage-terminal/`; earlier review outputs retained in `coverage-reviewed/` and `coverage/`. Module/lazy source owned by the separate G13 lane is present but unexercised in this checkpoint.
- Source TypeScript compile: Node and Bun exit 0 (`source-{node,bun}.log`).
- Strict affected fixture compile: Node and Bun exit 0 (`strict-{node,bun}.log`).
- Browser-core check and narrow diff whitespace check: exit 0 (`browser.log`, `diff-check.log`).

Prior G02 source revisions archived before editing under framework `.archive/2026-10-27/revisions/g03/packages/foundation/inject/`. No package/config/dependency/git changes made by this lane.

## Boundaries

Cancellation does not forcibly interrupt arbitrary JavaScript. A still-running initializer's owned value is cleaned after its initializer settles, outside the already-rejected caller/disposal path. Manual standalone member injection does not gain a build timeout. The fallback for environments without AbortSignal.any requires WeakRef and FinalizationRegistry; supported Node/Bun floors provide native any.

## Independent-review fixes

- HIGH replacement/disposal race: captured Bun red (`rollback-red.log`) resurrected an original record/disposal order after owner disposal. Rollback now skips disposing/disposed scopes, avoids reinstalling registrations into ended owners, and releases uninstalled original binding references plus all retained historical-issue snapshot refs. Owner checks immediately after async preflight/commit boundaries prevent allocating/installing into a concurrently ended owner. Regressions prove full owner disposal reclaims historical keys and leaves zero records/order after late initialization, and disposed-child rollback does not damage its still-active parent (including asynchronous late candidate cleanup).
- MEDIUM native timer overflow: captured fake-clock red (`overflow-red.log`) passed 2147483648 directly to native scheduling. All accepted finite delays now segment at 2147483647ms and re-arm against the absolute monotonic deadline, preserving unref and terminal cleanup. Fake-clock regression proves boundary expiration, Number.MAX_VALUE acceptance without immediate cancellation, and cancellation clearing the final segment.

- HIGH terminal rollback detached-original leak: exact independent-review repro captured in `terminal-rollback-red.log`: owner disposal during awaited candidate cleanup left both original dependency/dependent undisposed despite zero cache entries. Terminal rollback now disposes still-ready/unclaimed snapshot originals in reverse disposal order, respecting external ownership and existing cleanup claims. Cleanup errors attach to the original rollback error; ended scopes never restore records/history. The regression requires B then A exactly once, no cached/order entries, and token-cache baseline restoration. Focused Bun **40/40**, final full runtime and combined coverage counts above include this regression.
