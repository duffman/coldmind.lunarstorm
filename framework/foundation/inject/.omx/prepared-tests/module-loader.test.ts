import { describe, expect, it } from 'vitest';
import type { ModuleDefinition } from '@coldmind/inject/src/core/cminject.module.ts';
import { LifecycleError, MutationConflictError } from '@coldmind/inject/src/core/cminject.errors.ts';
import { ModuleRegistry, type ModuleOwnedBinding } from '../evidence/prio-one/module-loader-prep/engine.ts';

function gate() {
  let release!: () => void;
  const promise = new Promise<void>(resolve => { release = resolve; });
  return { promise, release };
}
function module(name: string, imports: readonly ModuleDefinition[] = []): ModuleDefinition {
  return { name, imports, register: () => undefined };
}
function harness() {
  const order: string[] = [];
  const live = new Set<string>();
  const counts = new Map<ModuleDefinition, number>();
  const registration = new Map<ModuleDefinition, () => void | Promise<void>>();
  const removal = new Map<ModuleDefinition, () => void | Promise<void>>();
  const bindingCounts = new Map<ModuleDefinition, number>();
  let disposed = false;
  const registry = new ModuleRegistry({
    active() { if (disposed) throw new LifecycleError('root ended'); },
    cycleError(path) { const error = new Error(`module-cycle: ${path.join(' -> ')}`); error.name = 'ModuleCycle'; return error; },
    async runRegistration(definition) {
      const generation = (counts.get(definition) ?? 0) + 1;
      counts.set(definition, generation); order.push(`register:${definition.name}`);
      await registration.get(definition)?.();
      const handles: ModuleOwnedBinding[] = [];
      for (let index = 0; index < (bindingCounts.get(definition) ?? 1); index++) {
        const key = `${definition.name}:${generation}:${index}`;
        live.add(key);
        handles.push({ async unregister() {
          if (!live.delete(key)) return false;
          order.push(`remove:${key}`);
          await removal.get(definition)?.();
          return true;
        } });
      }
      return handles;
    },
  });
  return { registry, order, live, counts, registration, removal, bindingCounts, dispose() { disposed = true; registry.invalidate(); } };
}

