import { expect, test } from 'vitest';
import { Container, createToken } from '@coldmind/inject';
import { currentContext, DiRuntime, type RuntimeErrorPhase } from '@coldmind/inject/server';

/** Controllable asynchronous boundary without wall-clock assumptions. */
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(complete => { resolve = complete; });
  return { promise, resolve };
}

test('runtime starts once, initializes owned singletons, and rejects use outside ready state', async () => {
  const root = new Container(); const gate = deferred(); let builds = 0; let ready = 0;
  root.singleton('runtime-start', async () => { builds++; await gate.promise; return { onReady: () => { ready++; } }; });
  const runtime = new DiRuntime(root); expect(runtime.state).toBe('created');
  await expect(runtime.acquireRequest('a')).rejects.toMatchObject({ code: 'RUNTIME_NOT_READY' });
  const first = runtime.start(); const second = runtime.start(); expect(runtime.state).toBe('starting');
  gate.resolve(); await Promise.all([first, second]); await runtime.start();
  expect(runtime.state).toBe('ready'); expect(builds).toBe(1); expect(ready).toBe(1);
  await runtime.dispose(); expect(runtime.state).toBe('disposed');
  await expect(runtime.start()).rejects.toMatchObject({ code: 'RUNTIME_DISPOSED' });
});

test('concurrent leases configure each tenant once and run within their request context', async () => {
  const gate = deferred(); const entered = deferred(); const calls: string[] = [];
  const runtime = new DiRuntime<string>(new Container(), { configureTenant: async id => { calls.push(id); entered.resolve(); await gate.promise; } });
  await runtime.start(); const first = runtime.acquireRequest('shared'); const second = runtime.acquireRequest('shared');
  await entered.promise; expect(runtime.tenantState('shared')).toBe('creating'); expect(calls).toEqual(['shared']);
  gate.resolve(); const [a, b] = await Promise.all([first, second]);
  expect(a.container).not.toBe(b.container); expect(runtime.tenantState('shared')).toBe('ready');
  await a.run(async c => { expect(currentContext()).toBe(c); await Promise.resolve(); expect(currentContext()).toBe(c); });
  await Promise.all([a.release(), b.release()]); await runtime.dispose();
});

test.each([false, true])('request eager=%s controls building request registrations at acquisition', async eagerRequest => {
  const root = new Container(); let builds = 0; const token = createToken<object>('runtime-request');
  root.request(token, () => { builds++; return {}; }); const runtime = new DiRuntime(root, { eagerRequest });
  await runtime.start(); expect(builds).toBe(0);
  const lease = await runtime.acquireRequest('a'); expect(builds).toBe(eagerRequest ? 1 : 0);
  const value = await lease.container.resolve(token); expect(await lease.container.resolve(token)).toBe(value); expect(builds).toBe(1);
  await lease.release(); await runtime.dispose();
});

test('double release shares cleanup and invalidates request work exactly once', async () => {
  const root = new Container(); const gate = deferred(); const entered = deferred(); let disposals = 0;
  root.request('runtime-release', { factory: () => ({}), onDispose: async () => { disposals++; entered.resolve(); await gate.promise; } });
  const runtime = new DiRuntime(root); await runtime.start(); const lease = await runtime.acquireRequest('a');
  await lease.container.resolve('runtime-release'); const first = lease.release(); const second = lease.release();
  expect(second).toBe(first); await entered.promise;
  await expect(lease.run(() => 1)).rejects.toMatchObject({ code: 'INVALID_SCOPE' });
  gate.resolve(); await Promise.all([first, second]); expect(disposals).toBe(1); await runtime.dispose();
});

test('drainTenant blocks new leases and waits for the live request before disposing the tenant', async () => {
  const root = new Container(); const disposed: string[] = [];
  root.tenant('runtime-tenant', { factory: () => ({}), onDispose: () => { disposed.push('tenant'); } });
  root.request('runtime-child', { factory: () => ({}), onDispose: () => { disposed.push('request'); } });
  const runtime = new DiRuntime(root); await runtime.start(); const lease = await runtime.acquireRequest('a');
  await lease.container.resolve('runtime-child'); let completed = false;
  const drain = runtime.drainTenant('a').then(() => { completed = true; });
  expect(runtime.tenantState('a')).toBe('draining');
  await expect(runtime.acquireRequest('a')).rejects.toMatchObject({ code: 'TENANT_DRAINING' });
  expect(completed).toBe(false); expect(disposed).toEqual([]);
  await lease.release(); await drain; expect(disposed).toEqual(['request', 'tenant']); expect(runtime.tenantState('a')).toBeUndefined();
  await runtime.dispose();
});

