import { expect, test } from 'vitest';
import { Container, Inject, Injectable, LifecycleError, type IContainer, createToken, defineDecorator } from '@coldmind/inject';

/** Holds a dependency until the test explicitly settles it. */
function deferred<T>(): { promise: Promise<T>; resolve(value: T): void; reject(error: unknown): void } {
  let resolve!: (value: T) => void; let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
/** Fails a regression deterministically instead of leaving a completion cycle pending forever. */
async function guarded<T>(promise: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([promise, new Promise<never>((_resolve, reject) => { timer = setTimeout(() => reject(new Error('completion made no progress')), 100); })]); }
  finally { if (timer !== undefined) clearTimeout(timer); }
}
/** The member token used by independently injected targets. */
const Word = createToken<string>('completion-word');
/** Wraps the actual method once for each tree completion. */
const Wrap = defineDecorator<[]>({ name: '@CompletionWrap', on: ['method'], apply(target) { target.wrap(previous => (...args) => `wrapped:${previous(...args)}`); } });

/** A supplied instance whose field, wrapper and hooks are completed by each tree. */
@Injectable({ tags: ['completion-target'] })
class Target {
  /** The injected value. */
  @Inject(Word) word!: string;
  /** Reads the injected value through a wrapped method. */
  @Wrap() read(): string { return this.word; }
}

test('two roots complete the same instance independently without stacking generated wrappers', async () => {
  const first = new Container(); const second = new Container(); const value = new Target(); let firstHooks = 0; let secondHooks = 0;
  first.registerInstance(Word, 'first'); second.registerInstance(Word, 'second');
  first.onInstance('completion-target', () => { firstHooks++; }); second.onInstance('completion-target', () => { secondHooks++; });
  try {
    expect(first.injectMembers(value)).toBe(value); expect(value.read()).toBe('wrapped:first');
    expect(second.injectMembers(value)).toBe(value); expect(value.read()).toBe('wrapped:second');
    expect(firstHooks).toBe(1); expect(secondHooks).toBe(1);
    first.injectMembers(value); second.injectMembers(value); expect(firstHooks).toBe(1); expect(secondHooks).toBe(1);
  } finally { await first.dispose(); await second.dispose(); }
});

test('a child shares completed state with its root rather than reinjecting a supplied object', async () => {
  const root = new Container(); const child = root.createScope(); const value = new Target(); let hooks = 0;
  root.registerInstance(Word, 'root'); child.registerInstance(Word, 'child'); root.onInstance('completion-target', () => { hooks++; });
  try { root.injectMembers(value); child.injectMembers(value); expect(value.read()).toBe('wrapped:root'); expect(hooks).toBe(1); }
  finally { await root.dispose(); }
});

test('concurrent member injection waits for one shared completion', async () => {
  const root = new Container(); const gate = deferred<string>(); const value = new Target(); let builds = 0; let secondSettled = false;
  root.singleton(Word, () => { builds++; return gate.promise; });
  try {
    const first = root.injectMembersAsync(value); const second = root.injectMembersAsync(value); second.then(() => { secondSettled = true; });
    await Promise.resolve(); await Promise.resolve(); expect(secondSettled).toBe(false);
    gate.resolve('ready'); expect(await first).toBe(value); expect(await second).toBe(value); expect(value.read()).toBe('wrapped:ready'); expect(builds).toBe(1);
  } finally { gate.resolve('cleanup'); await root.dispose(); }
});

test('all member waiters reject together and a later call retries a failed completion', async () => {
  const root = new Container(); const gate = deferred<string>(); const value = new Target(); const failure = new Error('member failed'); let attempts = 0;
  root.singleton(Word, () => ++attempts === 1 ? gate.promise : 'retry');
  try {
    const first = root.injectMembersAsync(value); const second = root.injectMembersAsync(value);
    const results = Promise.allSettled([first, second]); gate.reject(failure);
    expect(await results).toEqual([{ status: 'rejected', reason: failure }, { status: 'rejected', reason: failure }]);
    expect(await root.injectMembersAsync(value)).toBe(value); expect(value.read()).toBe('wrapped:retry'); expect(attempts).toBe(2);
  } finally { await root.dispose(); }
});