describe('prepared root module identity/ownership engine', () => {
  it('loads once by identity and returns the same active explicit handle', async () => {
    const h = harness(); const one = module('one');
    const [first] = await h.registry.load(one);
    const [second] = await h.registry.load(one);
    expect(first).toBe(second); expect(h.counts.get(one)).toBe(1);
    await first!.unload(); await second!.unload();
    expect(h.live.size).toBe(0); expect(h.order).toEqual(['register:one', 'remove:one:1:0']);
  });
  it('does not confuse different definitions with the same name', async () => {
    const h = harness(); const one = module('same'); const two = module('same');
    const handles = await h.registry.load(one, two);
    expect(handles[0]).not.toBe(handles[1]); expect(h.counts.get(one)).toBe(1); expect(h.counts.get(two)).toBe(1);
  });
  it('loads imports depth first in declaration order and unloads dependents first', async () => {
    const h = harness(); const leaf = module('leaf'); const first = module('first', [leaf]); const second = module('second');
    const root = module('root', [first, second]); const [handle] = await h.registry.load(root);
    expect(h.order).toEqual(['register:leaf', 'register:first', 'register:second', 'register:root']);
    await handle!.unload();
    expect(h.order.slice(4)).toEqual(['remove:root:1:0', 'remove:second:1:0', 'remove:first:1:0', 'remove:leaf:1:0']);
  });
  it('joins concurrent pending loads of one identity', async () => {
    const h = harness(); const one = module('one'); const entered = gate(); const finish = gate();
    h.registration.set(one, async () => { entered.release(); await finish.promise; });
    const first = h.registry.load(one); const second = h.registry.load(one); await entered.promise;
    expect(h.counts.get(one)).toBe(1); finish.release();
    expect((await first)[0]).toBe((await second)[0]);
  });
  it('allows independent registrations to overlap rather than serializing the entire root', async () => {
    const h = harness(); const slow = module('slow'); const fast = module('fast'); const entered = gate(); const finish = gate();
    h.registration.set(slow, async () => { entered.release(); await finish.promise; });
    const pending = h.registry.load(slow); await entered.promise;
    const [handle] = await h.registry.load(fast); expect(h.counts.get(fast)).toBe(1);
    await handle!.unload(); expect(h.live.size).toBe(0); finish.release(); await pending;
  });
  it('rejects import cycles before executing registration or creating pending waits', async () => {
    const h = harness(); const imports: ModuleDefinition[] = []; const a = module('A', imports); const b = module('B', [a]); imports.push(b);
    await expect(h.registry.load(a)).rejects.toMatchObject({ name: 'ModuleCycle', message: 'module-cycle: A -> B -> A' });
    expect(h.order).toEqual([]);
    await expect(Promise.all([h.registry.load(a), h.registry.load(b)])).rejects.toMatchObject({ name: 'ModuleCycle' });
    expect(h.order).toEqual([]);
  });
  it('retains shared imports until the last importing owner unloads', async () => {
    const h = harness(); const shared = module('shared'); const a = module('A', [shared]); const b = module('B', [shared]);
    const [first, second] = await h.registry.load(a, b); expect(h.counts.get(shared)).toBe(1);
    await first!.unload(); expect([...h.live].sort()).toEqual(['B:1:0', 'shared:1:0']);
    await second!.unload(); expect(h.live.size).toBe(0);
  });
  it('retains an explicitly loaded import independently of its importers', async () => {
    const h = harness(); const shared = module('shared'); const a = module('A', [shared]);
    const [first] = await h.registry.load(a); const [explicit] = await h.registry.load(shared);
    await first!.unload(); expect([...h.live]).toEqual(['shared:1:0']);
    await explicit!.unload(); expect(h.live.size).toBe(0);
  });
  it('does not accumulate hidden references from duplicate imports or duplicate loads', async () => {
    const h = harness(); const shared = module('shared'); const a = module('A', [shared, shared]);
    const [first, duplicate] = await h.registry.load(a, a); expect(first).toBe(duplicate);
    await first!.unload(); expect(h.live.size).toBe(0); expect(h.counts.get(shared)).toBe(1);
  });
  it('rolls back successful imports on parent failure and permits a fresh retry', async () => {
    const h = harness(); const dependency = module('dependency'); const a = module('A', [dependency]); const error = new Error('failed callback');
    h.registration.set(a, () => { throw error; });
    await expect(h.registry.load(a)).rejects.toBe(error); expect(h.live.size).toBe(0);
    expect(h.order).toEqual(['register:dependency', 'register:A', 'remove:dependency:1:0']);
    h.registration.delete(a); const [retry] = await h.registry.load(a);
    expect(h.counts.get(a)).toBe(2); expect(h.counts.get(dependency)).toBe(2); await retry!.unload(); expect(h.live.size).toBe(0);
  });
  it('does not roll back an import retained by an unrelated concurrent parent', async () => {
    const h = harness(); const shared = module('shared'); const a = module('A', [shared]); const b = module('B', [shared]);
    const entered = gate(); const fail = gate();
    h.registration.set(a, async () => { entered.release(); await fail.promise; throw new Error('fail A'); });
    const failed = expect(h.registry.load(a)).rejects.toThrow('fail A'); await entered.promise;
    const [second] = await h.registry.load(b); fail.release(); await failed;
    expect([...h.live].sort()).toEqual(['B:1:0', 'shared:1:0']); await second!.unload(); expect(h.live.size).toBe(0);
  });
  it('preserves a pre-existing explicit import when a new parent fails', async () => {
    const h = harness(); const shared = module('shared'); const [explicit] = await h.registry.load(shared); const bad = module('bad', [shared]);
    h.registration.set(bad, () => { throw new Error('bad'); }); await expect(h.registry.load(bad)).rejects.toThrow('bad');
    expect([...h.live]).toEqual(['shared:1:0']); await explicit!.unload(); expect(h.live.size).toBe(0);
  });
  it('removes only owned bindings in reverse registration order', async () => {
    const h = harness(); const one = module('one'); h.bindingCounts.set(one, 3); h.live.add('unrelated');
    const [handle] = await h.registry.load(one); await handle!.unload();
    expect([...h.live]).toEqual(['unrelated']);
    expect(h.order.slice(1)).toEqual(['remove:one:1:2', 'remove:one:1:1', 'remove:one:1:0']);
  });
  it('aggregates removal errors while still removing all own and imported bindings', async () => {
    const h = harness(); const dependency = module('dependency'); const one = module('one', [dependency]); h.bindingCounts.set(one, 2);
    const first = new Error('dispose one'); const second = new Error('dispose dependency');
    h.removal.set(one, () => { throw first; }); h.removal.set(dependency, () => { throw second; });
    const [handle] = await h.registry.load(one); let failure: unknown;
    try { await handle!.unload(); } catch (error) { failure = error; }
    expect(failure).toBeInstanceOf(AggregateError); expect(h.live.size).toBe(0);
    expect(h.order.slice(2)).toEqual(['remove:one:1:1', 'remove:one:1:0', 'remove:dependency:1:0']);
    await expect(handle!.unload()).rejects.toBe(failure);
  });
  it('waits for an unloading generation before registering the next one', async () => {
    const h = harness(); const one = module('one'); const entered = gate(); const finish = gate();
    h.removal.set(one, async () => { entered.release(); await finish.promise; });
    const [first] = await h.registry.load(one); const unloading = first!.unload(); await entered.promise;
    const loading = h.registry.load(one); await Promise.resolve(); expect(h.counts.get(one)).toBe(1);
    finish.release(); await unloading; const [second] = await loading;
    expect(second).not.toBe(first); expect(h.counts.get(one)).toBe(2);
    await first!.unload(); expect([...h.live]).toEqual(['one:2:0']); await second!.unload(); expect(h.live.size).toBe(0);
  });
  it('creates a fresh explicit handle when an imported module is explicitly reacquired', async () => {
    const h = harness(); const shared = module('shared'); const a = module('A', [shared]); const [parent] = await h.registry.load(a);
    const [first] = await h.registry.load(shared); await first!.unload();
    const [second] = await h.registry.load(shared); expect(second).not.toBe(first);
    await first!.unload(); await parent!.unload(); expect([...h.live]).toEqual(['shared:1:0']);
    await second!.unload(); expect(h.live.size).toBe(0);
  });
  it('rejects mutation of a loaded graph but unloads using its captured ownership graph', async () => {
    const h = harness(); const imports: ModuleDefinition[] = []; const one = module('one', imports); const [handle] = await h.registry.load(one);
    imports.push(module('later'));
    await expect(h.registry.load(one)).rejects.toBeInstanceOf(MutationConflictError);
    await handle!.unload(); expect(h.live.size).toBe(0);
    await h.registry.load(one); expect(h.order).toContain('register:later');
  });
  it('detects mutation during async registration and removes returned owned bindings', async () => {
    const h = harness(); const imports: ModuleDefinition[] = []; const one = module('one', imports);
    h.registration.set(one, async () => { await Promise.resolve(); imports.push(module('late')); });
    await expect(h.registry.load(one)).rejects.toBeInstanceOf(MutationConflictError);
    expect(h.live.size).toBe(0); expect(h.order).toContain('remove:one:1:0');
  });
  it('invalidates handles without double-unregistering root-owned bindings', async () => {
    const h = harness(); const one = module('one'); const [handle] = await h.registry.load(one);
    h.dispose(); await handle!.unload(); await handle!.unload();
    expect(h.order).toEqual(['register:one']); expect([...h.live]).toEqual(['one:1:0']);
    await expect(h.registry.load(one)).rejects.toBeInstanceOf(LifecycleError);
  });
  it('rejects completion after root shutdown and delegates late transaction rollback to the host', async () => {
    const h = harness(); const one = module('one'); const entered = gate(); const finish = gate();
    h.registration.set(one, async () => { entered.release(); await finish.promise; throw new LifecycleError('host rolled back root-ended transaction'); });
    const rejected = expect(h.registry.load(one)).rejects.toThrow('host rolled back'); await entered.promise;
    h.dispose(); finish.release(); await rejected; expect(h.live.size).toBe(0); expect(h.order).toEqual(['register:one']);
  });
  it('keeps identical definitions isolated between root registries', async () => {
    const first = harness(); const second = harness(); const one = module('one');
    const [a] = await first.registry.load(one); const [b] = await second.registry.load(one);
    expect(a).not.toBe(b); await a!.unload(); expect(second.live.size).toBe(1); await b!.unload();
    expect(first.live.size).toBe(0); expect(second.live.size).toBe(0);
  });
  it('reports original registration failure and all imported cleanup failures', async () => {
    const h = harness(); const imported = module('imported'); const parent = module('parent', [imported]);
    const original = new Error('register failed'); const cleanup = new Error('cleanup failed');
    h.registration.set(parent, () => { throw original; }); h.removal.set(imported, () => { throw cleanup; });
    let failed: unknown;
    try { await h.registry.load(parent); } catch (error) { failed = error; }
    expect(failed).toBeInstanceOf(AggregateError);
    if (!(failed instanceof AggregateError)) throw new Error('Expected aggregate');
    expect(failed.errors[0]).toBe(original); expect(failed.errors[1]).toBeInstanceOf(AggregateError);
    expect(h.live.size).toBe(0);
  });
  it('rejects a newly introduced cycle while the original immutable graph is loading', async () => {
    const h = harness(); const imports: ModuleDefinition[] = []; const child = module('child', imports); const parent = module('parent', [child]);
    const entered = gate(); const finish = gate(); h.registration.set(child, async () => { entered.release(); await finish.promise; });
    const rejected = expect(h.registry.load(parent)).rejects.toBeInstanceOf(MutationConflictError); await entered.promise;
    imports.push(parent);
    await expect(h.registry.load(child)).rejects.toBeInstanceOf(MutationConflictError);
    finish.release(); await rejected; expect(h.live.size).toBe(0);
  });
  it('treats a multi-module load as independent identities rather than silently rolling back successful siblings', async () => {
    const h = harness(); const good = module('good'); const bad = module('bad');
    h.registration.set(bad, () => { throw new Error('bad'); });
    await expect(h.registry.load(good, bad)).rejects.toThrow('bad');
    const [goodHandle] = await h.registry.load(good); expect(h.counts.get(good)).toBe(1);
    await goodHandle!.unload(); expect(h.live.size).toBe(0);
  });
  it('allows a fresh load after a completed unload reported disposal errors', async () => {
    const h = harness(); const one = module('one'); h.removal.set(one, () => { throw new Error('dispose'); });
    const [first] = await h.registry.load(one); await expect(first!.unload()).rejects.toThrow('Module unload failed');
    h.removal.delete(one); const [second] = await h.registry.load(one);
    expect(h.counts.get(one)).toBe(2); await second!.unload(); expect(h.live.size).toBe(0);
  });
  it('starts no further module-owned removal callbacks once root invalidation transfers teardown ownership', async () => {
    const h = harness(); const one = module('one'); h.bindingCounts.set(one, 3);
    const entered = gate(); const finish = gate(); h.removal.set(one, async () => { entered.release(); await finish.promise; });
    const [handle] = await h.registry.load(one); const unloading = handle!.unload(); await entered.promise;
    h.dispose(); finish.release(); await unloading;
    expect(h.order).toEqual(['register:one', 'remove:one:1:2']);
    expect([...h.live].sort()).toEqual(['one:1:0', 'one:1:1']);
  });

});
