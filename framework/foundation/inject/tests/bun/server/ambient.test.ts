import { expect, test } from 'vitest';
import { AsyncLocalStorage } from 'node:async_hooks';
import { Container, CircularDependencyError, LifecycleError, type IContainer } from '@coldmind/inject';

/** Flushes async factory continuations without requiring a hanging service to settle. */
const turn = () => new Promise<void>(resolve => setTimeout(resolve, 0));
/** Rejection handling is installed before disposal/timeout work begins. */
const outcome = <T>(promise: Promise<T>) => promise.then(value => ({ ok: true as const, value }), error => ({ ok: false as const, error }));
/** Explicit captured-container recursion, intentionally different from safe factory-context resolution. */
function capture(root: IContainer, name: string) { root.singleton(name, async () => { await Promise.resolve(); return root.resolve(name); }); }
/** One independently owned storage, without importing server installers. */
function source() { const storage = new AsyncLocalStorage<object>(); return { run<T>(frame: object, work: () => T): T { return storage.run(frame, work); }, current() { return storage.getStore(); } }; }

test('importing server is neutral for both standalone roots', async () => {
  const first = new Container(); const second = new Container(); await import('@coldmind/inject/server');
  for (const root of [first, second]) {
    capture(root, 'neutral-capture'); let settled = false; const pending = outcome(root.resolve('neutral-capture')); void pending.then(() => { settled = true; });
    await turn(); expect(settled).toBe(false); await root.dispose(); expect(await pending).toMatchObject({ ok: false, error: { code: 'DI_BUILD_CANCELLED' } });
  }
});

test('only the runtime-owned root detects captured-container recursion', async () => {
  const { DiRuntime } = await import('@coldmind/inject/server'); const attached = new Container(); const standalone = new Container(); const runtime = new DiRuntime(attached);
  capture(attached, 'owned-capture'); capture(standalone, 'standalone-capture');
  const owned = await outcome(attached.resolve('owned-capture')); expect(owned.ok).toBe(false); if (!owned.ok) expect(owned.error).toBeInstanceOf(CircularDependencyError);
  let settled = false; const pending = outcome(standalone.resolve('standalone-capture')); void pending.then(() => { settled = true; }); await turn(); expect(settled).toBe(false);
  await standalone.dispose(); expect(await pending).toMatchObject({ ok: false, error: { code: 'DI_BUILD_CANCELLED' } }); await runtime.dispose();
});

test('runtime roots never borrow another roots resolution frame', async () => {
  const { DiRuntime } = await import('@coldmind/inject/server'); const first = new Container(); const second = new Container(); const a = new DiRuntime(first); const b = new DiRuntime(second); const paths: string[][] = [];
  second.addInterceptor({ onBeforeResolve(context) { paths.push((context.path ?? []).map(step => step.tokenName)); } });
  second.singleton('foreign-call', async () => 2); first.singleton('foreign-call', async () => { await Promise.resolve(); return second.resolve('foreign-call'); });
  expect(await first.resolve('foreign-call')).toBe(2); expect(paths).toEqual([[JSON.stringify('foreign-call')]]); await Promise.all([a.dispose(), b.dispose()]);
});

test('even deliberately shared ambient storage cannot lend a foreign-root frame', async () => {
  const first = new Container(); const second = new Container(); const shared = source(); first.useAmbient(shared); second.useAmbient(shared); const paths: string[][] = [];
  second.addInterceptor({ onBeforeResolve(context) { paths.push((context.path ?? []).map(step => step.tokenName)); } });
  second.singleton('shared-storage', () => 3); first.singleton('shared-storage', async () => { await Promise.resolve(); return second.resolve('shared-storage'); });
  expect(await first.resolve('shared-storage')).toBe(3); expect(paths).toEqual([[JSON.stringify('shared-storage')]]); await Promise.all([first.dispose(), second.dispose()]);
});

test('children inherit root ambient resolution but cannot attach or detach it', async () => {
  const root = new Container(); const child = root.createScope(); root.useAmbient(source());
  expect(() => child.useAmbient(source())).toThrow(LifecycleError); expect(() => child.useAmbient(undefined)).toThrow(LifecycleError);
  capture(child, 'child-capture'); await expect(child.resolve('child-capture')).rejects.toBeInstanceOf(CircularDependencyError); await root.dispose();
});

test('root attachment can be replaced/detached between operations and cannot revive a disposed root', async () => {
  const root = new Container(); let firstRuns = 0; let secondRuns = 0; const first = source(); const second = source();
  root.useAmbient({ ...first, run(frame, work) { firstRuns++; return first.run(frame, work); } }); root.singleton('first-attachment', () => 1); expect(await root.resolve('first-attachment')).toBe(1); expect(firstRuns).toBeGreaterThan(0); const count = firstRuns;
  root.useAmbient({ ...second, run(frame, work) { secondRuns++; return second.run(frame, work); } }); root.singleton('second-attachment', () => 2); expect(await root.resolve('second-attachment')).toBe(2); expect(secondRuns).toBeGreaterThan(0); expect(firstRuns).toBe(count);
  root.useAmbient(undefined); const secondCount = secondRuns; root.singleton('detached-attachment', () => 3); expect(await root.resolve('detached-attachment')).toBe(3); expect(secondRuns).toBe(secondCount);
  await root.dispose(); expect(() => root.useAmbient(first)).toThrow(LifecycleError); const { DiRuntime } = await import('@coldmind/inject/server'); expect(() => new DiRuntime(root)).toThrow(LifecycleError);
});

test('request runInContext/currentContext behavior remains independent of build ambient storage', async () => {
  const { runInContext, currentContext, DiRuntime } = await import('@coldmind/inject/server'); const outer = new Container(); const runtime = new DiRuntime(); await runtime.start();
  expect(currentContext()).toBeUndefined(); await runInContext(outer, async () => { await Promise.resolve(); expect(currentContext()).toBe(outer); await runtime.withRequest('request-context', async request => { await Promise.resolve(); expect(currentContext()).toBe(request); }); expect(currentContext()).toBe(outer); });
  expect(currentContext()).toBeUndefined(); await Promise.all([outer.dispose(), runtime.dispose()]);
});
