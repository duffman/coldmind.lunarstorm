# Testing utilities

await createTestContainer(...modules) creates an isolated root with buildTimeout 2000. snapshot() returns an opaque owner-bound checkpoint; await restore(snapshot) reinstates definitions and disposes post-snapshot instances without reviving disposed scopes. Unchanged live owned supplied values preserve identity. A disposed owned supplied original cannot be reconstructed: restoration rejects before mutation. override is asynchronous; await its handle before resolving the replacement and restore nested handles in LIFO order. Owned registerInstance overrides reject before disposal. Callable values use { kind: 'value', value: fn }; explicit factories use { kind: 'factory', factory: fn }. Restoration rebuilds factory-backed originals instead of reusing disposed objects.

## Runnable example

[Source](../examples/cminject.testing.ts). This example is part of the final docs test suite.

```ts
import { createToken, defineModule, DiError } from '@coldmind/inject';
import { createTestContainer, override } from '@coldmind/inject/testing';

/** Restores factory definitions, snapshots and rejects non-recreatable owned overrides. */
export async function runExample(): Promise<string> {
  const Clock = createToken<{ now(): number }>('Clock');
  const Owned = createToken<{ onDispose(): void }>('Owned');
  const Extra = createToken<number>('Extra');
  let builds = 0;
  let ownedDisposals = 0;
  const clocks = defineModule({
    name: 'clocks',
    async register(c) {
      await Promise.resolve();
      c.singleton(Clock, () => { builds++; return { now: () => 7 }; }, { deps: [] });
    },
  });
  const root = await createTestContainer(clocks);
  let result = '';
  try {
    const original = await root.resolve(Clock);
    const replacement = await override(root, Clock, { now: () => 42 });
    let overridden: number;
    try { overridden = (await root.resolve(Clock)).now(); }
    finally { await replacement.restore(); }
    const restored = await root.resolve(Clock);
    if (restored === original || builds !== 2) throw new Error('Restore reused an old instance');
    const before = root.snapshot();
    try { root.scoped(Extra, () => 9, { deps: [] }); await root.resolve(Extra); }
    finally { await root.restore(before); }
    if (root.has(Extra)) throw new Error('Snapshot retained new binding');
    const owned = { onDispose: () => { ownedDisposals++; } };
    root.registerInstance(Owned, owned);
    let conflict = false;
    try {
      const unexpected = await override(root, Owned, { onDispose() {} });
      await unexpected.restore();
    } catch (error) {
      if (!(error instanceof DiError) || error.code !== 'DI_MUTATION_CONFLICT') throw error;
      conflict = true;
    }
    if (!conflict || root.get(Owned) !== owned || ownedDisposals !== 0) throw new Error('Owned override was not atomic');
    result = `override:${overridden}|restored:${restored.now()}|rebuilt:${builds}|owned-conflict:${conflict}`;
  } finally {
    await root.dispose();
  }
  return `${result}|owned-disposed:${ownedDisposals}`;
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)
