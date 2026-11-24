import { expect, test, vi } from 'vitest';
import { Container, DiError, ServiceLifetime, Injectable, type IContainer, getTokenCacheSize, createToken } from '@coldmind/inject';

/** Explicit settlement avoids timing-based races in build-control regressions. */
function deferred<T>() {
  let resolve!: (value: T) => void; let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
/** Finite watchdog is a test failure, never evidence of a DI deadline. */
async function bounded<T>(promise: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([promise, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('build-control watchdog expired')), 1000); })]); }
  finally { if (timer !== undefined) clearTimeout(timer); }
}
/** Attach rejection handling immediately, before starting other work. */
function outcome<T>(promise: Promise<T>) { return promise.then(value => ({ ok: true as const, value }), error => ({ ok: false as const, error })); }
/** Require the product error rather than accepting a watchdog or runner timeout. */
async function failure<T>(result: ReturnType<typeof outcome<T>>, code = 'DI_BUILD_TIMEOUT'): Promise<DiError> {
  const settled = await bounded(result); expect(settled.ok).toBe(false);
  if (settled.ok) throw new Error('expected build failure');
  expect(settled.error).toBeInstanceOf(DiError); expect(settled.error.code).toBe(code); return settled.error;
}
/** Allow late settlement handlers and rejection events to run without elapsed-time assertions. */
async function turn(): Promise<void> { await new Promise<void>(resolve => setTimeout(resolve, 0)); }

test('first caller and joined singleton waiters time out from one hanging factory', async () => {
  const c = new Container(); c.buildTimeout = 10; const gate = deferred<object>(); let calls = 0;
  const token = createToken<object>('timeout-first-and-joined'); c.singleton(token, () => { calls++; return gate.promise; });
  const first = outcome(c.resolve(token)); const joined = outcome(c.resolve(token));
  await Promise.all([failure(first), failure(joined)]); expect(calls).toBe(1); expect(c.cachedBindings()).toEqual([]); await bounded(c.dispose());
});

test('independent transient callers both receive a deadline without caching', async () => {
  const c = new Container(); c.buildTimeout = 10; let calls = 0; const token = createToken<object>('timeout-transient');
  c.transient(token, () => { calls++; return deferred<object>().promise; });
  const first = outcome(c.resolve(token)); const second = outcome(c.resolve(token));
  await Promise.all([failure(first), failure(second)]); expect(calls).toBe(2); expect(c.cachedBindings()).toEqual([]); await bounded(c.dispose());
});

test('deadline includes a never-settling onInit and aborts its build context', async () => {
  const c = new Container(); c.buildTimeout = 10; const entered = deferred<AbortSignal>(); const gate = deferred<void>();
  c.singleton('timeout-init', { factory: () => ({}), onInit: (_, context) => { entered.resolve(context.signal); return gate.promise; } });
  const pending = outcome(c.resolve('timeout-init')); const signal = await bounded(entered.promise);
  await failure(pending); expect(signal.aborted).toBe(true); expect(c.cachedBindings()).toEqual([]); gate.resolve(); await turn(); await bounded(c.dispose());
});

test('a scoped build inherits its owning child timeout from the root', async () => {
  const root = new Container(); root.buildTimeout = 10; const child = root.createScope();
  root.scoped('timeout-inherited', () => deferred<object>().promise);
  await failure(outcome(child.resolve('timeout-inherited'))); await bounded(root.dispose());
});

test('a child deadline overrides the root for inherited scoped registrations', async () => {
  const root = new Container(); const child = root.createScope(); child.buildTimeout = 0;
  root.scoped('timeout-child', () => deferred<object>().promise);
  await failure(outcome(child.resolve('timeout-child'))); expect(root.buildTimeout).toBeUndefined(); await bounded(root.dispose());
});

test('registration timeout overrides an otherwise disabled owner deadline', async () => {
  const c = new Container(); c.singleton('timeout-registration', () => deferred<object>().promise, { timeoutMs: 0 });
  await failure(outcome(c.resolve('timeout-registration'))); await bounded(c.dispose());
});

