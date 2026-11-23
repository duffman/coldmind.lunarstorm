import { expect, test } from 'vitest';
import { Container, normalizeToken } from '@coldmind/inject';

test('health uses the lifecycle hook before the instance fallback and passes the exact instance', async () => {
  const root = new Container(); const calls: unknown[] = [];
  const value = { onHealthCheck: () => { throw new Error('fallback must not run'); } };
  root.singleton('health-lifecycle', { factory: () => value, onHealthCheck: instance => { calls.push(instance); return { healthy: true }; } });
  await root.resolve('health-lifecycle'); expect(await root.healthCheck('health-lifecycle')).toEqual({ healthy: true }); expect(calls).toEqual([value]);
  await root.dispose();
});

test('health falls back to an instance method with its receiver and accepts objects without hooks', async () => {
  const root = new Container(); const value = { healthy: false, onHealthCheck() { return { healthy: this.healthy, error: 'instance status' }; } };
  root.registerInstance('health-instance', value); root.registerInstance('health-default', {});
  expect(await root.healthCheck('health-instance')).toEqual({ healthy: false, error: 'instance status' });
  value.healthy = true; expect(await root.healthCheck('health-instance')).toEqual({ healthy: true });
  expect(await root.healthCheck('health-default')).toEqual({ healthy: true }); await root.dispose();
});

test('health distinguishes missing and uninitialized services without invoking factories', async () => {
  const root = new Container(); let builds = 0; root.singleton('health-cold', () => { builds++; return {}; });
  expect(await root.healthCheck('health-missing')).toEqual({ healthy: false, error: 'Service not registered' });
  expect(await root.healthCheck('health-cold')).toEqual({ healthy: false, error: 'Service not initialized' });
  expect(builds).toBe(0); await root.dispose();
});

test('health traverses descendant scoped instances while a child excludes sibling scopes', async () => {
  const root = new Container(); const good = root.createScope(); const bad = root.createScope();
  root.scoped('health-scoped', c => ({ onHealthCheck: () => ({ healthy: c.getScopeId() === good.getScopeId(), error: 'sibling unhealthy' }) }));
  await good.resolve('health-scoped'); await bad.resolve('health-scoped');
  expect(await root.healthCheck('health-scoped')).toEqual({ healthy: false, error: 'sibling unhealthy' });
  expect(await good.healthCheck('health-scoped')).toEqual({ healthy: true });
  await bad.dispose(); expect(await root.healthCheck('health-scoped')).toEqual({ healthy: true }); await root.dispose();
});

test('health awaits async checks and aggregates rejected, thrown and explicit unhealthy multi results', async () => {
  const root = new Container(); const checked: number[] = [];
  root.singletonMulti('health-multi', { factory: () => ({}), onHealthCheck: async () => { await Promise.resolve(); checked.push(1); return { healthy: false, error: 'first' }; } });
  root.singletonMulti('health-multi', { factory: () => ({}), onHealthCheck: async () => { checked.push(2); throw new Error('second'); } });
  root.singletonMulti('health-multi', { factory: () => ({}), onHealthCheck: () => { checked.push(3); throw new Error('third'); } });
  root.singletonMulti('health-multi', { factory: () => ({}), onHealthCheck: () => { checked.push(4); return { healthy: false }; } });
  await root.resolveAll('health-multi');
  expect(await root.healthCheck('health-multi')).toEqual({ healthy: false, error: 'first; second; third; Unhealthy' });
  expect(checked).toEqual([1, 2, 3, 4]); await root.dispose();
});

test('healthCheckAll reports local registration keys and does not materialize cold services', async () => {
  const root = new Container(); const child = root.createScope(); root.registerInstance('health-parent', {});
  child.registerInstance('health-local', {}); child.singleton('health-unbuilt', () => ({}));
  const statuses = await child.healthCheckAll();
  expect([...statuses.keys()]).toEqual([normalizeToken('health-local'), normalizeToken('health-unbuilt')]);
  expect(statuses.get(normalizeToken('health-local'))).toEqual({ healthy: true });
  expect(statuses.get(normalizeToken('health-unbuilt'))).toEqual({ healthy: false, error: 'Service not initialized' });
  expect(child.cachedBindings()).toHaveLength(1); await root.dispose();
});
