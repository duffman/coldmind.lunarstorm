import { DiRuntime } from '@coldmind/inject/server';
import { expect, test } from 'vitest';
import { CircularDependencyError, Container, DiGraphError, LifecycleError } from '@coldmind/inject';

test('a request service resolved while a tenant service is built is reported as captive', async () => {
  const root = new Container();
  root.request('r', () => ({}));
  root.tenant('t', c => c.resolve('r'));
  const request = root.createTenantScope().createRequestScope();
  await expect(request.resolve('t')).rejects.toBeInstanceOf(DiGraphError);
});

test('a tenant service resolved while a singleton is built is reported as captive', async () => {
  const root = new Container();
  root.tenant('t', () => ({}));
  root.singleton('s', c => c.resolve('t'));
  const tenant = root.createTenantScope();
  await expect(tenant.resolve('s')).rejects.toBeInstanceOf(DiGraphError);
});

test('a request service built from a request service is not captive', async () => {
  const root = new Container();
  root.request('inner', () => ({}));
  root.request('outer', c => c.resolve('inner'));
  const request = root.createRequestScope();
  expect(await request.resolve('outer')).toBeDefined();
});

test('data that differs from the data a shared instance was built with is rejected', async () => {
  const root = new Container();
  root.singleton('s', (_c, data: { id: number }) => ({ id: data.id }));
  const first = { id: 1 };
  expect(await root.resolve('s', first)).toStrictEqual({ id: 1 });
  expect(await root.resolve('s', first)).toStrictEqual({ id: 1 });
  expect(await root.resolve('s')).toStrictEqual({ id: 1 });
  await expect(root.resolve('s', { id: 2 })).rejects.toBeInstanceOf(LifecycleError);
});

test('a frozen thenable service cannot be resolved through a promise, but resolveSync returns it', async () => {
  const value = Object.freeze({ then: () => undefined });
  const c = new Container();
  c.singleton('t', () => value);
  await expect(c.resolve('t')).rejects.toBeInstanceOf(LifecycleError);
  expect(c.resolveSync('t')).toBe(value);
});

test('a resolve through a captured container joins the running factory, so a cycle is reported, not deadlocked', async () => {
  const root = new Container();
  const runtime = new DiRuntime(root);
  root.singleton('a', async () => ({ b: await root.resolve('b') }));
  root.singleton('b', async () => ({ a: await root.resolve('a') }));
  await expect(root.resolve('a')).rejects.toBeInstanceOf(CircularDependencyError);
  await runtime.dispose();
});
