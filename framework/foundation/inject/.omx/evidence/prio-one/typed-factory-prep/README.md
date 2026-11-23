# PRIO_ONE 4.4 typed-factory compiler preparation

Prepared only; no live source, configuration, package, or documentation changed.

## Artifacts

- `.omx/prepared-tests/typed-factory.facade.ts`: isolated declaration facade importing actual token, dependency slot/value, lifecycle, factory, options, handle, and IContainer types. All ten methods use the same three overloads.
- `.omx/prepared-tests/typed-factory.test-d.ts`: positive/negative cases, all ten methods, exact handle checks, readonly/empty tuples, class/abstract/delayed/typed/unqualified tokens, Lazy/Provider, sync/async, hooks, legacy factory/constructor/data forms and G04 no-data/undefined/union/caller-any arity preservation.
- `.omx/prepared-tests/typed-factory.dialects.test-d.ts`: predeclared malformed tuple lifecycle cannot fall through to the legacy overload; ordinary exported ServiceLifecycle variables remain accepted.
- `.omx/prepared-tests/typed-factory.tsconfig.json`: isolated compiler entry point, extends the actual project strict compiler options.
- `proposed-signatures.patch`: actual aliases and overload signatures for all ten IContainer registration methods. This is a proposal, NOT an applied or runtime-complete patch. Equivalent overload insertion/legacy lifecycle guard is required in Container and ContainerView; implementation/runtime integration remains another task.

## Contract and inference findings

1. Put the typed overload BEFORE legacy overloads. With a callback generic constrained directly to `InjectedFactory<T, Slots>`, TypeScript contextually types the callback from DependencyValues. Putting it after legacy overloads incorrectly contextually types the first argument as IContainer.
2. Capture the callback's actual type in Factory, then compare its Parameters length with the inject tuple length in BOTH directions. An incompatible arity makes the options rest tuple a required impossible argument; ordinary valid calls retain optional RegisterOptions. This catches omitted callbacks arguments, extra arguments, optional arguments, rest arrays, and widened inject arrays. A plain function constraint alone would allow fewer callback parameters under TypeScript function assignability.
3. Legacy lifecycle input is intersected with `{ readonly inject?: never }`. This intentionally reserves known `inject` properties for the tuple dialect, preventing predeclared malformed tuple objects from structurally falling back to ServiceLifecycle. Exported ServiceLifecycle and ServiceFactory declarations are unchanged; bare c/data factories, lifecycle variables, required caller-any data and Constructor overloads are preserved.
4. Callback return is constrained by the token's service type; Promise<T> is allowed without making handles asynchronous.
5. No new `any` type syntax, casts, double assertions, dependencies, or runtime implementations are introduced. The G04 caller-any case derives the existing standard-library JSON.parse return solely to exercise that already-supported compatibility branch.
6. Usual TypeScript limitation: a caller can explicitly erase extra properties into the exported ServiceLifecycle type, losing static knowledge of inject. No structural type system can recover erased properties. This is not a known-inject fallback; runtime dialect discrimination still must test actual inject presence.

## Verification

The exact same fixture is checked by local TypeScript 5.9.3 under:

```
/Users/patrik/.nvm/versions/node/v24.14.1/bin/node node_modules/typescript/bin/tsc -p .omx/prepared-tests/typed-factory.tsconfig.json --pretty false
.cache/bun/bun-darwin-aarch64/bun node_modules/typescript/bin/tsc -p .omx/prepared-tests/typed-factory.tsconfig.json --pretty false
```

Both exit 0 with no diagnostics (including no unused negative assertions).
`unsuppressed-diagnostics.mjs` reruns the real local compiler with only negative-directive comments stripped in memory; both hosts must independently emit errors for the negative fixture. Logs contain actual compiler diagnostics. This is additional evidence, not a replacement for the CLI checks.

No runtime core behavior, graph derivation, exact runtime argument ordering, package build, lint, or full repository suite is claimed by this preparation task.
