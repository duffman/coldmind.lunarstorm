# Async construction and cycles

Factories and onInit may return native promises. Concurrent cached resolutions share a build. resolveSync stays synchronous until asynchronous work is necessary, then throws LifecycleError. Set buildTimeout in milliseconds; undefined inherits, null disables, and registration timeoutMs overrides the owner policy. Cancellation aborts signals and rejects callers; arbitrary user promises are not forcibly stopped. Lazy/provider edges defer resolution without disabling captive checks. Use the supplied factory container for contextual resolution, especially across await.

## Runnable example

[Source](../examples/cminject.async-and-cycles.ts). This example is part of the final docs test suite.

```ts
import { Container, createToken, lazy, provider, type Lazy } from '@coldmind/inject';

/** Joins async singleton builds and breaks a scoped cycle with a deferred slot. */
export async function runExample(): Promise<string> {
  const Connection = createToken<{ ready: boolean }>('Connection');
  const Serial = createToken<number>('Serial');
  const Next = createToken<() => Promise<number>>('Next');
  class A { constructor(readonly other: Lazy<B>) {} }
  class B { constructor(readonly other: A) {} }
  const root = new Container();
  let builds = 0;
  let serial = 0;
  let disposed = 0;
  let result = '';
  try {
    root.buildTimeout = 2_000;
    root.singleton(Connection, {
      factory: async c => { c.signal.throwIfAborted(); builds++; await Promise.resolve(); return { ready: false }; },
      onInit: async value => { await Promise.resolve(); value.ready = true; },
      onDispose: value => { value.ready = false; disposed++; },
    }, { deps: [], timeoutMs: 500 });
    root.scoped(A, { deps: [lazy(B)] });
    root.scoped(B, { deps: [A] });
    root.transient(Serial, () => ++serial, { deps: [] });
    root.scoped(Next, { inject: [provider(Serial)] as const, factory: next => next });
    const scope = root.createScope();
    const [first, second] = await Promise.all([scope.resolve(Connection), scope.resolve(Connection)]);
    const a = await scope.resolve(A);
    if (a.other.peek() !== undefined) throw new Error('Lazy peek built its dependency');
    const b = await a.other.get();
    const next = await scope.resolve(Next);
    result = `shared:${first === second && first.ready && builds === 1}|cycle:${b.other === a}|provider:${await next()},${await next()}`;
  } finally {
    await root.dispose();
  }
  return `${result}|disposed:${disposed}`;
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)
