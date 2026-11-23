# Lifetimes

singleton caches per registration owner; transient builds on every call; scoped caches per resolving scope; tenant and request require matching ancestor scope kinds. A longer-lived service must not capture a shorter-lived tenant/request service. Successfully returned transients are caller-owned. registerInstance defaults to owned; use explicit external ownership when another system disposes the value.

## Runnable example

[Source](../examples/cminject.lifetimes.ts). This example is part of the final docs test suite.

```ts
import { Container, createToken } from '@coldmind/inject';

/** Verifies tenant/request boundaries and externally managed instance ownership. */
export async function runExample(): Promise<string> {
  const Cache = createToken<Map<string, string>>('Cache');
  const Events = createToken<string[]>('Events');
  const External = createToken<{ onDispose(): void }>('External');
  let externalDisposals = 0;
  let ownedDisposals = 0;
  const root = new Container();
  let result = '';
  try {
    root.tenant(Cache, () => new Map<string, string>(), { deps: [] });
    root.request(Events, { factory: () => [], onDispose: () => { ownedDisposals++; } }, { deps: [] });
    root.registerInstance(External, { onDispose: () => { externalDisposals++; } }, {
      ownership: 'external', reason: 'Example host owns this resource',
    });
    const tenant = root.createTenantScope('north');
    const first = tenant.createRequestScope('first');
    const second = tenant.createRequestScope('second');
    const cache = await first.resolve(Cache);
    cache.set('status', 'ready');
    const events = await first.resolve(Events);
    events.push(cache.get('status')!);
    const other = await second.resolve(Events);
    if (await second.resolve(Cache) !== cache || events === other) throw new Error('Wrong cache boundary');
    if (await first.resolve(Events) !== events) throw new Error('Request cache was not reused');
    result = `${events.join()}|isolated:${other.length === 0}`;
  } finally {
    await root.dispose();
  }
  return `${result}|owned-disposed:${ownedDisposals}|external-disposed:${externalDisposals}`;
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)
