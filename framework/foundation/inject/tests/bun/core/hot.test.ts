import { expect, test } from 'vitest';
import { Container, createToken, ServiceLifetime } from '@coldmind/inject';
import { hotReplace } from '@coldmind/inject/hot';

/** Versioned resource used by replacement characterizations. */
interface Version { /** Factory version. */ version: number }

test('hot replacement builds a cold registration and advances its generation', async () => {
  const c = new Container(); const token = createToken<Version>('hot-cold');
  const original = c.singleton(token, () => ({ version: 1 }));
  const handle = await hotReplace(c, token, () => ({ version: 2 }), ServiceLifetime.Singleton);
  expect(c.get(token)).toEqual({ version: 2 });
  expect(handle.bindingId).not.toBe(original.bindingId);
  expect(handle.generation).toBe(original.generation + 1);
  expect(handle.retainedInstances).toEqual([]);
  await c.dispose();
});

test('hot replacement rebuilds the live dependent chain against the candidate before disposing old instances', async () => {
  const c = new Container(); const events: string[] = [];
  const base = createToken<Version>('hot-base');
  const middle = createToken<{ base: Version }>('hot-middle');
  const top = createToken<{ middle: { base: Version } }>('hot-top');
  c.singleton(base, { factory: () => ({ version: 1 }), onDispose: () => { events.push('dispose-base'); } });
  c.singleton(middle, { factory: async c => { const value = await c.resolve(base); events.push(`middle-${value.version}`); return { base: value }; }, onDispose: () => { events.push('dispose-middle'); } }, { deps: [base] });
  c.singleton(top, { factory: async c => { const value = await c.resolve(middle); events.push(`top-${value.base.version}`); return { middle: value }; }, onDispose: () => { events.push('dispose-top'); } }, { deps: [middle] });
  const before = await c.resolve(top); events.length = 0;
  await hotReplace(c, base, () => { events.push('base-2'); return { version: 2 }; }, ServiceLifetime.Singleton);
  expect(c.get(top)).not.toBe(before);
  expect(c.get(top).middle).toBe(c.get(middle));
  expect(c.get(middle).base).toBe(c.get(base));
  expect(events.slice(0, 3)).toEqual(['base-2', 'middle-2', 'top-2']);
  expect(events.slice(3).sort()).toEqual(['dispose-base', 'dispose-middle', 'dispose-top']);
  await c.dispose();
});

test('hot replacement disposes the old owned instance exactly once and the candidate at final disposal', async () => {
  const c = new Container(); const token = createToken<Version>('hot-owned'); const disposed: number[] = [];
  c.singleton(token, { factory: () => ({ version: 1 }), onDispose: value => { disposed.push(value.version); } });
  await c.resolve(token);
  await hotReplace(c, token, { factory: () => ({ version: 2 }), onDispose: value => { disposed.push(value.version); } }, ServiceLifetime.Singleton);
  expect(disposed).toEqual([1]);
  await c.dispose(); await c.dispose();
  expect(disposed).toEqual([1, 2]);
});

test('hot replacement rolls back a failed candidate without changing the old binding or instance', async () => {
  const c = new Container(); const token = createToken<Version>('hot-failed'); const error = new Error('candidate');
  const original = c.singleton(token, () => ({ version: 1 })); const value = await c.resolve(token);
  await expect(hotReplace(c, token, () => { throw error; }, ServiceLifetime.Singleton)).rejects.toBe(error);
  expect(c.bindingOf(token)?.id).toBe(original.bindingId);
  expect(c.bindingOf(token)?.generation).toBe(0);
  expect(c.get(token)).toBe(value);
  await c.dispose();
});

test('failed dependent rebuild restores original instances and cleans up the staged candidate', async () => {
  const c = new Container(); const base = createToken<Version>('hot-rollback'); const dependent = createToken<Version>('hot-dependent');
  let fail = false; let disposed = 0;
  c.singleton(base, () => ({ version: 1 }));
  c.singleton(dependent, async c => { const value = await c.resolve(base); if (fail) throw new Error('dependent'); return value; }, { deps: [base] });
  const previous = await c.resolve(dependent); const epoch = c.getRegistryEpoch(); fail = true;
  await expect(hotReplace(c, base, { factory: () => ({ version: 2 }), onDispose: () => { disposed++; } }, ServiceLifetime.Singleton)).rejects.toThrow('dependent');
  expect(c.get(base)).toBe(previous); expect(c.get(dependent)).toBe(previous);
  expect(c.getRegistryEpoch()).toBe(epoch); expect(disposed).toBe(1);
  await c.dispose();
});

test('hot replacement preserves external ownership for both original and candidate', async () => {
  const c = new Container(); let disposed = 0; const token = createToken<Version>('hot-external');
  const original = { version: 1, onDispose: () => { disposed++; } };
  c.registerInstance(token, original, { ownership: 'external', reason: 'caller' });
  await c.resolve(token);
  await hotReplace(c, token, { factory: () => ({ version: 2 }), onDispose: () => { disposed++; } }, ServiceLifetime.Singleton);
  expect(c.bindingOf(token)?.owner).toEqual({ ownership: 'external', reason: 'caller' });
  expect(c.get(token).version).toBe(2);
  await c.dispose(); expect(disposed).toBe(0);
});