test('drain waits for leases without disposing services and dispose performs terminal cleanup', async () => {
  const root = new Container(); const disposed: string[] = [];
  root.singleton('runtime-root', { factory: () => ({}), onDispose: () => { disposed.push('root'); } });
  root.tenant('runtime-drain', { factory: () => ({}), onDispose: () => { disposed.push('tenant'); } });
  const runtime = new DiRuntime(root); await runtime.start(); const lease = await runtime.acquireRequest('a');
  let completed = false; const draining = runtime.drain().then(() => { completed = true; });
  expect(runtime.state).toBe('draining'); await expect(runtime.acquireRequest('b')).rejects.toMatchObject({ code: 'RUNTIME_DRAINING' });
  expect(completed).toBe(false); await lease.release(); await draining;
  expect(disposed).toEqual([]); await Promise.all([runtime.dispose(), runtime.dispose()]);
  expect(disposed).toEqual(['tenant', 'root']); expect(runtime.state).toBe('disposed');
});

test.each(['release', 'drain-tenant', 'dispose'] as const)('onError reports %s disposal failures and isolates observer errors', async phase => {
  const root = new Container(); const error = new Error(phase); const reports: Array<{ error: unknown; phase: RuntimeErrorPhase; tenantId?: string }> = [];
  const runtime = new DiRuntime<string>(root, { onError: (error, context) => { reports.push({ error, ...context }); throw new Error('observer'); } });
  if (phase === 'release') root.request('runtime-error', { factory: () => ({}), onDispose: () => { throw error; } });
  else if (phase === 'drain-tenant') root.tenant('runtime-error', { factory: () => ({}), onDispose: () => { throw error; } });
  else root.singleton('runtime-error', { factory: () => ({}), onDispose: () => { throw error; } });
  await runtime.start(); const lease = await runtime.acquireRequest('a');
  if (phase === 'release') await lease.container.resolve('runtime-error');
  await lease.release(); if (phase === 'drain-tenant') await runtime.drainTenant('a');
  await runtime.dispose(); expect(runtime.state).toBe('disposed');
  expect(reports).toEqual([{ error, phase, ...(phase === 'dispose' ? {} : { tenantId: 'a' }) }]);
});

test.each(['start', 'configure-tenant', 'validate', 'initialize', 'acquire'] as const)('onError reports %s failures without replacing the original cause', async phase => {
  const root = new Container(); const error = new Error(phase); const reports: Array<{ error: unknown; phase: RuntimeErrorPhase; tenantId?: string }> = [];
  const runtime = new DiRuntime<string>(root, {
    eagerRequest: true,
    configureTenant: (_id, c) => {
      if (phase === 'configure-tenant') throw error;
      if (phase === 'validate') c.singleton('runtime-invalid', () => ({}), { deps: ['runtime-missing'] });
      if (phase === 'initialize') c.tenant('runtime-init', () => { throw error; });
    },
    onError: (error, context) => { reports.push({ error, ...context }); throw new Error('observer'); },
  });
  if (phase === 'start') root.singleton('runtime-start-error', () => { throw error; });
  if (phase === 'acquire') root.request('runtime-acquire-error', () => { throw error; });
  if (phase === 'start') await expect(runtime.start()).rejects.toMatchObject({ code: 'RUNTIME_FAILED', cause: error });
  else {
    await runtime.start();
    if (phase === 'acquire') await expect(runtime.acquireRequest('a')).rejects.toBe(error);
    else await expect(runtime.acquireRequest('a')).rejects.toMatchObject({ code: 'TENANT_FAILED', ...(phase === 'validate' ? {} : { cause: error }) });
  }
  expect(reports).toContainEqual(expect.objectContaining({ phase, ...(phase === 'start' ? {} : { tenantId: 'a' }) }));
  if (phase !== 'validate') expect(reports.find(report => report.phase === phase)?.error).toBe(error);
  await runtime.dispose(); expect(runtime.state).toBe('disposed');
});
