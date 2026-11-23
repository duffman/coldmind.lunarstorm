# Getting started

Create one root, register services, resolve them and dispose the root. Prefer exported typed symbols to string tokens. Singleton ownership belongs to its registering scope; scoped instances belong to the resolving scope.

## Runnable example

[Source](../examples/cminject.getting-started.ts). This example is part of the final docs test suite.

```ts
import { Container, createToken } from '@coldmind/inject';

/** Registers explicit constructor and typed factory dependencies. */
export async function runExample(): Promise<string> {
  const Config = createToken<{ region: string }>('Config');
  const Message = createToken<string>('Message');
  class Greeting {
    constructor(readonly config: { region: string }) {}
    text(): string { return `Hello from ${this.config.region}`; }
  }
  const root = new Container();
  try {
    root.singleton(Config, () => ({ region: 'north' }), { deps: [] });
    root.transient(Greeting, { deps: [Config] });
    root.singleton(Message, { inject: [Config] as const, factory: config => config.region.toUpperCase() });
    root.validate({ requireDeps: true });
    if (root.tryGet(Message) !== undefined) throw new Error('A cache read built a service');
    const greeting = await root.resolve(Greeting);
    await root.resolve(Message);
    return `${greeting.text()}|${root.get(Message)}`;
  } finally {
    await root.dispose();
  }
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)