test('hook failure retries only unfinished stages without stacking wrappers or repeating successful hooks', async () => {
  const root = new Container(); const value = new Target(); let successful = 0; let retrying = 0;
  root.registerInstance(Word, 'ready'); root.onInstance('completion-target', () => { successful++; });
  root.onInstance('completion-target', () => { if (++retrying === 1) throw new Error('hook failed'); });
  try {
    await expect(root.injectMembersAsync(value)).rejects.toThrow('hook failed'); expect(value.read()).toBe('wrapped:ready');
    expect(await root.injectMembersAsync(value)).toBe(value); expect(value.read()).toBe('wrapped:ready'); expect(successful).toBe(1); expect(retrying).toBe(2);
  } finally { await root.dispose(); }
});

test('concurrent callers await asynchronous hooks as well as injected members', async () => {
  const root = new Container(); const gate = deferred<void>(); const value = new Target(); let hooks = 0; let secondSettled = false;
  root.registerInstance(Word, 'ready'); root.onInstance('completion-target', () => { hooks++; return gate.promise; });
  try {
    const first = root.injectMembersAsync(value); const second = root.injectMembersAsync(value); second.then(() => { secondSettled = true; });
    await Promise.resolve(); await Promise.resolve(); expect(secondSettled).toBe(false);
    gate.resolve(); await Promise.all([first, second]); expect(hooks).toBe(1); expect(value.read()).toBe('wrapped:ready');
  } finally { gate.resolve(); await root.dispose(); }
});

test('a synchronous injection that starts asynchronous work can be joined through the async API', async () => {
  const root = new Container(); const gate = deferred<string>(); const value = new Target(); root.singleton(Word, () => gate.promise);
  try {
    expect(() => root.injectMembers(value)).toThrow(LifecycleError); const pending = root.injectMembersAsync(value);
    gate.resolve('joined'); expect(await pending).toBe(value); expect(value.read()).toBe('wrapped:joined');
  } finally { gate.resolve('cleanup'); await root.dispose(); }
});

test('reentrant synchronous member injection from a hook does not recurse or await itself', async () => {
  const root = new Container(); const value = new Target(); let hooks = 0; root.registerInstance(Word, 'ready');
  root.onInstance('completion-target', instance => { hooks++; expect(root.injectMembers(instance)).toBe(instance); });
  try { expect(root.injectMembers(value)).toBe(value); expect(hooks).toBe(1); expect(value.read()).toBe('wrapped:ready'); }
  finally { await root.dispose(); }
});

test('retrying a member setter failure does not repeat fields that already completed', async () => {
  /** A class using only static member metadata. */
  class Partial {
    /** The two member slots, assigned in declaration order. */
    static $members = { first: Word, second: Word };
    /** First injected member. */
    first!: string;
    /** Second injected member. */
    second!: string;
  }
  const root = new Container(); const value = new Partial(); let writes = 0; let allow = false; let second: unknown;
  Object.defineProperty(value, 'first', { configurable: true, set() { writes++; } });
  Object.defineProperty(value, 'second', { configurable: true, get() { return second; }, set(next) { if (!allow) throw new Error('setter failed'); second = next; } });
  root.registerInstance(Word, 'ready');
  try {
    expect(() => root.injectMembers(value)).toThrow('setter failed'); expect(writes).toBe(1);
    allow = true; expect(root.injectMembers(value)).toBe(value); expect(value.second).toBe('ready'); expect(writes).toBe(1);
  } finally { await root.dispose(); }
});


test('an asynchronous hook reenters through its contextual container while outside callers still wait', async () => {
  const root = new Container(); const value = new Target(); const entered = deferred<void>(); const reentered = deferred<void>(); const release = deferred<void>(); let hooks = 0; let outsideSettled = false;
  root.registerInstance(Word, 'ready'); root.onInstance('completion-target', async (instance, context) => {
    hooks++; await Promise.resolve(); entered.resolve(); expect(context.container.resolveSync(Word)).toBe('ready');
    expect(context.container.injectMembers(instance)).toBe(instance); expect(await context.container.injectMembersAsync(instance)).toBe(instance); reentered.resolve(); await release.promise;
  });
  const first = root.injectMembersAsync(value); const outside = root.injectMembersAsync(value); outside.then(() => { outsideSettled = true; });
  try {
    await entered.promise; await guarded(reentered.promise); expect(outsideSettled).toBe(false); expect(value.read()).toBe('wrapped:ready');
    release.resolve(); await Promise.all([first, outside]); expect(hooks).toBe(1);
  } finally { release.resolve(); await root.dispose(); }
});

