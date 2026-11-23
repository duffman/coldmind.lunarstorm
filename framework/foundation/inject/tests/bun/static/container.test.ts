import { expect, test } from 'vitest';
import {
  Container, ServiceLifetime, ScopeType, CircularDependencyError, MutationConflictError,
  DiGraphError, ScopeMissingError, LifecycleError, NotRegisteredError, DiGraph,
  delay, getGlobalToken, normalizeToken, clearTokenCaches, getTokenCacheSize,
} from '@coldmind/inject';
import { DiRuntime } from '@coldmind/inject/server';

test('token normalization, aliases, weak constructor mapping, cache count', async () => {
  clearTokenCaches();
  const before = getTokenCacheSize();
  const original = getGlobalToken('cached-token');
  expect(normalizeToken('cached-token')).toBe(original);
  expect(original).not.toBe(Symbol('cached-token'));
  expect(getTokenCacheSize()).toBe(before + 1);
  clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 1);
  expect(getGlobalToken('cached-token')).toBe(original);
  class A {}
  expect(normalizeToken(A)).toBe(normalizeToken(delay(() => A)));
  expect(() => normalizeToken(123 as never)).toThrow(/Invalid token type: number/);
});

test('singleton, scoped, tenant, request, transient route to correct owner', async () => {
  const c = new Container();
  c.singleton('singleton', owner => owner);
  c.scoped('scoped', () => ({}));
  c.tenant('tenant', () => ({}));
  c.request('request', () => ({}));
  c.transient('transient', () => ({ onDispose() { throw new Error('transient disposed'); } }));
  const a = c.createTenantScope('a'); const b = c.createTenantScope('b');
  const a1 = a.createRequestScope('a1'); const a2 = a.createRequestScope('a2');
  // The factory receives a resolution-bound view of its owner, not the container itself.
  expect((await a1.resolve<Container>('singleton')).identity.id).toBe(c.identity.id);
  expect(await a2.resolve('singleton')).toBe(await a1.resolve('singleton'));
  expect(await a1.resolve('tenant')).toBe(await a2.resolve('tenant'));
  expect(await a1.resolve('tenant')).not.toBe(await b.resolve('tenant'));
  expect(await a1.resolve('scoped')).toBe(await a1.resolve('scoped'));
  expect(await a1.resolve('scoped')).not.toBe(await a2.resolve('scoped'));
  expect(await a1.resolve('request')).not.toBe(await a2.resolve('request'));
  expect(await c.resolve('transient')).not.toBe(await c.resolve('transient'));
  await expect(c.resolve('tenant')).rejects.toThrow(expect.toSatisfy((e: unknown) => e instanceof ScopeMissingError && e.scopeKind === 'tenant'));
  await expect(a.resolve('request')).rejects.toThrow(expect.toSatisfy((e: unknown) => e instanceof ScopeMissingError && e.scopeKind === 'request'));
  await c.dispose();
});

test('50 concurrent resolutions share one singleton promise including synchronous throw', async () => {
  const c = new Container(); let attempts = 0;
  c.singleton('x', () => { attempts++; if (attempts === 1) throw new Error('first'); return 0; });
  const failed = await Promise.allSettled(Array.from({ length: 50 }, () => c.resolve('x')));
  expect(failed.filter(result => result.status === 'rejected').length).toBe(50);
  expect(attempts).toBe(1);
  expect(await c.resolve('x')).toBe(0);
  expect(c.get('x')).toBe(0);
  c.registerInstance('undefined', undefined);
  expect(c.get('undefined')).toBe(undefined);
  c.registerInstance('false', false);
  expect(c.get('false')).toBe(false);
  await c.dispose();
});

test('raw thenables are preserved instead of awaited', async () => {
  const c = new Container();
  let assimilations = 0;
  const value = { state: 'literal', then(onResolve: (v: unknown) => void) { assimilations++; onResolve('wrong'); } };
  c.singleton('thenable', () => value);
  const resolved = await c.resolve('thenable');
  expect(resolved).toBe(value);
  expect(assimilations).toBe(0);
  expect(resolved).toHaveProperty('then', value.then);
  await c.dispose();
});


