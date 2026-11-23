# Migrating Inversify wiring

This covers dependency wiring, not application framework replacement. Inversify 8.x bind/get/getAsync map to explicit registrations, resolveSync and resolve. Its default transient scope is not cminject singleton. Choose lifetimes explicitly. Map activation/deactivation to lifecycle hooks with ownership tests. cminject snapshot handles are explicit and async to restore, not Inversify snapshot-stack restore calls. See [official container API](https://inversify.io/docs/api/container/).

## Runnable example

[Source](../examples/cminject.migrate-inversify.ts). This example is part of the final docs test suite.

```ts
import { Container, createToken } from '@coldmind/inject';

/** Migrates explicit bindings, multi ordering and definition snapshots to inject APIs. */
export async function runExample(): Promise<string> {
  interface Weapon { strike(): string }
  const Weapon = createToken<Weapon>('Weapon');
  const Extension = createToken<string>('Extension');
  const Extra = createToken<number>('Extra');
  class Sword implements Weapon { strike(): string { return 'slash'; } }
  class Fighter {
    constructor(readonly weapon: Weapon) {}
    attack(): string { return this.weapon.strike(); }
  }
  const root = new Container();
  try {
    root.singleton(Weapon, () => new Sword(), { deps: [] });
    root.transient(Fighter, { deps: [Weapon] });
    root.singletonMulti(Extension, () => 'first', { deps: [] });
    root.singletonMulti(Extension, () => 'second', { deps: [] });
    root.validate();
    const attack = root.resolveSync(Fighter).attack();
    const saved = root.snapshot();
    try { root.singleton(Extra, () => 1, { deps: [] }); await root.resolve(Extra); }
    finally { await root.restore(saved); }
    if (root.has(Extra)) throw new Error('Snapshot did not restore definitions');
    return `${attack}|${(await root.resolveAll(Extension)).join(',')}|restored:${!root.has(Extra)}`;
  } finally {
    await root.dispose();
  }
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)
