# Modules

defineModule preserves object identity. load(...modules) installs root-owned tree-wide registrations and returns idempotent unload handles. Imports load depth-first; cycles are module-cycle graph issues. Repeated explicit loads do not accumulate hidden references. Shared imports survive remaining dependents or explicit owners. The registration callback receives an explicit transaction facade that captures registrations across await; unrelated root registrations are not captured. Retained registration facades cannot be used after callback completion. Use imports rather than recursively loading from register. Failed callbacks roll back their own registrations; module-owned pending mutations are rejected. Typed provides/requires capability maps are opt-in author declarations, not inferred token identity proofs.

## Runnable example

[Source](../examples/cminject.modules.ts). This example is part of the final docs test suite.

```ts
import { Container, composeModules, createToken, defineModule } from '@coldmind/inject';

/** Loads an async typed module graph once and unloads its owned registrations. */
export async function runExample(): Promise<string> {
  const Config = createToken<{ region: string }>('Config');
  const Label = createToken<string>('Label');
  let disposed = 0;
  const configuration = defineModule<{ config: { region: string } }, {}>()({
    name: 'configuration',
    register(c) { c.singleton(Config, () => ({ region: 'north' }), { deps: [] }); },
  });
  const labels = defineModule<{ label: string }, { config: { region: string } }>()({
    name: 'labels',
    imports: [configuration],
    async register(c) {
      await Promise.resolve();
      c.singleton(Label, {
        inject: [Config] as const,
        factory: config => `Region: ${config.region}`,
        onDispose: () => { disposed++; },
      });
    },
  });
  const composed = composeModules(configuration, labels);
  const root = new Container();
  try {
    const [handle] = await root.load(composed);
    if (!handle) throw new Error('Module load returned no handle');
    const label = await root.resolve(Label);
    await handle.unload();
    if (root.has(Label) || root.has(Config)) throw new Error('Unload retained module bindings');
    return `${label}|unloaded:${disposed}`;
  } finally {
    await root.dispose();
  }
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)