test('single cycles, transient concurrency, captive dependencies, multi shadowing', async () => {
  const c = new Container(); c.transient('a', c => c.resolve('b')); c.transient('b', c => c.resolve('a'));
  await expect(c.resolve('a')).rejects.toThrow(expect.toSatisfy((e: unknown) => e instanceof CircularDependencyError && e.path.steps.length === 3));
  const parallel = new Container(); parallel.transient('t', async () => { await Promise.resolve(); return {}; });
  const both = await Promise.all([parallel.resolve('t'), parallel.resolve('t')]);
  expect(both[0]).not.toBe(both[1]);
  const captive = new Container(); captive.scoped('scoped', () => ({}));
  captive.singleton('one', c => c.resolve('scoped'));
  await expect(captive.resolve('one')).rejects.toThrow(expect.toSatisfy((e: unknown) => e instanceof DiGraphError && e.issues[0]?.kind === 'captive'));
  const multi = new Container(); multi.singletonMulti('x', () => 1);
  const child = multi.createScope(); child.singleton('x', () => 2);
  // A child single shadows the parent multi registration.
  expect(await child.resolve('x')).toBe(2);
  expect(await child.resolveAll('x')).toStrictEqual([1]);
  expect(await multi.resolveAll('unknown')).toStrictEqual([]);
  expect(await c.resolveAll('unknown')).toStrictEqual([]);
  await c.dispose(); await parallel.dispose(); await captive.dispose(); await multi.dispose();
});

test('multi resolution preserves ancestor order and sequential execution', async () => {
  const calls: number[] = []; const root = new Container();
  root.singletonMulti('x', async () => { calls.push(1); return 1; });
  root.singletonMulti('x', () => { calls.push(2); return 2; });
  const child = root.createScope(); child.singletonMulti('x', () => { calls.push(3); return 3; });
  const results = await child.resolveAll<number>('x');
  expect(results).toStrictEqual([1, 2, 3]); expect(calls).toStrictEqual([1, 2, 3]);
  await root.dispose();
});

test('hook precedence, init cleanup and failure states', async () => {
  const c = new Container(); const order: string[] = [];
  c.singleton('x', {
    factory: () => ({ onInit: () => order.push('instanceInit'), onDispose: () => order.push('instanceDispose') }),
    onInit: () => { order.push('lifecycleInit'); throw new Error('init'); },
    onDispose: () => { order.push('lifecycleDispose'); }
  });
  await expect(c.resolve('x')).rejects.toThrow(/init/);
  expect(c.instanceState('x')).toBe('failed');
  expect(order).toStrictEqual(['lifecycleInit', 'lifecycleDispose']);
  await c.dispose();
});

test('interceptors ordered root-first and errors retain the original', async () => {
  const c = new Container(); const child = c.createScope(); const order: string[] = [];
  c.addInterceptor({ onBeforeResolve: () => { order.push('beforeRoot'); }, onAfterResolve: () => { order.push('afterRoot'); } });
  child.addInterceptor({ onBeforeResolve: () => { order.push('beforeChild'); }, onAfterResolve: () => { order.push('afterChild'); } });
  c.singleton('valid', () => ({ ready: true }));
  await child.resolve('valid');
  expect(order).toStrictEqual(['beforeRoot', 'beforeChild', 'afterRoot', 'afterChild']);
  c.singleton('bad', () => { throw new Error('original'); });
  child.addInterceptor({ onResolutionError: () => { throw new Error('error in hook'); } });
  await expect(child.resolve('bad')).rejects.toThrow(expect.toSatisfy((error: unknown) => error instanceof Error && error.message === 'original' && (error.cause as Error)?.message === 'error in hook'));
  await c.dispose();
});

test('decorator-free constructor and property injection', async () => {
  class Service {
    static $injectable = { token: 'svc', lifetime: ServiceLifetime.Singleton };
    static $inject = ['db'];
    static $members = { optional: { token: 'missing', optional: true }, values: { token: 'values', all: true }, config: 'config' };
    optional?: unknown; values: unknown[] = []; config?: number; db: unknown;
    constructor(db: unknown) { this.db = db; }
    onInit() { expect(this.values).toStrictEqual([1, 2]); expect(this.optional).toBe(undefined); expect(this.config).toBe(3); }
  }
  const c = new Container(); c.registerInstance('db', { valid: true }); c.registerInstance('config', 3);
  c.singletonMulti('values', () => 1); c.singletonMulti('values', () => 2);
  c.registerClass(Service); c.validate();
  const a = await c.resolve<Service>('svc');
  expect(a.db).toBe(c.get('db'));
  expect(a).toBe(await c.resolve('svc'));
  await c.dispose();
});

