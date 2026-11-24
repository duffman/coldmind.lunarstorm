# Errors

Catch DiError and inspect its stable code; graph issue kinds are not error codes. DI_GRAPH_UNSOUND carries missing, cycle, captive, dangling, decorator, module-cycle, undeclared, undeclared-edge or ambiguous issues. Timeouts/cancellation do not imply user work was forcibly terminated. Cleanup errors may be attached to the original failure. Never retry an operation on a disposed container.

## Runnable example

[Source](../examples/cminject.errors.ts). This example is part of the final docs test suite.

```ts
import { Container, createToken, DiError, DiGraphError } from '@coldmind/inject';

/** Matches stable error codes and keeps graph issue kinds distinct from codes. */
export async function runExample(): Promise<string> {
  const Missing = createToken<string>('Missing');
  const Consumer = createToken<string>('Consumer');
  const root = new Container();
  try {
    let missingCode = '';
    try { await root.resolve(Missing); }
    catch (error) {
      if (!(error instanceof DiError) || error.code !== 'DI_NOT_REGISTERED') throw error;
      missingCode = error.code;
    }
    if (!missingCode) throw new Error('Missing dependency unexpectedly resolved');
    root.singleton(Consumer, c => c.resolve(Missing), { deps: [Missing] });
    try { root.validate(); }
    catch (error) {
      if (!(error instanceof DiGraphError) || !error.issues.some(issue => issue.kind === 'missing')) throw error;
      return `${missingCode}|${error.code}:missing`;
    }
    throw new Error('Validation accepted a missing dependency');
  } finally {
    await root.dispose();
  }
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)

## Stable codes

- `DI_BUILD_CANCELLED`: The owning scope ended before materialization completed.
- `DI_BUILD_TIMEOUT`: An asynchronous materialization exceeded its configured deadline.
- `DI_CIRCULAR_DEPENDENCY`: The explicit resolution or waits-for graph contains a cycle.
- `DI_GRAPH_UNSOUND`: Graph validation failed; inspect issues for the problem kinds.
- `DI_LIFECYCLE_ERROR`: The scope/build state or synchronous API cannot perform this operation.
- `DI_MUTATION_CONFLICT`: The requested registry mutation cannot safely proceed.
- `DI_NOT_REGISTERED`: No matching registration; conditional candidates may have no matching predicate/fallback.
- `DI_SCOPE_MISSING`: The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.
- `DI_UNRESOLVABLE_PARAMETER`: The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.
- `RUNTIME_DISPOSED`: The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.
- `RUNTIME_DRAINING`: The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.
- `RUNTIME_FAILED`: The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.
- `RUNTIME_NOT_READY`: The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.
- `TENANT_DRAINING`: The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.
- `TENANT_FAILED`: The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.
- `INVALID_SCOPE`: A request lease was released or the requested runtime scope is invalid.
