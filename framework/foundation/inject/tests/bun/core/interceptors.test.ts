import { expect, test } from 'vitest';
import { Container, createToken, type IContainer, type ResolutionContext } from '@coldmind/inject';

test.each(['sync', 'async'] as const)('interceptors run parent then child before and after %s resolution including cache hits', async mode => {
  const root = new Container(); const child = root.createScope(); const events: string[] = []; let builds = 0;
  root.singleton('intercept-value', () => { builds++; return {}; });
  root.addInterceptor({ onBeforeResolve: () => { events.push('root-before'); }, onAfterResolve: (_c, _value, duration) => { expect(duration).toBeGreaterThanOrEqual(0); events.push('root-after'); } });
  child.addInterceptor({ onBeforeResolve: () => { events.push('child-before'); }, onAfterResolve: () => { events.push('child-after'); } });
  const first = mode === 'sync' ? child.resolveSync('intercept-value') : await child.resolve('intercept-value');
  const second = mode === 'sync' ? child.resolveSync('intercept-value') : await child.resolve('intercept-value');
  expect(second).toBe(first); expect(builds).toBe(1);
  expect(events).toEqual(['root-before', 'child-before', 'root-after', 'child-after', 'root-before', 'child-before', 'root-after', 'child-after']);
  await root.dispose();
});

test('nested interceptors receive binding paths, requester identity and original factory data', async () => {
  const root = new Container(); const child = root.createScope(); const seen: ResolutionContext[] = [];
  const inner = createToken<number>('intercept-inner'); const outer = createToken<number>('intercept-outer'); const data = { value: 7 };
  const dependency = root.transient(inner, (_c: IContainer, input: { value: number }) => input.value);
  const dependent = root.scoped(outer, c => c.resolve(inner, data), { deps: [inner] });
  child.addInterceptor({ onBeforeResolve: context => { seen.push(context); } });
  expect(await child.resolve(outer)).toBe(7);
  expect(seen).toHaveLength(2); expect(seen[0]!.requester).toBeUndefined();
  expect(seen[0]!.path).toEqual([expect.objectContaining({ bindingId: dependent.bindingId, scopeId: child.getScopeId() })]);
  expect(seen[1]!.requester).toEqual(expect.objectContaining({ bindingId: dependent.bindingId, scopeId: child.getScopeId() }));
  expect(seen[1]!.path?.map(step => step.bindingId)).toEqual([dependent.bindingId, dependency.bindingId]);
  expect(seen[1]!.data).toBe(data); expect(seen[1]!.attempt).toBe(1);
  await root.dispose();
});

test.each(['sync', 'async'] as const)('error-hook failures attach cause without replacing the %s factory error', async mode => {
  const root = new Container(); const original = new Error('factory'); const hook = new Error('observer'); const errors: Error[] = [];
  root.transient('intercept-error', () => { throw original; });
  root.addInterceptor({ onResolutionError: (_ctx, error) => { errors.push(error); if (mode === 'async') return Promise.reject(hook); throw hook; } });
  if (mode === 'sync') expect(() => root.resolveSync('intercept-error')).toThrow(original);
  else await expect(root.resolve('intercept-error')).rejects.toBe(original);
  expect(errors).toEqual([original]); expect(original.cause).toBe(hook);
  await root.dispose();
});

test('initializeOwned suppresses interceptor callbacks through nested dependency builds', async () => {
  const root = new Container(); const events: string[] = [];
  root.singleton('intercept-dependency', () => ({})); root.singleton('intercept-owner', c => c.resolve('intercept-dependency'));
  root.addInterceptor({ onBeforeResolve: () => { events.push('before'); }, onAfterResolve: () => { events.push('after'); } });
  await root.initializeOwned(); expect(root.cachedBindings()).toHaveLength(2); expect(events).toEqual([]);
  await root.resolve('intercept-owner'); expect(events).toEqual(['before', 'after']); await root.dispose();
});

test('removing a child interceptor leaves parent observation active', async () => {
  const root = new Container(); const child = root.createScope(); const events: string[] = [];
  root.singleton('intercept-remove', () => 1);
  root.addInterceptor({ onBeforeResolve: () => { events.push('root'); } });
  const interceptor = { onBeforeResolve: () => { events.push('child'); } }; child.addInterceptor(interceptor);
  expect(child.removeInterceptor(interceptor)).toBe(true); expect(child.removeInterceptor(interceptor)).toBe(false);
  await child.resolve('intercept-remove'); expect(events).toEqual(['root']); await root.dispose();
});