test('missing class parameter throws when resolving, not when registering', async () => {
  class Missing { static $injectable = {}; constructor(_dependency: unknown) {} }
  const c = new Container(); c.registerClass(Missing);
  await expect(c.resolve(Missing)).rejects.toThrow(/Cannot resolve dependency at index 0/);
  await c.dispose();
});

test('declared graph deferred missing, captive, cycles, undeclared and dangling', async () => {
  const c = new Container(); c.singleton('a', () => ({}), { deps: ['b'] });
  expect(() => c.validate()).toThrow(expect.toSatisfy((e: unknown) => e instanceof DiGraphError && e.issues.some(i => i.kind === 'missing')));
  c.singleton('b', () => ({})); c.validate();
  expect(c.graph().undeclared().length).toBe(1);
  const rc = new Container(); rc.request('req', () => ({})); rc.singleton('capt', () => ({}), { deps: ['req'] });
  expect(rc.issuesObserved().some(i => i.kind === 'captive')).toBeTruthy();
  expect(() => rc.validate()).toThrow(DiGraphError);
  await c.unregister('b'); expect(c.issuesObserved().some(i => i.kind === 'dangling')).toBeTruthy();
  const cycles = new Container(); cycles.singleton('x', () => ({}), { deps: ['y'] });
  cycles.singleton('y', () => ({}), { deps: ['x'] });
  expect(() => cycles.validate()).toThrow(expect.toSatisfy((e: unknown) => e instanceof DiGraphError && e.issues.some(i => i.kind === 'cycle')));
  await c.dispose(); await rc.dispose(); await cycles.dispose();
});

test('live graph parent updates and standalone graph ordering', () => {
  const parent = new DiGraph({ onIssue: () => {} });
  const child = new DiGraph({ parent, onIssue: () => {} });
  const dependency = Symbol('dependency'); const consumer = Symbol('consumer');
  child.add({ key: consumer, token: consumer, lifetime: ServiceLifetime.Scoped, declared: true,
    deps: [{ token: dependency, key: dependency, index: 0, optional: false, multi: false }] });
  expect(child.unresolved().length).toBe(1);
  parent.add({ key: dependency, token: dependency, lifetime: ServiceLifetime.Scoped, declared: true, deps: [] });
  expect(child.unresolved().length).toBe(0);
  expect(child.order().map(n => n.key)).toStrictEqual([consumer]);
  child.remove(consumer); expect(child.size).toBe(0);
});

test('replace preserves generation and supports retainPrevious and rejectIfLive', async () => {
  const c = new Container(); const closed: string[] = [];
  c.singleton('thing', () => ({ version: 1, onDispose: () => closed.push('old') }));
  const before = await c.resolve('thing');
  await expect(c.replace('thing', () => ({}), ServiceLifetime.Singleton, { rejectIfLive: true })).rejects.toThrow(MutationConflictError);
  const handle = await c.replace('thing', () => ({ version: 2 }), ServiceLifetime.Singleton, { retainPrevious: true });
  expect(handle.generation).toBe(1);
  expect(handle.retainedInstances).toStrictEqual([before]);
  expect(closed).toStrictEqual([]);
  expect((await c.resolve<{ version: number }>('thing')).version).toBe(2);
  await c.dispose();
});

test('transactional replacement invalidates dependents and rolls back failed rebuild', async () => {
  const c = new Container(); let fail = false; const events: string[] = [];
  c.singleton('db', () => ({ value: 1, onDispose: () => events.push('db') }));
  c.singleton('service', async c => {
    if (fail) throw new Error('bad service');
    return { db: await c.resolve<{ value: number }>('db'), onDispose: () => events.push('service') };
  }, { deps: ['db'] });
  const originalService = await c.resolve('service'); const originalDb = c.get('db');
  fail = true;
  await expect(c.replace('db', () => ({ value: 2 }), ServiceLifetime.Singleton, { rebuild: true })).rejects.toThrow(/bad service/);
  expect(c.get('db')).toBe(originalDb); expect(c.get('service')).toBe(originalService);
  fail = false;
  const handle = await c.replace('db', () => ({ value: 3 }), ServiceLifetime.Singleton);
  expect(handle.generation).toBe(1);
  expect(events).toStrictEqual(['service', 'db']);
  expect((await c.resolve<{ db: { value: number } }>('service')).db.value).toBe(3);
  await c.dispose();
});