test('deadlines are off by default and a controlled async factory remains pending', async () => {
  const c = new Container(); const gate = deferred<object>(); const value = {}; let settled = false;
  c.singleton('timeout-default-off', () => gate.promise); const pending = c.resolve('timeout-default-off');
  const observed = pending.then(result => { settled = true; return result; }); await turn(); expect(settled).toBe(false);
  gate.resolve(value); expect(await bounded(observed)).toBe(value); await bounded(c.dispose());
});

test('explicit null child deadline disables inherited zero deadline', async () => {
  const root = new Container(); root.buildTimeout = 0; const child = root.createScope(); child.buildTimeout = null;
  const gate = deferred<object>(); const value = {}; root.scoped('timeout-child-off', () => gate.promise);
  const pending = outcome(child.resolve('timeout-child-off')); await turn(); gate.resolve(value);
  expect(await bounded(pending)).toEqual({ ok: true, value }); await bounded(root.dispose());
});

test('explicit null registration deadline disables owner zero deadline', async () => {
  const c = new Container(); c.buildTimeout = 0; const gate = deferred<object>(); const value = {};
  c.singleton('timeout-registration-off', () => gate.promise, { timeoutMs: null });
  const pending = outcome(c.resolve('timeout-registration-off')); await turn(); gate.resolve(value);
  expect(await bounded(pending)).toEqual({ ok: true, value }); await bounded(c.dispose());
});

test('zero means an immediate async deadline rather than disabled', async () => {
  const c = new Container(); c.buildTimeout = 0; c.singleton('timeout-zero', () => deferred<object>().promise);
  await failure(outcome(c.resolve('timeout-zero'))); await bounded(c.dispose());
});

