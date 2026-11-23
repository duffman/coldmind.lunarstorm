# G04 — public types and sound context facades

## Implemented

- Constructor and AbstractConstructor use `never[]`, preserving required-argument constructor/token inference without exporting `any[]`. Actual arbitrary-argument construction uses Reflect.construct at a documented concrete-constructor boundary; the zero-argument fast path remains synchronous.
- Universal decorators have explicit legacy/TC39 class, field, accessor, getter, setter, method and parameter signatures. Registration aliases expose class placement only; injection aliases expose field/accessor/setter and legacy constructor parameters only. Abstract class declarations/constructor parameters remain supported. Builtin aliases use checked annotations rather than assertions.
- ServiceFactory uses a caller-any detector followed by a non-distributive argument tuple: `(c, ...data: 0 extends (1 & D) ? [data: D] : [D] extends [void] ? [] : [data: D])`. Default void, explicit undefined, and void|undefined retain exactly one parameter. Other payloads, including unions containing undefined, keep their explicit data slot and contextual type. Class factories ignore an optional unknown data input internally rather than asserting conditional factory compatibility.
- TypedBinding<T,D> genuinely implements BindingHandle<T,D>; internal registry Binding erases only the public replace payload signature via Omit and recursive operational links, so generic variance never requires an unsafe cast; plain registrations remain their own allocation-free handles. Retained handles hold typed bindings. Removed fake handle/auto-binding casts.
- Pulled forward the approved G05 explicit view boundary in place: ContainerView implements IContainer and the internal construction scope, with real generic overloads/accessor setters and direct actual-target delegation. Factory/completion subclasses retain contextual resolution/construction and stable signals. Private operation callbacks are allocated once per container. Preserved public Container extras (tracked disposal and runtime rollback) and one-way instanceof prototype attachment. No layout/file extraction yet.
- Removed all `as unknown as`, fake IContainer/Scope assertions, runtime-generated forwarding, private bracket escapes and prototype method transplants. Shared immutable empty lists now have actual readonly types. Remaining internal assertions have nearby ownership/metadata/typed-token invariant reasons; inventory in `assertions.txt` (31 including three const assertions). No explicit any syntax remains in production source.
- Authored member writes use a checked Reflect.set boundary, retaining strict assignment failure on nonwritable/frozen instances. Identity copying retains the historical for-in/inherited-property semantics without a double assertion.

## Red → green

- Initial actual compiler: six fixtures, **2 passed / 4 failed**, eleven diagnostics (`types-red.log`).
- Expanded primary compiler suite: **10/10** on Node and Bun with `ignoreSourceErrors:false`; covers constructor parameters, abstract tokens, factory payloads/unions, decorator placements and explicit undefined/void exact factory arity (`types-{node,bun}.log`). Caller-supplied any is tested deliberately only in consumer fixtures, not production source.
- Independent review caught an intermediate ServiceFactory simplification that lost explicit-undefined one-argument arity. Regression captured **8 pass / 1 fail, five diagnostics** (`undefined-red.log`); non-distributive tuple fixed it without public any/double assertions or an internal factory erasure assertion.
- Three new runtime tests cover facade overloads/reporters/mutation/actual receivers, sync+async construction/injection, and strict frozen authored-member failure (`tests/bun/core/view.test.ts`).

## Final validation

Workspace runner `node_modules/vitest/vitest.mjs`, Node24.14.1 and Bun1.3.0.

- Node runtime **241/241**; Bun runtime **241/241** (`full-{node,bun}.log`, exit 0), all three decorator runtime dialects retained.
- Bun React **19/19** (`react-bun.log`, exit 0).
- Node combined V8 **259 passed / 1 Bun-only skip**, unchanged per-file floors pass. Container **92.68% statements / 86.64% branches / 94.21% functions / 96.34% lines**; decorators **88.81 / 82.60 / 89.65 / 95.31**. Final reviewed immutable coverage output in `coverage-reviewed/`; prereview output retained in `coverage/`. Unexported module/lazy leaf source belongs to its separate lane and is not exercised at this checkpoint.
- Source tsc and strict six affected runtime fixture checks pass on **both runtimes** (`source-*`, `strict-*`, exit 0).
- Additional actual experimentalDecorators/emitDecoratorMetadata consumer syntax compiles on both runtimes (`legacy-types-*`, exit 0): required-argument abstract base, injected constructor/property/setter, class decorator; class-on-field and method-parameter negatives preserved.
- Browser-core and narrow diff checks pass (`browser.log`, `diff-check.log`, exit 0).

## Legacy checker limitation reported upward

Legacy TypeScript allows a two-argument property decorator on a method syntactically; it does not expose a distinct placement context. Rejecting all function-valued keys would also reject valid function-valued injected fields. Direct legacy three-argument method descriptor calls are rejected by the public overloads; TC39 method/getter placements are rejected; legacy runtime placement checks still reject method injection. The supplemental failed checker experiment is retained separately in `legacy-placement-limitation.ts` (unused expected-error diagnostic), not hidden as a successful negative test. The primary compiler suite's negative assertions were not weakened. This inherent legacy syntax limit was approved as a documented expressibility boundary, not a task blocker; InjectDecorator API JSDoc now states it.

G03 sources archived before G04 edits under framework `.archive/2026-10-27/revisions/g04/packages/foundation/inject/`. No dependency, package, config, git, goal or leader-state writes by this lane. Source frozen pending independent re-review/checkpoint; no G05 layout work started.

## Caller-supplied any compatibility review

Independent review caught the non-distributive void check treating an explicitly supplied any data type as no-data. `any-red.log` captures nine existing tests passed / one failed, nine diagnostics. `any-previous.ts`/`.log` probe the original conditional directly: declared, required-data, and parameter-ignoring factories all reject one-argument calls, while two-argument payload calls compile. The explicit detector restores this historical call shape for direct and lifecycle factories with no own any syntax or erasure assertions. The tenth compiler test verifies required payload assignment/calls, lifecycle parity and rejection of missing payload calls. Registry storage now omits the typed public replace operation instead of pretending all typed bindings implement a BindingHandle<unknown,unknown>; actual public handles remain genuinely generic and allocation-free. Final source/types/runtime/strict/browser/coverage evidence above was refreshed after this fix.
