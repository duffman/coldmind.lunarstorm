import { expect, test } from 'vitest';
import { Container } from '@coldmind/inject/src/core/cminject.container.ts';
import { LifecycleError } from '@coldmind/inject/src/core/cminject.errors.ts';
import { createToken, delay } from '@coldmind/inject/src/core/cminject.token.ts';
import { bindDeferredSlot, describeDependencySlot, isDeferredToken, isLazyToken, isProviderToken, lazy, provider, unwrapDependencySlot, type DeferredScope } from '@coldmind/inject/src/core/cminject.lazy.ts';

test('markers preserve class, delayed class, symbol and string identity without building', () => {
  class Service {}
  for (const token of [Service, delay(() => Service), createToken<Service>('service'), 'service']) {
    expect(unwrapDependencySlot(lazy(token))).toBe(token);
    expect(unwrapDependencySlot(provider(token))).toBe(token);
    expect(unwrapDependencySlot(token)).toBe(token);
  }
});

test('immutable nominal markers cannot be confused with arbitrary objects or copies', () => {
  const first = lazy('x');
  const second = provider('x');
  expect(Object.isFrozen(first)).toBe(true);
  expect(Object.isFrozen(second)).toBe(true);
  expect(isLazyToken(first)).toBe(true);
  expect(isProviderToken(first)).toBe(false);
  expect(isProviderToken(second)).toBe(true);
  for (const value of [undefined, null, {}, { token: 'x' }, { ...first }, 'x', () => {}]) expect(isDeferredToken(value)).toBe(false);
});

test('graph descriptors retain deferred tokens without performing graph validation', () => {
  const token = createToken<number>('n');
  expect(describeDependencySlot(token)).toEqual({ token, kind: 'eager' });
  expect(describeDependencySlot(lazy(token))).toEqual({ token, kind: 'lazy' });
  expect(describeDependencySlot(provider(token))).toEqual({ token, kind: 'provider' });
});

test('lazy and provider add no cache over ordinary transient and singleton lifetimes', async () => {
  const scope = new Container();
  let builds = 0;
  scope.transient('t', () => ({ id: ++builds }));
  scope.singleton('s', () => ({}));
  const deferred = bindDeferredSlot(scope, lazy('t'));
  const factory = bindDeferredSlot(scope, provider('t'));
  expect(deferred.peek()).toBeUndefined();
  expect(builds).toBe(0);
  expect(await deferred.get()).not.toBe(await deferred.get());
  expect(await factory()).not.toBe(await factory());
  expect(builds).toBe(4);
  const singleton = bindDeferredSlot(scope, lazy('s'));
  expect(singleton.peek()).toBeUndefined();
  const value = await singleton.get();
  expect(singleton.peek()).toBe(value);
  expect(singleton.getSync()).toBe(value);
  expect(await bindDeferredSlot(scope, provider('s'))()).toBe(value);
  await scope.dispose();
});

test('bound slots preserve originating scoped, tenant and request lifetimes', async () => {
  const root = new Container();
  root.scoped('scoped', () => ({}));
  root.tenant('tenant', () => ({}));
  root.request('request', () => ({}));
  const tenant = root.createTenantScope();
  const request = tenant.createRequestScope();
  const otherRequest = tenant.createRequestScope();
  for (const name of ['scoped', 'tenant', 'request']) {
    const first = await bindDeferredSlot(request, lazy(name)).get();
    expect(await bindDeferredSlot(request, provider(name))()).toBe(first);
    const other = await bindDeferredSlot(otherRequest, provider(name))();
    if (name === 'tenant') expect(other).toBe(first);
    else expect(other).not.toBe(first);
  }
  await root.dispose();
});

test('getSync builds synchronously while peek never builds even registered services', async () => {
  const root = new Container();
  let builds = 0;
  root.singleton('sync', () => ++builds);
  const deferred = bindDeferredSlot(root, lazy('sync'));
  expect(deferred.peek()).toBeUndefined();
  expect(builds).toBe(0);
  expect(deferred.getSync()).toBe(1);
  expect(deferred.peek()).toBe(1);
  await root.dispose();
});

test('getSync propagates the ordinary asynchronous build error', async () => {
  const root = new Container();
  root.singleton('async', async () => 42);
  const deferred = bindDeferredSlot(root, lazy('async'));
  expect(deferred.peek()).toBeUndefined();
  expect(() => deferred.getSync()).toThrow(LifecycleError);
  expect(await deferred.get()).toBe(42);
  await root.dispose();
});

test('failed builds and missing tokens preserve ordinary resolution diagnostics', async () => {
  const root = new Container();
  const failure = new Error('factory failed');
  root.transient('bad', async () => { throw failure; });
  await expect(bindDeferredSlot(root, lazy('bad')).get()).rejects.toBe(failure);
  await expect(bindDeferredSlot(root, provider('bad'))()).rejects.toBe(failure);
  await expect(bindDeferredSlot(root, lazy('missing')).get()).rejects.toThrow('missing');
  expect(bindDeferredSlot(root, lazy('missing')).peek()).toBeUndefined();
  await root.dispose();
});

test('disposed originating scopes reject reads and never fall back to the root', async () => {
  const root = new Container();
  root.singleton('x', () => ({}));
  const child = root.createScope();
  const deferred = bindDeferredSlot(child, lazy('x'));
  const factory = bindDeferredSlot(child, provider('x'));
  await deferred.get();
  await child.dispose();
  await expect(deferred.get()).rejects.toThrow(LifecycleError);
  await expect(factory()).rejects.toThrow(LifecycleError);
  expect(() => deferred.getSync()).toThrow(LifecycleError);
  expect(() => deferred.peek()).toThrow(LifecycleError);
  expect(await root.resolve('x')).toBeDefined();
  await root.dispose();
});

test('binding calls the provided public receiver without copying its resolver state', async () => {
  const root = new Container();
  let captured: DeferredScope | undefined;
  root.singleton('capture', view => { captured = view; return {}; });
  await root.resolve('capture');
  if (!captured) throw new Error('Expected captured view');
  root.singleton('later', () => 7);
  const deferred = bindDeferredSlot(captured, lazy('later'));
  expect(await deferred.get()).toBe(7);
  expect(deferred.getSync()).toBe(7);
  expect(deferred.peek()).toBe(7);
  await root.dispose();
});

test('declaring and describing delayed tokens never evaluates their getter', () => {
  let reads = 0;
  const token = delay(() => { reads++; return class Service {}; });
  expect(describeDependencySlot(lazy(token)).token).toBe(token);
  expect(describeDependencySlot(provider(token)).token).toBe(token);
  expect(reads).toBe(0);
});

test('a bound factory view retains active-frame cycle checks before completing', async () => {
  const root = new Container();
  root.singleton('cycle', async view => bindDeferredSlot(view, lazy('cycle')).get());
  await expect(root.resolve('cycle')).rejects.toThrow(/circular|cycle/i);
  await root.dispose();
});