test('negative and nonfinite container settings reject before starting builds', async () => {
  const c = new Container();
  for (const invalid of [-1, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) expect(() => { c.buildTimeout = invalid; }).toThrow();
  await bounded(c.dispose());
});

test('negative and nonfinite registration settings reject without installing bindings', async () => {
  const c = new Container(); const token = createToken<object>('timeout-invalid'); let calls = 0;
  for (const timeoutMs of [-1, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
    expect(() => c.singleton(token, () => { calls++; return {}; }, { timeoutMs })).toThrow(); expect(c.bindingOf(token)).toBeUndefined();
  }
  expect(calls).toBe(0); await bounded(c.dispose());
});

test('timeout aborts the signal provided to the factory', async () => {
  const c = new Container(); c.buildTimeout = 10; const started = deferred<AbortSignal>(); let aborts = 0;
  c.singleton('timeout-signal', context => { started.resolve(context.signal); context.signal.addEventListener('abort', () => { aborts++; }); return deferred<object>().promise; });
  const pending = outcome(c.resolve('timeout-signal')); const signal = await bounded(started.promise); expect(signal.aborted).toBe(false);
  await failure(pending); expect(signal.aborted).toBe(true); expect(aborts).toBe(1); await bounded(c.dispose()); expect(aborts).toBe(1);
});

test('owner disposal aborts before waiting for an abort-aware build without deadline', async () => {
  const c = new Container(); const child = c.createScope(); const started = deferred<AbortSignal>(); let aborts = 0;
  c.scoped('cancel-aware', context => { started.resolve(context.signal); return new Promise<object>(resolve => context.signal.addEventListener('abort', () => { aborts++; resolve({}); }, { once: true })); });
  const pending = outcome(child.resolve('cancel-aware')); const signal = await bounded(started.promise);
  await bounded(child.dispose()); expect(signal.aborted).toBe(true); expect(aborts).toBe(1); await failure(pending, 'DI_BUILD_CANCELLED'); expect(c.signal.aborted).toBe(false); await bounded(c.dispose());
});

test('owner disposal settles first and joined callers despite an ignoring factory without deadline', async () => {
  const c = new Container(); const started = deferred<AbortSignal>();
  c.singleton('cancel-ignoring', context => { started.resolve(context.signal); return deferred<object>().promise; });
  const first = outcome(c.resolve('cancel-ignoring')); const joined = outcome(c.resolve('cancel-ignoring')); const signal = await bounded(started.promise);
  await bounded(c.dispose()); expect(signal.aborted).toBe(true); expect(c.signal.aborted).toBe(true);
  await Promise.all([failure(first, 'DI_BUILD_CANCELLED'), failure(joined, 'DI_BUILD_CANCELLED')]);
});

test('late owned factory success is disposed once and cannot overwrite an immediate retry', async () => {
  const c = new Container(); c.buildTimeout = 0; const gate = deferred<object>(); let attempts = 0; let disposed = 0;
  const late = { onDispose() { disposed++; } }; const fresh = {};
  c.singleton('timeout-retry', () => ++attempts === 1 ? gate.promise : fresh);
  await failure(outcome(c.resolve('timeout-retry'))); expect(await bounded(c.resolve('timeout-retry'))).toBe(fresh);
  gate.resolve(late); await turn(); expect(disposed).toBe(1); expect(c.get('timeout-retry')).toBe(fresh); expect(attempts).toBe(2);
  await bounded(c.dispose()); expect(disposed).toBe(1);
});

test('late onInit completion never publishes readiness and owned cleanup occurs once', async () => {
  const c = new Container(); c.buildTimeout = 0; const entered = deferred<void>(); const init = deferred<void>(); let disposed = 0;
  c.singleton('timeout-late-init', { factory: () => ({}), onInit: () => { entered.resolve(); return init.promise; }, onDispose: () => { disposed++; } });
  const pending = outcome(c.resolve('timeout-late-init')); await bounded(entered.promise); await failure(pending);
  init.resolve(); await turn(); expect(c.cachedBindings()).toEqual([]); expect(disposed).toBe(1); await bounded(c.dispose()); expect(disposed).toBe(1);
});

test('disposal cancellation also cleans a late owned factory value exactly once', async () => {
  const c = new Container(); const started = deferred<void>(); const gate = deferred<object>(); let disposed = 0;
  c.singleton('cancel-late', () => { started.resolve(); return gate.promise; }); const pending = outcome(c.resolve('cancel-late'));
  await bounded(started.promise); await bounded(c.dispose()); await failure(pending, 'DI_BUILD_CANCELLED');
  gate.resolve({ onDispose() { disposed++; } }); await turn(); expect(disposed).toBe(1); expect(c.isDisposed).toBe(true); await bounded(c.dispose()); expect(disposed).toBe(1);
});

test('cancelled unreturned transient is cleaned but successfully returned transient stays caller-owned', async () => {
  const c = new Container(); c.buildTimeout = 0; const gate = deferred<object>(); let attempts = 0; let lateDisposals = 0; let returnedDisposals = 0;
  const returned = { onDispose() { returnedDisposals++; } };
  c.transient('timeout-transient-ownership', () => ++attempts === 1 ? gate.promise : returned);
  await failure(outcome(c.resolve('timeout-transient-ownership'))); expect(await bounded(c.resolve('timeout-transient-ownership'))).toBe(returned);
  gate.resolve({ onDispose() { lateDisposals++; } }); await turn(); expect(lateDisposals).toBe(1);
  await bounded(c.dispose()); expect(lateDisposals).toBe(1); expect(returnedDisposals).toBe(0);
});

test('an external replacement finishing onInit after timeout is never disposed', async () => {
  const c = new Container(); c.buildTimeout = 0; const entered = deferred<void>(); const init = deferred<void>(); let disposals = 0;
  const value = { onDispose() { disposals++; } }; const handle = c.registerInstance('timeout-external', value, { ownership: 'external', reason: 'caller' });
  const pending = outcome(handle.replace({ factory: () => value, onInit() { entered.resolve(); return init.promise; } }));
  await bounded(entered.promise); await failure(pending);
  init.resolve(); await turn(); expect(c.get('timeout-external')).toBe(value); await bounded(c.dispose()); expect(disposals).toBe(0);
});

test('diagnostics include both pending builds, the resolution path and a waits-for edge', async () => {
  const c = new Container(); const gate = deferred<object>();
  const a = createToken<object>('timeout-diagnostic-A'); const b = createToken<object>('timeout-diagnostic-B');
  c.singleton(b, () => gate.promise); c.singleton(a, async context => context.resolve(b), { timeoutMs: 0 });
  c.transient('timeout-diagnostic-transient', () => gate.promise);
  // Only A has a deadline, so B and the transient are still pending when its diagnostics snapshot is captured.
  const firstB = outcome(c.resolve(b)); const firstA = outcome(c.resolve(a)); const transient = outcome(c.resolve('timeout-diagnostic-transient'));
  const error = await failure(firstA); expect(error.message).toContain('timeout-diagnostic-A'); expect(error.message).toContain('timeout-diagnostic-B');
  expect(error.message).toContain('timeout-diagnostic-transient'); expect(error.message).toMatch(/path/i); expect(error.message).toMatch(/waits?[- ]?for/i); expect(error.message).toMatch(/->|→/);
  await bounded(c.dispose()); await Promise.all([failure(firstB, 'DI_BUILD_CANCELLED'), failure(transient, 'DI_BUILD_CANCELLED')]);
});

test('replacement timeout preserves original binding and instance and cleans only late candidate', async () => {
  const c = new Container(); c.buildTimeout = 0; let oldDisposals = 0; let candidateDisposals = 0; const init = deferred<void>();
  const token = createToken<object>('timeout-replacement'); const original = { onDispose() { oldDisposals++; } }; const handle = c.singleton(token, () => original);
  expect(c.resolveSync(token)).toBe(original); const epoch = c.getRegistryEpoch(); const descriptor = c.bindingOf(token);
  const pending = outcome(handle.replace({ factory: () => ({}), onInit: () => init.promise, onDispose: () => { candidateDisposals++; } }, ServiceLifetime.Singleton, { rebuild: true }));
  await failure(pending); expect(c.get(token)).toBe(original); expect(c.bindingOf(token)?.id).toBe(descriptor?.id); expect(c.bindingOf(token)?.generation).toBe(descriptor?.generation);
  expect(c.getRegistryEpoch()).toBe(epoch); expect(oldDisposals).toBe(0); init.resolve(); await turn(); expect(candidateDisposals).toBe(1);
  await bounded(c.dispose()); expect(oldDisposals).toBe(1); expect(candidateDisposals).toBe(1);
});

test('late factory and onInit rejections are consumed without unhandled rejection', async () => {
  const c = new Container(); c.buildTimeout = 0; const factory = deferred<object>(); const init = deferred<void>(); const errors: unknown[] = [];
  const listener = (reason: unknown) => { errors.push(reason); }; process.on('unhandledRejection', listener);
  try {
    c.singleton('timeout-late-reject-factory', () => factory.promise); c.singleton('timeout-late-reject-init', { factory: () => ({}), onInit: () => init.promise });
    const first = outcome(c.resolve('timeout-late-reject-factory')); const second = outcome(c.resolve('timeout-late-reject-init'));
    await Promise.all([failure(first), failure(second)]); factory.reject(new Error('late factory')); init.reject(new Error('late init'));
    await turn(); await turn(); expect(errors).toEqual([]); await bounded(c.dispose());
  } finally { process.off('unhandledRejection', listener); }
});

test('zero deadline preserves synchronous get and resolveSync as immediate values', async () => {
  const c = new Container(); c.buildTimeout = 0; let calls = 0; const value = {};
  c.singleton('timeout-sync', () => { calls++; return value; }); expect(c.resolveSync('timeout-sync')).toBe(value); expect(c.get('timeout-sync')).toBe(value);
  c.transient('timeout-sync-transient', () => value, { timeoutMs: 0 }); expect(c.resolveSync('timeout-sync-transient')).toBe(value);
  expect(calls).toBe(1); expect(c.signal.aborted).toBe(false); await bounded(c.dispose());
});

/** Tagged class exercises the registered and manual completion-context signal boundary. */
@Injectable({ tags: ['build-signal-hook'] })
class SignalTarget {}

test('successful factory and onInit contexts retain one signal through readiness and owner disposal', async () => {
  const c = new Container(); let factory!: IContainer; let initialization!: IContainer;
  c.singleton('ready-signal', { factory: context => { factory = context; return {}; }, onInit: (_, context) => { initialization = context; } });
  await c.resolve('ready-signal'); const signal = factory.signal;
  expect(initialization.signal).toBe(signal); expect(signal.aborted).toBe(false);
  await c.dispose(); expect(factory.signal).toBe(signal); expect(initialization.signal).toBe(signal); expect(signal.aborted).toBe(true);
});

test('completion contexts retain originating build signals while manual hooks use scope lifetime', async () => {
  const c = new Container(); c.buildTimeout = 0; let hook!: IContainer; const gate = deferred<void>();
  c.onInstance('build-signal-hook', async (_, context) => { hook = context.container; await gate.promise; });
  c.registerClass(SignalTarget);
  await failure(outcome(c.resolve(SignalTarget))); const signal = hook.signal;
  expect(signal).not.toBe(c.signal); expect(signal.aborted).toBe(true); gate.resolve(); await turn(); expect(hook.signal).toBe(signal);
  const manual = new SignalTarget(); await c.injectMembersAsync(manual); expect(hook.signal).toBe(c.signal); expect(hook.signal.aborted).toBe(false);
  await c.dispose(); expect(hook.signal.aborted).toBe(true);
});

test('a completed transient signal still follows owner lifetime without acquiring ownership', async () => {
  const c = new Container(); let context!: IContainer; let disposed = 0;
  c.transient('transient-ready-signal', factory => { context = factory; return { onDispose() { disposed++; } }; });
  await c.resolve('transient-ready-signal'); const signal = context.signal; await c.dispose();
  expect(context.signal).toBe(signal); expect(signal.aborted).toBe(true); expect(disposed).toBe(0);
});

test('signal fallback propagates local timeout and owner disposal with no repeated abort', async () => {
  const descriptor = Object.getOwnPropertyDescriptor(AbortSignal, 'any');
  Object.defineProperty(AbortSignal, 'any', { configurable: true, value: undefined });
  try {
    const c = new Container(); c.buildTimeout = 0; let pending!: AbortSignal; let ready!: AbortSignal; let aborts = 0;
    c.singleton('fallback-pending', context => { pending = context.signal; pending.addEventListener('abort', () => { aborts++; }); return deferred<object>().promise; });
    c.singleton('fallback-ready', context => { ready = context.signal; return {}; });
    await c.resolve('fallback-ready'); await failure(outcome(c.resolve('fallback-pending')));
    expect(pending.aborted).toBe(true); expect(ready.aborted).toBe(false); await c.dispose(); expect(ready.aborted).toBe(true); expect(aborts).toBe(1);
    expect(c.signal.aborted).toBe(true);
  } finally { if (descriptor) Object.defineProperty(AbortSignal, 'any', descriptor); else Reflect.deleteProperty(AbortSignal, 'any'); }
});

test('late owned cleanup rejection is consumed and cleanup is not retried', async () => {
  const c = new Container(); c.buildTimeout = 0; const gate = deferred<object>(); let disposals = 0; const errors: unknown[] = [];
  const listener = (error: unknown) => { errors.push(error); }; process.on('unhandledRejection', listener);
  try {
    c.singleton('late-cleanup-reject', () => gate.promise); await failure(outcome(c.resolve('late-cleanup-reject')));
    gate.resolve({ onDispose() { disposals++; return Promise.reject(new Error('cleanup rejected')); } });
    await turn(); await turn(); await c.dispose(); expect(disposals).toBe(1); expect(errors).toEqual([]);
  } finally { process.off('unhandledRejection', listener); }
});

test('singleton deadlines and lifetime belong to the root rather than requesting child', async () => {
  const c = new Container(); const child = c.createScope(); child.buildTimeout = 0; const gate = deferred<object>(); let signal!: AbortSignal;
  c.singleton('root-owner', context => { signal = context.signal; return gate.promise; });
  const pending = outcome(child.resolve('root-owner')); await turn(); await child.dispose(); expect(signal.aborted).toBe(false);
  const value = {}; gate.resolve(value); expect(await pending).toEqual({ ok: true, value }); await c.dispose(); expect(signal.aborted).toBe(true);
});

test('a factory disposing its own owner is cancelled even before its first async boundary', async () => {
  const c = new Container(); let disposals = 0; let disposal!: Promise<void>;
  c.singleton('reentrant-dispose', () => { disposal = c.dispose(); return { onDispose() { disposals++; } }; });
  await failure(outcome(c.resolve('reentrant-dispose')), 'DI_BUILD_CANCELLED'); await disposal; await turn(); expect(disposals).toBe(1);
});

test('a deadline does not keep an otherwise idle process alive', async () => {
  const { spawn } = await import('node:child_process');
  const bun = typeof Reflect.get(globalThis, 'Bun') !== 'undefined';
  const source = "import {Container} from '@coldmind/inject'; const c=new Container(); c.buildTimeout=60000; c.singleton('unref-build',()=>new Promise(()=>{})); void c.resolve('unref-build').catch(()=>{}); console.log('build-started');";
  const child = spawn(process.execPath, [...(bun ? [] : ['--experimental-transform-types', '--input-type=module']), '-e', source], { cwd: process.cwd(), stdio: ['ignore', 'pipe', 'pipe'] });
  let stdout = ''; let stderr = ''; child.stdout.on('data', data => { stdout += String(data); }); child.stderr.on('data', data => { stderr += String(data); });
  let watchdog: ReturnType<typeof setTimeout> | undefined;
  try {
    const exit = await Promise.race([new Promise<number | null>((resolve, reject) => { child.once('error', reject); child.once('exit', resolve); }), new Promise<never>((_, reject) => { watchdog = setTimeout(() => { child.kill(); reject(new Error(`deadline kept child alive: ${stderr}`)); }, 15000); })]);
    expect(exit).toBe(0); expect(stdout).toContain('build-started');
  } finally { if (watchdog) clearTimeout(watchdog); if (child.exitCode === null) child.kill(); }
}, 20000);

test('registration deadline applies to the class overload and is preserved by replacement', async () => {
  const c = new Container(); const init = deferred<void>();
  c.onInstance('build-signal-hook', () => init.promise);
  const handle = c.singleton(SignalTarget, { timeoutMs: 0 }); await failure(outcome(c.resolve(SignalTarget)));
  // The replacement keeps the registration deadline despite the owner remaining disabled.
  const gate = deferred<object>(); await failure(outcome(handle.replace(() => gate.promise, ServiceLifetime.Singleton, { rebuild: true })));
  init.resolve(); gate.resolve({}); await turn(); await c.dispose();
});

test('an abort listener reentering disposal joins the already-started teardown', async () => {
  const c = new Container(); let aborts = 0; let nested!: Promise<void>; let disposals = 0;
  c.singleton('dispose-reentry-ready', () => ({ onDispose() { disposals++; } })); await c.resolve('dispose-reentry-ready');
  c.signal.addEventListener('abort', () => { aborts++; nested = c.dispose(); });
  await bounded(c.dispose()); await bounded(nested); expect(aborts).toBe(1); expect(disposals).toBe(1);
});

test('reentrant disposal callers receive the same teardown failure', async () => {
  const c = new Container(); let nested!: ReturnType<typeof outcome<void>>;
  c.singleton('dispose-reentry-failure', () => ({ onDispose() { throw new Error('teardown failure'); } })); await c.resolve('dispose-reentry-failure');
  c.signal.addEventListener('abort', () => { nested = outcome(c.dispose()); });
  const first = outcome(c.dispose()); const results = await bounded(Promise.all([first, nested]));
  expect(results.every(result => !result.ok && result.error instanceof AggregateError)).toBe(true);
});

test('deadline timers are unrefed and cleared on success, failure and cancellation', async () => {
  const schedule = vi.spyOn(globalThis, 'setTimeout'); const clear = vi.spyOn(globalThis, 'clearTimeout');
  const c = new Container(); c.buildTimeout = 60000;
  try {
    c.singleton('timer-sync', () => ({})); c.resolveSync('timer-sync'); expect(schedule).not.toHaveBeenCalled();
    c.singleton('timer-success', async () => ({})); await c.resolve('timer-success');
    const successful = schedule.mock.results[0]!.value as ReturnType<typeof setTimeout>;
    expect(successful.hasRef()).toBe(false); expect(clear).toHaveBeenCalledWith(successful);
    c.singleton('timer-failure', async () => { throw new Error('build failed'); }); await expect(c.resolve('timer-failure')).rejects.toThrow('build failed');
    const failed = schedule.mock.results[1]!.value as ReturnType<typeof setTimeout>;
    expect(clear).toHaveBeenCalledWith(failed);
    c.singleton('timer-cancel', () => deferred<object>().promise); const pending = outcome(c.resolve('timer-cancel'));
    const cancelled = schedule.mock.results[2]!.value as ReturnType<typeof setTimeout>;
    expect(cancelled.hasRef()).toBe(false); await c.dispose(); expect(await pending).toMatchObject({ ok: false, error: { code: 'DI_BUILD_CANCELLED' } }); expect(clear).toHaveBeenCalledWith(cancelled);
  } finally { schedule.mockRestore(); clear.mockRestore(); await c.dispose(); }
});

test('replacement cancellation never restores records after its owner has disposed', async () => {
  const before = getTokenCacheSize(); const c = new Container(); let originalDisposals = 0; let candidateDisposals = 0; const entered = deferred<void>(); const init = deferred<void>();
  c.singleton('dispose-rollback-history-provider', () => 1); c.singleton('dispose-rollback-history-user', () => 2, { deps: ['dispose-rollback-history-provider'] });
  await c.unregister('dispose-rollback-history-provider'); await c.unregister('dispose-rollback-history-user');
  const handle = c.singleton<object>('dispose-replacement-rollback', () => ({ onDispose() { originalDisposals++; } })); await c.resolve('dispose-replacement-rollback');
  const pending = outcome(handle.replace({ factory: () => ({}), onInit: () => { entered.resolve(); return init.promise; }, onDispose: () => { candidateDisposals++; } }));
  await entered.promise; await c.dispose(); await failure(pending, 'DI_BUILD_CANCELLED');
  expect(c.isDisposed).toBe(true); expect(Reflect.get(c, 'records').size).toBe(0); expect(Reflect.get(c, 'disposalOrder')).toEqual([]);
  expect(getTokenCacheSize()).toBe(before); expect(originalDisposals).toBe(1); init.resolve(); await turn(); expect(candidateDisposals).toBe(1);
  expect(Reflect.get(c, 'records').size).toBe(0); expect(Reflect.get(c, 'disposalOrder')).toEqual([]);
});

test('replacement rollback skips a disposed child but preserves its active owner and handles async late cleanup', async () => {
  const root = new Container(); const child = root.createScope(); const entered = deferred<void>(); const init = deferred<void>(); const cleanup = deferred<void>(); let candidateDisposals = 0;
  const handle = root.scoped('child-dispose-replacement', () => ({})); await child.resolve('child-dispose-replacement');
  const pending = outcome(handle.replace({ factory: () => ({}), onInit: () => { entered.resolve(); return init.promise; }, onDispose: () => { candidateDisposals++; return cleanup.promise; } }));
  await entered.promise; await child.dispose(); await failure(pending, 'DI_BUILD_CANCELLED');
  expect(root.isDisposed).toBe(false); expect(Reflect.get(child, 'records').size).toBe(0); expect(Reflect.get(child, 'disposalOrder')).toEqual([]);
  const fresh = root.createScope(); expect(await fresh.resolve('child-dispose-replacement')).toEqual({});
  init.resolve(); await turn(); expect(candidateDisposals).toBe(1); cleanup.resolve(); await turn();
  expect(Reflect.get(child, 'records').size).toBe(0); await root.dispose(); expect(candidateDisposals).toBe(1);
});

test('finite deadlines above native timer range are segmented against monotonic time', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] }); const schedule = vi.spyOn(globalThis, 'setTimeout');
  const c = new Container(); c.buildTimeout = 2147483648;
  try {
    c.singleton('segmented-deadline', () => deferred<object>().promise); let settled = false;
    const pending = outcome(c.resolve('segmented-deadline')); void pending.then(() => { settled = true; });
    expect(schedule.mock.calls[0]![1]).toBe(2147483647);
    await vi.advanceTimersByTimeAsync(2147483647); expect(settled).toBe(false); expect(vi.getTimerCount()).toBe(1);
    await vi.advanceTimersByTimeAsync(1); expect(await pending).toMatchObject({ ok: false, error: { code: 'DI_BUILD_TIMEOUT' } }); expect(vi.getTimerCount()).toBe(0);
    c.buildTimeout = Number.MAX_VALUE; c.singleton('huge-finite-deadline', () => deferred<object>().promise);
    const huge = outcome(c.resolve('huge-finite-deadline')); await vi.advanceTimersByTimeAsync(20);
    expect(schedule.mock.calls.every(call => Number(call[1]) <= 2147483647)).toBe(true);
    await c.dispose(); expect(await huge).toMatchObject({ ok: false, error: { code: 'DI_BUILD_CANCELLED' } }); expect(vi.getTimerCount()).toBe(0);
  } finally { await c.dispose(); schedule.mockRestore(); vi.useRealTimers(); }
});

