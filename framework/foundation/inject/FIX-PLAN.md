# cminject: fix plan

Scope: the design defects and bugs found in the code review, plus constructor injection in both dialects and
`container.autoInject`. Nothing is removed. Tests are written, not run: the commands are at the end.

## Assumptions

- `autoInject` is a property on `Container`, inherited by child scopes unless set on them.
- The shared decorator kernel goes to `packages/foundation/decorate` as `@coldmind/decorate`.
- `registerInstance` keeps disposing supplied instances by default (the behaviour); the doc and the reported ownership change to match.

## 1. One post-construction path, sync when possible (design defect 1)

Historical defect, now repaired: class construction and completion stay synchronous until a dependency/hook actually returns a native promise.

- New private `complete(instance, descriptor, view)` in `cminject.container.ts`: members, `derive`, wraps, instance
  hooks. Built on the existing `then`/`each` helpers: synchronous until a step returns a promise.
- `classFactory` resolves params through the view's `resolveIn` (record or promise per slot), constructs synchronously when
  every slot is synchronous, then calls `complete`.
- `injectMembers(instance)` (added without approval) becomes the public face of `complete` for an instance the container
  did not build: members **and** wraps and hooks, same semantics as a container-built instance. Returns `T`, or throws
  `LifecycleError` naming the member whose service is async. Add `injectMembersAsync(instance): Promise<T>`.
- Optional/multi slot resolution moves out of the async closure into one helper shared by params and members.

## 2. Constructor injection in both dialects

Token sources per constructor parameter, first match wins:

1. `@Inject`/`@InjectOptional`/`@InjectAll` on the parameter (legacy).
2. The class decorator's `deps` (`@Injectable({ deps })`, `@Singleton({ deps })`, ...).
3. `static $inject`.
4. Compiler-emitted parameter tokens (below), stored as `static $inject` on the class.
5. Reflected `design:paramtypes` (legacy + `emitDecoratorMetadata`).

**Standard dialect without parameter decorators:** a ctsc pass emits the tokens from the constructor's parameter types.

- New `packages/tools/ctsc/passes/cminject.paramtypes.ts`.
  - `analyze`: for every class with a cminject class decorator, resolved by symbol, use the checker to read each
    constructor parameter's type.
  - `visit`: add `static $inject = [A, B, ...]` to the class.
  - A parameter whose type is not a class value (an interface, union or primitive): emit `undefined` at that index, and
    report `ctx.error` with the class, the parameter name and "name it in deps or $inject".
- A class that already declares `static $inject`, or a decorator with `deps`, is left unchanged.
- `split build` runs this pass with its own passes.

**Supplied leading arguments:** new `container.instantiate(ctor, ...args)`.

- Positional `args` fill the first parameters, the container resolves the rest, then `complete`.
- Sync when possible, otherwise `instantiateAsync`.
- It is not registered or cached, and not disposed by the container.
- This is how `Application.createForm` builds a form with its owner: `kernel.container.instantiate(FormClass, this)`.

**Reflected non-class types (design defect 3):** treat `Object`, `Function`, `Array`, `String`, `Number`, `Boolean`,
`Symbol`, `Promise` and `undefined` as "no token". Throw `DiError` `DI_UNRESOLVABLE_PARAMETER` naming class, index and
parameter type, never `NotRegisteredError(class Object)`.

## 3. `container.autoInject`

```ts
container.autoInject = true;                                   // or
container.autoInject = { lifetime: ServiceLifetime.Singleton, members: true };
```

- **Type:** `boolean | AutoInjectOptions`, with `{ lifetime?: ServiceLifetime /* default Transient */; members?: boolean }`.
  Readable and writable at any time. A child scope inherits it until it sets its own (`autoInject = false` turns it off there).
- **Unregistered class tokens:** with `autoInject` on, `resolve(SomeClass)` / `resolveSync(SomeClass)` registers the class
  implicitly on this container with `options.lifetime` (`kindTag: 'auto'`), using the parameter sources from section 2, then
  resolves it. A class with its own registration (`@Injectable`, `$injectable`) keeps that registration's lifetime.
- **Constructor parameters:** a parameter with no token at all fails with `DI_UNRESOLVABLE_PARAMETER`; `autoInject` never
  guesses beyond the five sources.
- **Members:** with `members: true`, `injectMembers` and `instantiate` also auto-resolve the dependencies of
  auto-registered classes.
- **Graph:**
  - Auto registrations go through `install`, so `validate()`, the graph, `discover` and snapshots see them.
  - `kindTag: 'auto'` marks them in `BindingDescriptor`.
- **Strings, symbols and typed tokens are never auto-registered:** they still fail with `NotRegisteredError`.
- `IContainer` gets `autoInject`, `instantiate`, `instantiateAsync` and `injectMembersAsync`.

## 4. One decorator kernel for cminject and rpc (design defect 4)

New `@coldmind/decorate` (`packages/foundation/decorate/src/decorate.*.ts`), browser-safe:

- Dialect normalization for every placement: class, method, field, accessor, getter, setter, parameter.
- **Standard dialect:** a per-class record keyed on `context.metadata`. No global pending queue, so no class decorator is
  required and members never attach to another class.
  - `onClass(callback)` runs when the class decorator runs, or on the first lookup through `ctor[Symbol.metadata]`.
  - `strayMembers()` stays.
- **Legacy dialect:** a record keyed on the constructor.
- Field wrapping (`FieldWrap`: `get`/`set`/`init`) from rpc, and parameter sites from cminject.
- `Symbol.metadata` polyfill lives here, once.

cminject's `defineDecorator` and rpc's `defineDecorator` become thin layers over it. Both keep their public signatures.
rpc's `flush` wildcard match (`metadata === undefined` matches everything) goes away with the queue.