test('nested asynchronous completion contexts break A to B to A self-waits without repeating hooks', async () => {
  /** A second manually injected object in the completion lineage. */
  @Injectable({ tags: ['completion-nested'] })
  class Nested { /** Injected field. */ @Inject(Word) word!: string; }
  const root = new Container(); const a = new Target(); const b = new Nested(); let aHooks = 0; let bHooks = 0; root.registerInstance(Word, 'ready');
  root.onInstance('completion-target', async (_instance, context) => { aHooks++; await Promise.resolve(); await context.container.injectMembersAsync(b); });
  root.onInstance('completion-nested', async (_instance, context) => { bHooks++; await Promise.resolve(); expect(await context.container.injectMembersAsync(a)).toBe(a); });
  try { expect(await guarded(root.injectMembersAsync(a))).toBe(a); expect(aHooks).toBe(1); expect(bHooks).toBe(1); expect(b.word).toBe('ready'); }
  finally { await root.dispose(); }
});

test('an expired earlier-hook context joins later hooks instead of bypassing pending completion', async () => {
  const root = new Container(); const value = new Target(); const entered = deferred<void>(); const release = deferred<void>(); let earlier!: IContainer; let settled = false;
  root.registerInstance(Word, 'ready'); root.onInstance('completion-target', (_instance, context) => { earlier = context.container; });
  root.onInstance('completion-target', async () => { entered.resolve(); await release.promise; });
  const first = root.injectMembersAsync(value);
  try {
    await entered.promise; const joined = earlier.injectMembersAsync(value); joined.then(() => { settled = true; });
    await Promise.resolve(); await Promise.resolve(); expect(settled).toBe(false);
    release.resolve(); await Promise.all([first, joined]); expect(value.read()).toBe('wrapped:ready');
  } finally { release.resolve(); await root.dispose(); }
});

test('a retained failed-attempt context cannot bypass a later retry', async () => {
  const root = new Container(); const value = new Target(); const entered = deferred<void>(); const release = deferred<void>(); let failedContext!: IContainer; let calls = 0; let joinedSettled = false;
  root.registerInstance(Word, 'ready'); root.onInstance('completion-target', async (_instance, context) => {
    if (++calls === 1) { failedContext = context.container; await Promise.resolve(); throw new Error('first attempt failed'); }
    entered.resolve(); await release.promise;
  });
  try {
    await expect(root.injectMembersAsync(value)).rejects.toThrow('first attempt failed');
    const retry = root.injectMembersAsync(value); await entered.promise;
    const joined = failedContext.injectMembersAsync(value); joined.then(() => { joinedSettled = true; });
    await Promise.resolve(); await Promise.resolve(); expect(joinedSettled).toBe(false);
    release.resolve(); await Promise.all([retry, joined]); expect(calls).toBe(2); expect(value.read()).toBe('wrapped:ready');
  } finally { release.resolve(); await root.dispose(); }
});

test('a retained completed hook context injects new instances through normal fresh attempts', async () => {
  const root = new Container(); const first = new Target(); const second = new Target(); let retained!: IContainer; let calls = 0;
  root.registerInstance(Word, 'ready'); root.onInstance('completion-target', (_instance, context) => { calls++; retained ??= context.container; });
  try {
    root.injectMembers(first); expect(retained.identity.id).toBe(root.identity.id); expect(retained).toBeInstanceOf(Container);
    expect(await retained.injectMembersAsync(second)).toBe(second); expect(second.read()).toBe('wrapped:ready'); expect(calls).toBe(2);
  } finally { await root.dispose(); }
});