test('owner disposal during awaited rollback cleanup disposes detached originals in reverse order', async () => {
  const before = getTokenCacheSize(); const c = new Container(); const entered = deferred<void>(); const cleanup = deferred<void>(); const order: string[] = []; let builds = 0; let candidates = 0;
  const a = c.singleton<object>('terminal-rollback-A', () => ({ onDispose() { order.push('A'); } }));
  c.singleton('terminal-rollback-B', context => {
    context.resolveSync('terminal-rollback-A');
    if (++builds > 1) throw new Error('dependent rebuild failure');
    return { onDispose() { order.push('B'); } };
  }, { deps: ['terminal-rollback-A'] });
  await c.resolve('terminal-rollback-B');
  const replacement = outcome(a.replace({ factory: () => ({}), onDispose() { candidates++; entered.resolve(); return cleanup.promise; } }, ServiceLifetime.Singleton, { rebuild: true }));
  await entered.promise; await c.dispose(); expect(order).toEqual([]);
  cleanup.resolve(); const result = await replacement; expect(result).toMatchObject({ ok: false, error: { message: 'dependent rebuild failure' } });
  expect(candidates).toBe(1); expect(order).toEqual(['B', 'A']); expect(c.isDisposed).toBe(true); expect(getTokenCacheSize()).toBe(before);
  expect(Reflect.get(c, 'records').size).toBe(0); expect(Reflect.get(c, 'disposalOrder')).toEqual([]);
});
