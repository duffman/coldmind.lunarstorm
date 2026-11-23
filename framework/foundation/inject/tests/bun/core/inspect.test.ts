import { expect, test } from 'vitest';
import { Container, createToken } from '@coldmind/inject';
import { inspect } from '@coldmind/inject/inspect';

test('inspect returns a detached snapshot without building registrations or mutating the registry', async () => {
  const c = new Container(); let builds = 0;
  const handle = c.singleton('inspect-cold', () => { builds++; return {}; }); const epoch = c.getRegistryEpoch();
  const snapshot = inspect(c);
  expect(Object.keys(snapshot).sort()).toEqual(['dependencies', 'drift', 'issues', 'resources']);
  expect(snapshot.dependencies.nodes).toEqual([expect.objectContaining({ bindingId: handle.bindingId, token: '"inspect-cold"', generation: 0 })]);
  expect(snapshot.resources.resources).toEqual([]); expect(snapshot.issues).toEqual([]);
  snapshot.dependencies.nodes.length = 0; snapshot.resources.scopes.length = 0;
  expect(inspect(c).dependencies.nodes).toHaveLength(1); expect(inspect(c).resources.scopes).toHaveLength(1);
  expect(builds).toBe(0); expect(c.getRegistryEpoch()).toBe(epoch);
  await c.dispose();
});

test('inspect distinguishes declared and observed binding edges and links live resource identities', async () => {
  const c = new Container(); const base = createToken<object>('inspect-base'); const consumer = createToken<object>('inspect-consumer');
  const dependency = c.singleton(base, () => ({}));
  const dependent = c.singleton(consumer, async c => ({ base: await c.resolve(base) }), { deps: [base] });
  expect(inspect(c).dependencies.edges).toContainEqual(expect.objectContaining({ from: dependent.bindingId, to: dependency.bindingId, type: 'declared' }));
  await c.resolve(consumer); const snapshot = inspect(c);
  expect(snapshot.dependencies.edges).toContainEqual(expect.objectContaining({ from: dependent.bindingId, to: dependency.bindingId, type: 'runtime' }));
  const from = snapshot.resources.resources.find(value => value.bindingId === dependent.bindingId)!;
  const to = snapshot.resources.resources.find(value => value.bindingId === dependency.bindingId)!;
  expect(snapshot.resources.resourceEdges).toContainEqual(expect.objectContaining({ from: from.instanceId, to: to.instanceId, type: 'runtime' }));
  expect(snapshot.resources.bindings).toEqual(snapshot.dependencies.nodes);
  await c.dispose();
});

test('dependency trees retain nested declared topology without resolving services', async () => {
  const c = new Container(); const leaf = c.singleton('inspect-leaf', () => ({}));
  const middle = c.singleton('inspect-middle', () => ({}), { deps: ['inspect-leaf'] });
  const top = c.singleton('inspect-top', () => ({}), { deps: ['inspect-middle'] });
  expect(c.getDependencyTree('inspect-top')).toEqual([{ bindingId: top.bindingId, token: '"inspect-top"', scopeId: c.getScopeId(), children: [
    { bindingId: middle.bindingId, token: '"inspect-middle"', scopeId: c.getScopeId(), children: [
      { bindingId: leaf.bindingId, token: '"inspect-leaf"', scopeId: c.getScopeId(), children: [] },
    ] },
  ] }]);
  expect(c.cachedBindings()).toEqual([]); expect(c.getDependencyTree('inspect-absent')).toEqual([]);
  await c.dispose();
});

test('inspect reports external and owned resources with their holding scope and ownership records', async () => {
  const c = new Container(); const request = c.createTenantScope('inspect-tenant').createRequestScope('inspect-request');
  const external = c.registerInstance('inspect-external', {}, { ownership: 'external', reason: 'caller' });
  const owned = c.request('inspect-owned', () => ({})); await request.resolve('inspect-owned');
  const snapshot = inspect(request);
  expect(snapshot.resources.resources).toEqual(expect.arrayContaining([
    expect.objectContaining({ bindingId: external.bindingId, ownership: 'external', resolutionScopeId: c.getScopeId() }),
    expect.objectContaining({ bindingId: owned.bindingId, ownership: 'owned', ownerScopeId: c.getScopeId(), resolutionScopeId: request.getScopeId(), tenantId: 'inspect-tenant', requestId: 'inspect-request' }),
  ]));
  expect(request.ownershipRecords()).toContainEqual(expect.objectContaining({ bindingId: owned.bindingId, scopeId: request.getScopeId(), generation: 0 }));
  expect(snapshot.resources.scopes).toHaveLength(3);
  await c.dispose();
});

test('registry epochs change on registration and unregister but not ordinary resolution or inspection', async () => {
  const c = new Container(); const initial = c.getRegistryEpoch();
  const handle = c.singleton('inspect-epoch', () => ({})); const registered = c.getRegistryEpoch();
  expect(registered).toBeGreaterThan(initial);
  const value = await c.resolve('inspect-epoch'); await c.resolve('inspect-epoch');
  expect(c.get('inspect-epoch')).toBe(value); expect(c.getRegistryEpoch()).toBe(registered);
  expect(c.epochSnapshot()).toEqual(expect.objectContaining({ epoch: registered, ownerScopeId: c.getScopeId() }));
  expect(inspect(c).resources.epoch).toBe(registered); expect(c.getRegistryEpoch()).toBe(registered);
  await handle.unregister(); expect(c.getRegistryEpoch()).toBeGreaterThan(registered);
  expect(inspect(c).dependencies.nodes).toEqual([]); expect(inspect(c).resources.resources).toEqual([]);
  await c.dispose();
});

test('inspect exposes declared graph issues without throwing validation errors', async () => {
  const c = new Container(); c.singleton('inspect-broken', () => ({}), { deps: ['inspect-missing'] });
  const snapshot = inspect(c);
  expect(snapshot.issues).toContainEqual(expect.objectContaining({ kind: 'missing' }));
  expect(snapshot.dependencies.nodes).toHaveLength(1); expect(snapshot.resources.resources).toEqual([]);
  await c.dispose();
});
