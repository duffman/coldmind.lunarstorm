# Introspection and validation

graph(), bindingOf(), cachedBindings(), dependency/resource snapshots and inspect() describe registrations and managed instances. validate({ requireDeps: true }) reports undeclared factory dependencies; deps: [] is explicitly declared. includeObserved merges current-generation dynamic edges for drift/cycle checks. Inspection exposes drift separately. qualify(Db, "replica") interns a distinct typed token named Db#replica. when predicates run in registration order before the unconditional fallback and receive requester/path/scope. Static graph queries do not execute predicates. Registry epochs change on definition mutations, not cache hits.

## Runnable example

[Source](../examples/cminject.introspection.ts). This example is part of the final docs test suite.

```ts
import { Container, createToken, qualify, tokenName } from '@coldmind/inject';
import { inspect } from '@coldmind/inject/inspect';

/** Inspects declared/observed wiring and selects qualified conditional bindings. */
export async function runExample(): Promise<string> {
  const Db = createToken<{ name: string }>('Db');
  const Replica = qualify(Db, 'replica');
  const Label = createToken<string>('Label');
  const root = new Container();
  try {
    root.singleton(Db, () => ({ name: 'primary' }), { deps: [] });
    root.singleton(Replica, () => ({ name: 'fallback' }), { deps: [] });
    root.singleton(Replica, () => ({ name: 'replica' }), {
      deps: [], when: ({ requester }) => requester?.tokenName === tokenName(Label),
    });
    root.singleton(Label, {
      inject: [Db, Replica] as const,
      factory: (primary, replica) => `${primary.name}/${replica.name}`,
    });
    root.validate({ requireDeps: true });
    const label = await root.resolve(Label);
    const fallback = await root.resolve(Replica);
    root.validate({ requireDeps: true, includeObserved: true });
    const snapshot = inspect(root);
    if (qualify(Db, 'replica') !== Replica) throw new Error('Qualified token was not interned');
    if (snapshot.dependencies.nodes.length === 0 || snapshot.resources.resources.length === 0) throw new Error('Empty inspection');
    return `${label}|top-level:${fallback.name}|issues:${snapshot.issues.length}`;
  } finally {
    await root.dispose();
  }
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)