test('completion lineage survives contextual instantiateAsync calls', async () => {
  /** An unregistered class created by the first instance hook. */
  @Injectable({ tags: ['completion-instantiated'] })
  class Constructed { /** Injected field. */ @Inject(Word) word!: string; }
  const root = new Container(); const value = new Target(); let built: Constructed | undefined; root.registerInstance(Word, 'ready');
  root.onInstance('completion-target', async (_instance, context) => {
    await Promise.resolve(); built = await context.container.instantiateAsync(Constructed);
    /** A synchronous construction using the same contextual member path. */
    class Plain { /** Static member metadata. */ static $members = { word: Word }; /** Injected value. */ word!: string; }
    expect(context.container.instantiate(Plain).word).toBe('ready');
  });
  root.onInstance('completion-instantiated', async (_instance, context) => { await Promise.resolve(); await context.container.injectMembersAsync(value); });
  try { await guarded(root.injectMembersAsync(value)); expect(built?.word).toBe('ready'); expect(root.isRegistered(Constructed)).toBe(false); }
  finally { await root.dispose(); }
});

test('resolution-bound factory views propagate completion lineage into another registered class', async () => {
  /** A registered class whose hook resolves another class before publication. */
  @Injectable({ tags: ['completion-registered-a'] })
  class A { /** Injected field. */ @Inject(Word) word!: string; }
  /** A registered class whose hook reenters the already injected first instance. */
  @Injectable({ tags: ['completion-registered-b'] })
  class B { /** Injected field. */ @Inject(Word) word!: string; }
  const root = new Container(); let a!: object; let b: B | undefined; let aHooks = 0; let bHooks = 0;
  root.registerInstance(Word, 'ready'); root.singleton(A); root.singleton(B);
  root.onInstance('completion-registered-a', async (instance, context) => { a = instance; aHooks++; await Promise.resolve(); b = (await context.container.resolveAll(B))[0]; expect(await context.container.resolve(B)).toBe(b); });
  root.onInstance('completion-registered-b', async (_instance, context) => { bHooks++; await Promise.resolve(); expect(await context.container.injectMembersAsync(a)).toBe(a); });
  try { const value = await guarded(root.resolve(A)); expect(value.word).toBe('ready'); expect(b?.word).toBe('ready'); expect(aHooks).toBe(1); expect(bHooks).toBe(1); }
  finally { if (b) await root.dispose(); }
});


test('hook containers forward autoInject setters to the actual owning container', async () => {
  /** A class resolved only after contextual auto-injection is enabled. */
  class Fresh {}
  const root = new Container(); const value = new Target(); root.registerInstance(Word, 'ready');
  root.onInstance('completion-target', (_instance, context) => {
    context.container.autoInject = true; expect(root.autoInject).toBe(true); expect(context.container.resolveSync(Fresh)).toBeInstanceOf(Fresh);
    context.container.autoInject = false; expect(root.autoInject).toBe(false);
  });
  try { root.injectMembers(value); expect(root.isRegistered(Fresh)).toBe(true); }
  finally { await root.dispose(); }
});

test('factory containers share the same target-backed accessor setter forwarding', async () => {
  /** A class resolved only after factory-context auto-injection is enabled. */
  class Fresh {}
  const root = new Container(); const token = Symbol('completion-factory-setter');
  root.singleton(token, context => {
    context.autoInject = true; expect(root.autoInject).toBe(true); const value = context.resolveSync(Fresh);
    context.autoInject = false; expect(root.autoInject).toBe(false); return value;
  });
  try { expect(root.resolveSync(token)).toBeInstanceOf(Fresh); expect(root.autoInject).toBe(false); }
  finally { await root.dispose(); }
});

test('forwarded setters use the actual target receiver even when its accessor has no getter', async () => {
  const root = new Container(); const token = Symbol('completion-setter-only'); let receiver: unknown; let observed: unknown;
  Object.defineProperty(root, 'autoInject', { configurable: true, set(this: Container, value: unknown) { receiver = this; observed = value; } });
  root.singleton(token, context => { context.autoInject = true; return 1; });
  try { expect(root.resolveSync(token)).toBe(1); expect(receiver).toBe(root); expect(observed).toBe(true); }
  finally { await root.dispose(); }
});