test('unregister a multi binding removes one entry; child cannot unregister parent', async () => {
  const c = new Container(); const one = c.singletonMulti('x', () => 1); c.singletonMulti('x', () => 2);
  const child = c.createScope();
  await expect(child.unregister('x')).rejects.toThrow(MutationConflictError);
  expect(await one.unregister()).toBe(true);
  expect(await c.resolveAll('x')).toStrictEqual([2]);
  await c.dispose();
});

test('clearInstances disposes owned instances in reverse build order, drops them, keeps supplied ones', async () => {
  const c = new Container(); let constructed = 0; const order: string[] = [];
  c.singleton('s', () => { constructed++; return { onDispose: () => { order.push('s'); } }; });
  c.singleton('t', async () => ({ onDispose: () => { order.push('t'); } }));
  const manual = { x: 1, onDispose: () => { order.push('manual'); } }; c.registerInstance('manual', manual);
  await c.resolve('s'); await c.resolve('t'); await c.clearInstances();
  expect(order).toStrictEqual(['t', 's']); expect(c.get('manual')).toBe(manual);
  await c.resolve('s'); expect(constructed).toBe(2);
  await c.dispose(); expect(order).toContain('manual');
});

test('clearInstances waits for a build in progress, then clears it', async () => {
  const c = new Container(); let disposed = 0;
  c.singleton('slow', async () => { await new Promise(r => setTimeout(r, 5)); return { onDispose: () => { disposed++; } }; });
  const building = c.resolve('slow'); await c.clearInstances(); await building;
  expect(disposed).toBe(1); expect(c.instanceState('slow')).toBe('uninitialized');
  await c.dispose();
});

test('clearInstances reports a failing hook after dropping every instance', async () => {
  const c = new Container(); let constructed = 0;
  c.singleton('bad', () => { constructed++; return { onDispose: () => { throw new Error('nope'); } }; });
  await c.resolve('bad');
  await expect(c.clearInstances()).rejects.toThrow(AggregateError);
  await c.resolve('bad'); expect(constructed).toBe(2);
});

test('an instance supplied without a policy is owned; an external one is never disposed', async () => {
  const c = new Container(); let owned = 0; let external = 0;
  c.registerInstance('a', { onDispose: () => { owned++; } });
  c.registerInstance('b', { onDispose: () => { external++; } }, { ownership: 'external' });
  const byToken = new Map(c.getResourceGraphSnapshot().resources.map(r => [r.token, r.ownership]));
  expect(byToken.get('"a"')).toBe('owned'); expect(byToken.get('"b"')).toBe('external');
  await c.dispose(); expect(owned).toBe(1); expect(external).toBe(0);
});

test('dispose children then parents, reverse build order, continues after failure, rejects disposed access', async () => {
  const c = new Container(); const child = c.createScope(); const order: string[] = [];
  c.singleton('parent', () => ({ onDispose: () => { order.push('parent'); throw new Error('dispose-error'); } }));
  child.singleton('child', () => ({ onDispose: () => order.push('child') }));
  await c.resolve('parent'); await child.resolve('child');
  await expect(c.dispose()).rejects.toThrow(AggregateError);
  expect(order).toStrictEqual(['child', 'parent']);
  await expect(c.resolve('x')).rejects.toThrow(LifecycleError);
  expect(() => child.graph()).toThrow(LifecycleError);
  await c.dispose();
});

test('initializeOwned is scope-sensitive and runs onReady once per instance', async () => {
  const c = new Container(); let global = 0, ten = 0, req = 0;
  c.singleton('g', () => ({ onReady: () => global++ }));
  c.tenant('t', () => ({ onReady: () => ten++ }));
  c.request('r', () => ({ onReady: () => req++ }));
  await c.initializeOwned(); expect(global).toBe(1); expect(ten).toBe(0); expect(req).toBe(0);
  await c.initializeOwned(); expect(global).toBe(1);
  const t = c.createTenantScope('t1'); await t.initializeOwned(); expect(ten).toBe(1);
  const r = t.createRequestScope(); await r.initializeOwned(); expect(req).toBe(1);
  await c.dispose();
});