## 5. `reflect-metadata` without a global side effect in core (design defect 5)

- `reflect-metadata` is a first-class library for browser and Node. `cminject.decorators.ts` imports it and stays listed in
  `sideEffects`. Nothing changes here.

## 6. Bugs

1. **`registerInstance` ownership.**
   - Without a policy, report `{ ownership: 'owned', reason: 'supplied through registerInstance' }` and dispose.
   - With `{ ownership: 'external' }`, never dispose.
   - Drop the `disposeSupplied` special case.
   - Fix the doc on `Container` and `IContainer`.
2. **`clearInstances` becomes `async clearInstances(): Promise<void>`.**
   - Await in-flight builds.
   - Dispose owned ready records in reverse build order.
   - Then drop them.
   - Never clear `pending` under a running build.
3. **`data` on cached lifetimes.**
   - `entry` stores `data` on the record (it already does).
   - A resolve on a non-transient binding that hits a ready or pending record built with different data
     (`!Object.is`) throws `LifecycleError` naming the token.
4. **Cycles through a captured container.**
   - `cminject/server` sets the current `Resolver` in its AsyncLocalStorage (`cminject.context.ts`) around every
     factory call.
   - A `ROOT` resolve on the server picks it up, so waits-for detection sees resolves made through a captured container.
   - Core stays free of ambient state.
   - Core gets `container.buildTimeout` (ms, inherited, default off). A build pending longer rejects with a
     `DiError` `DI_BUILD_TIMEOUT` listing the pending builds and their waits-for edges.
5. **Captive lifetimes.**
   - Rank lifespans Singleton > Tenant > Request; Scoped and Transient take the rank of the scope they are built in.
   - In `entry`, before `route`, throw the `captive` `DiGraphError` when a binding of shorter lifespan is resolved
     while a longer one is building.
   - The `ScopeMissingError` diagnosis is kept for resolves outside any build.
6. **`onReady` once.** A per-container `WeakSet` of records whose `onReady` ran; `initializeOwned` skips them.
7. **Standard class decorator without a metadata object.** One object, created once, is used for both the record and
   `standardCtor`.
8. **Thenable services.**
   - When `shield` cannot hide `then` (non-configurable or non-extensible), `resolve` rejects with `LifecycleError`
     ("thenable service cannot be resolved through a promise; use resolveSync or get").
   - It never settles with the adopted value.
9. **Duplication.**
   - `disposeRecorded` and `disposeQuietly` merge into `disposeRecord(record, track)`.
   - One `disposes(binding)` predicate replaces the three ownership checks.
   - Move the two orphan doc comments (container.ts:38–39, 259) onto their declarations.

## 7. VCL wiring (replaces the unapproved edits)

- **`Application.createForm`:** `const form = kernel.container.instantiate(FormClass, this); load(form);`.
- **`load(component, container = kernel.container)`:** keeps injecting the tree, now through `injectMembers` with the full
  pipeline, so components built with `new Child(this)` inside a form are injected too. `instantiate` already completed
  the form itself; `injectMembers` skips an instance it already completed (a `WeakSet` of completed instances).

## Order

1. §4 kernel (rpc and cminject on it).
2. §1 sync path.
3. §2 constructor injection + `instantiate`.
4. §3 `autoInject`.
5. §5 (nothing to do: `reflect-metadata` stays imported).
6. §6 bugs.
7. §7 VCL.
8. ctsc pass from §2.

## Tests to write

- `tests/bun/core/sync.test.ts`:
  - `resolveSync` of a decorated class with sync dependencies;
  - an async dependency names the member;
  - `injectMembers` applies wraps and hooks.
- `tests/bun/core/construct.test.ts`:
  - each of the five token sources and their precedence;
  - `instantiate` with leading arguments;
  - the `DI_UNRESOLVABLE_PARAMETER` message.
- `tests/bun/core/auto-inject.test.ts`:
  - unregistered class resolution;
  - lifetime option and inheritance;
  - a child scope turning it off;
  - strings and symbols never auto-registered;
  - `kindTag: 'auto'` in `validate()` and snapshots.
- `tests/bun/core/regressions.test.ts`: one test per bug in §6.
- `tests/bun/legacy/`:
  - parameter decorators and reflected types;
  - reflected `Object` error.
- `packages/foundation/decorate/tests/`: kernel suites, standard and legacy, including "no class decorator" and "no cross-class
  leak".
- `packages/tools/ctsc/tests/cminject.paramtypes.test.ts`: class in, `static $inject` out; interface parameter error.

## Commands (you run them)

```bash
cd packages/foundation/inject && bun run test:bun && bun run test:bun:legacy && bun run typecheck
cd packages/foundation/decorate && bunx vitest run
cd packages/contract/rpc && bunx vitest run
```

## Superseding complete-delivery integration

The implementation now includes the planned parameter pass at `../../tools/ctsc/src/passes/cminject.paramtypes.ts`, split integration and explicit source/dist exports. Timeout/cancellation live in core resolution. Final typecheck/review/test results must be attached before completion is claimed; the earlier bug descriptions above are historical rationale.

## Final verification — 2026-10-27T18:15:46.597Z

Complete integrated scope verified solo: 356 functional cases each on Node/Bun, React19Bun/18Node+1skip, compiledNode13cases on22.0/24,19compiler contracts,11ctscNode/13splitBun cases, real compiler/injectNode+Bun execution, clean builds, TypeDoc and all import/browser/cycle/coverage gates pass. See `.omx/evidence/prio-one/solo-final/README.md` and tests/FEATURES.md. Earlier pending/draft notes are historical. Container1041lines after feature additions; the861-line Phase2 figure is not the final file size.