test('health check detects unloaded and throwing hooks', async () => {
  const c = new Container(); c.singleton('s', () => ({ onHealthCheck: () => { throw new Error('offline'); } }));
  expect(await c.healthCheck('s')).toStrictEqual({ healthy: false, error: 'Service not initialized' });
  await c.resolve('s'); expect(await c.healthCheck('s')).toStrictEqual({ healthy: false, error: 'offline' });
  expect(await c.healthCheck('other')).toStrictEqual({ healthy: false, error: 'Service not registered' });
  await c.dispose();
});

test('runtime failing start retries successfully, rolls back only new services', async () => {
  const c = new Container(); let fail = true; let cleaned = 0;
  c.singleton('a', () => ({ onDispose: () => cleaned++ }));
  c.singleton('b', () => { if (fail) throw new Error('startup'); return 1; });
  const runtime = new DiRuntime(c);
  await expect(runtime.start()).rejects.toThrow(expect.toSatisfy((e: unknown) => (e as { code: string }).code === 'RUNTIME_FAILED'));
  expect(cleaned).toBe(1); expect(c.instanceState('a')).toBe('uninitialized');
  fail = false; await runtime.start(); expect(runtime.state).toBe('ready');
  await runtime.dispose();
});

test('runtime concurrently configures one tenant; failed tenant isolated and retry after drain', async () => {
  const root = new Container(); const called = new Map<string, number>();
  const runtime = new DiRuntime(root, { configureTenant(id: string) {
    called.set(id, (called.get(id) ?? 0) + 1);
    if (id === 'fail' && called.get(id) === 1) throw new Error('no');
  } });
  await runtime.start();
  await Promise.all(Array.from({ length: 20 }, () => runtime.withTenant('a', async () => {})));
  expect(called.get('a')).toBe(1);
  await expect(runtime.withTenant('fail', () => {})).rejects.toThrow(expect.toSatisfy((e: unknown) => (e as { code: string }).code === 'TENANT_FAILED'));
  await runtime.withTenant('a', async () => {});
  await runtime.drainTenant('fail');
  await runtime.withTenant('fail', async () => {});
  expect(called.get('fail')).toBe(2);
  await runtime.dispose();
});

test('each request gets a new scope, release idempotent, graceful drain waits for lease', async () => {
  const c = new Container(); c.request('resource', () => ({}));
  const runtime = new DiRuntime(c); await runtime.start();
  const first = await runtime.acquireRequest('tenant'); const second = await runtime.acquireRequest('tenant');
  expect(first.container.identity.id).not.toBe(second.container.identity.id);
  expect(first.container.identity.tenantId).toBe('tenant');
  await Promise.all([first.release(), first.release()]);
  let complete!: () => void;
  const pending = new Promise<void>(resolve => { complete = resolve; });
  const work = second.run(async () => { await pending; });
  let drained = false;
  const draining = runtime.drain().then(() => { drained = true; });
  await Promise.resolve(); expect(drained).toBe(false);
  complete(); await work; await second.release(); await draining;
  await expect(runtime.withTenant('tenant', () => {})).rejects.toThrow(expect.toSatisfy((e: unknown) => (e as { code: string }).code === 'RUNTIME_DRAINING'));
  await runtime.dispose();
});

test('resource graph includes observed edges, changes generation on replacement', async () => {
  const c = new Container(); c.singleton('b', () => ({})); c.singleton('a', async c => ({ b: await c.resolve('b') }), { deps: ['b'] });
  await c.resolve('a');
  expect(c.getResourceGraphSnapshot().resources.length).toBe(2);
  expect(c.getResourceGraphSnapshot().resourceEdges.length).toBe(1);
  expect(c.dependencyEdges()[0]!.from.generation).toBe(0);
  await c.replace('b', () => ({}), ServiceLifetime.Singleton, { rebuild: true });
  expect(c.dependencyEdges().some(e => e.to.generation === 1)).toBeTruthy();
  await c.dispose();
});
