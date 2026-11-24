import { expect, test } from 'vitest';
import { Container, DiGraph, DiGraphError, MutationConflictError, NotRegisteredError, ServiceLifetime, clearTokenCaches, getGlobalToken, getTokenCacheSize, normalizeToken, delay, internToken, lookupToken, releaseTokenKey } from '@coldmind/inject';
import { inspect } from '@coldmind/inject/inspect';

test('ten thousand tenant registrations release their unpinned string keys', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  try {
    for (let tenant = 0; tenant < 100; tenant++) {
      const scope = root.createTenantScope(`cache-scope-${tenant}`);
      for (let index = 0; index < 100; index++) {
        const token = `cache-stress-${tenant}-${index}`;
        const handle = index % 2 ? scope.scoped(token, { factory: () => index }) : scope.scoped(token, () => index);
        await handle.unregister();
      }
      await scope.dispose();
    }
    expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('missing resolution and inspection never intern string tokens', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  try {
    await expect(root.resolve('cache-missing-resolve')).rejects.toBeInstanceOf(NotRegisteredError);
    expect(() => root.resolveSync('cache-missing-sync')).toThrow(NotRegisteredError);
    expect(() => root.get('cache-missing-get')).toThrow(NotRegisteredError);
    expect(root.tryGet('cache-missing-try')).toBeUndefined();
    expect(root.has('cache-missing-has')).toBe(false);
    expect(root.hasRecursive('cache-missing-recursive')).toBe(false);
    expect(root.isRegistered('cache-missing-registered')).toBe(false);
    expect(root.bindingOf('cache-missing-binding')).toBeUndefined();
    expect(root.multiBindingsOf('cache-missing-multi')).toEqual([]);
    expect(root.instanceState('cache-missing-state')).toBe('uninitialized');
    expect(await root.resolveAll('cache-missing-all')).toEqual([]);
    expect(root.getDependencyTree('cache-missing-tree')).toEqual([]);
    await root.unregister('cache-missing-unregister');
    expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('public string keys remain pinned through collection and reregistration', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  const normalized = normalizeToken('cache-pinned-normalize'); const global = getGlobalToken('cache-pinned-global');
  expect(getTokenCacheSize()).toBe(before + 2);
  root.singleton('cache-pinned-normalize', () => 1); root.singleton('cache-pinned-global', () => 2);
  await root.dispose(); clearTokenCaches();
  expect(getTokenCacheSize()).toBe(before + 2);
  expect(normalizeToken('cache-pinned-normalize')).toBe(normalized); expect(getGlobalToken('cache-pinned-global')).toBe(global);
  const next = new Container();
  try {
    next.singleton('cache-pinned-normalize', () => 3); next.singleton('cache-pinned-global', () => 4);
    expect(next.resolveSync(normalized)).toBe(3); expect(next.resolveSync(global)).toBe(4);
  } finally { await next.dispose(); }
});

test('shared string keys survive another root and all multi handles release them', async () => {
  const first = new Container(); const second = new Container(); const before = getTokenCacheSize();
  try {
    const a = first.singletonMulti('cache-shared-multi', () => 1);
    const b = first.singletonMulti('cache-shared-multi', () => 2);
    second.singleton('cache-shared-multi', () => 3);
    expect(getTokenCacheSize()).toBe(before + 1);
    await a.unregister(); await b.unregister();
    expect(second.resolveSync('cache-shared-multi')).toBe(3); expect(getTokenCacheSize()).toBe(before + 1);
    await second.dispose(); expect(getTokenCacheSize()).toBe(before);
  } finally { await first.dispose(); await second.dispose(); }
});

test('declared dependency keys retain identity while their provider is absent', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  try {
    const dependent = root.transient('cache-declared-user', c => c.resolveSync('cache-declared-dep'), { deps: ['cache-declared-dep'] });
    const provider = root.singleton('cache-declared-dep', () => 1);
    expect(root.resolveSync('cache-declared-user')).toBe(1);
    await provider.unregister(); clearTokenCaches();
    expect(getTokenCacheSize()).toBe(before + 2);
    root.singleton('cache-declared-dep', () => 2);
    expect(root.resolveSync('cache-declared-user')).toBe(2); expect(() => root.validate()).not.toThrow();
    await dependent.unregister(); await root.unregister('cache-declared-dep'); expect(getTokenCacheSize()).toBe(before + 2);
    await root.dispose(); expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('dynamic factory keys survive provider removal and are released with their binding', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  try {
    const dependent = root.transient('cache-dynamic-user', c => c.resolveSync('cache-dynamic-dep'));
    root.singleton('cache-dynamic-dep', () => 1); expect(root.resolveSync('cache-dynamic-user')).toBe(1);
    await root.unregister('cache-dynamic-dep'); clearTokenCaches();
    expect(getTokenCacheSize()).toBe(before + 2);
    root.singleton('cache-dynamic-dep', () => 2); expect(root.resolveSync('cache-dynamic-user')).toBe(2);
    await dependent.unregister(); await root.unregister('cache-dynamic-dep'); expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('failed duplicate registrations do not retain new declared dependency keys', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  try {
    root.singleton('cache-duplicate', () => 1);
    expect(() => root.singleton('cache-duplicate', { factory: () => 2 }, { deps: ['cache-rejected-dep'] })).toThrow(MutationConflictError);
    clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 1);
    await root.dispose(); expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('replacement rollback and commit balance their own dependency retention', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  try {
    const original = root.singleton('cache-replace', () => 1); expect(root.resolveSync('cache-replace')).toBe(1);
    await expect(original.replace(() => { throw new Error('candidate failed'); }, ServiceLifetime.Singleton, { deps: ['cache-failed-candidate'] })).rejects.toThrow('candidate failed');
    clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 1); expect(root.resolveSync('cache-replace')).toBe(1);
    root.singleton('cache-committed-dep', () => 3);
    const replacement = await original.replace(c => c.resolveSync<number>('cache-committed-dep'), ServiceLifetime.Singleton, { deps: ['cache-committed-dep'] });
    expect(root.resolveSync('cache-replace')).toBe(3);
    await root.unregister('cache-committed-dep'); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 2);
    await replacement.unregister(); expect(getTokenCacheSize()).toBe(before + 2);
    await root.dispose(); expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});


test('sound graph handles and inspection snapshots do not permanently expose name-only keys', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  root.singleton('cache-inspect-cold', () => 1);
  root.singleton('cache-inspect-user', () => 2, { deps: ['cache-inspect-cold'] });
  expect(root.graph().size).toBe(2); expect(inspect(root).issues).toEqual([]); root.validate();
  await root.dispose(); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before);
});

test('raw public graph nodes pin only the returned keys and retain their aliases', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  root.singleton('cache-graph-exposed', () => 1);
  const node = root.graph().nodes()[0]!; await root.dispose(); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 1);
  const next = new Container();
  try { next.singleton('cache-graph-exposed', () => 2); expect(next.resolveSync(node.key)).toBe(2); }
  finally { await next.dispose(); }
});

test('public unresolved edges do not also pin their unreturned consumer key', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  root.singleton('cache-unresolved-user', () => 1, { deps: ['cache-unresolved-dependency'] });
  const edge = root.graph().unresolved()[0]!; await root.dispose(); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 1);
  const next = new Container();
  try { next.singleton('cache-unresolved-dependency', () => 2); expect(next.resolveSync(edge.key)).toBe(2); }
  finally { await next.dispose(); }
});

test('actual inspect issues deliberately preserve the raw consumer and dependency aliases', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  root.singleton('cache-inspect-broken', () => 1, { deps: ['cache-inspect-missing'] });
  const issue = inspect(root).issues[0]!; await root.dispose(); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 2);
  const next = new Container();
  try {
    next.singleton('cache-inspect-broken', () => 3); next.singleton('cache-inspect-missing', () => 4);
    expect(next.resolveSync(issue.at)).toBe(3); expect(next.resolveSync(issue.edge!.key)).toBe(4);
  } finally { await next.dispose(); }
});

test('tracked disposal and explicit registration-key enumeration keep issued aliases usable', async () => {
  const root = new Container(); const before = getTokenCacheSize(); root.singleton('cache-disposal-key', () => 1);
  await root.resolve('cache-disposal-key'); const results = await root.disposeWithTracking(); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 1);
  const next = new Container();
  try {
    next.singleton('cache-disposal-key', () => 2); expect(next.resolveSync(results[0]!.token)).toBe(2);
    next.singleton('cache-enumerated-key', () => 3); const key = next.getRegisteredTokens()[1]!;
    await next.unregister('cache-enumerated-key'); clearTokenCaches(); next.singleton('cache-enumerated-key', () => 4); expect(next.resolveSync(key)).toBe(4);
  } finally { await next.dispose(); }
});

test('interceptor context keys are explicit stable exposures even for missing registrations', async () => {
  const root = new Container(); const before = getTokenCacheSize(); let key!: symbol;
  root.addInterceptor({ onBeforeResolve(context) { key = context.token; } });
  await expect(root.resolve('cache-intercepted-missing')).rejects.toBeInstanceOf(NotRegisteredError);
  await root.dispose(); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 1);
  const next = new Container();
  try { next.singleton('cache-intercepted-missing', () => 2); expect(next.resolveSync(key)).toBe(2); }
  finally { await next.dispose(); }
});

test('failed graph validation preserves only the keys actually present in its diagnostic', async () => {
  const root = new Container(); const before = getTokenCacheSize(); root.singleton('cache-validation-user', () => 1, { deps: ['cache-validation-missing'] });
  let error!: DiGraphError;
  try { root.validate(); } catch (caught) { error = caught as DiGraphError; }
  expect(error).toBeInstanceOf(DiGraphError); await root.dispose(); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before + 2);
  const next = new Container();
  try { next.singleton('cache-validation-user', () => 2); expect(next.resolveSync(error.issues[0]!.at)).toBe(2); }
  finally { await next.dispose(); }
});

test('multi presence across roots still enforces shadowing after a sibling root unregisters', async () => {
  const first = new Container(); const second = new Container(); const child = second.createScope(); const before = getTokenCacheSize();
  first.singletonMulti('cache-multi-presence', () => 1); child.singletonMulti('cache-multi-presence', () => 2); second.singleton('cache-multi-presence', () => 3);
  await first.unregister('cache-multi-presence'); expect(() => child.resolveSync('cache-multi-presence')).toThrow(MutationConflictError);
  await child.dispose(); expect(second.resolveSync('cache-multi-presence')).toBe(3); await second.dispose(); await first.dispose(); expect(getTokenCacheSize()).toBe(before);
});


test('historical diagnostics retain removed keys until disposal or deliberate public exposure', async () => {
  const root = new Container(); const before = getTokenCacheSize(); root.singleton('cache-history-provider', () => 1);
  root.singleton('cache-history-consumer', () => 2, { deps: ['cache-history-provider'] });
  await root.unregister('cache-history-provider'); await root.unregister('cache-history-consumer'); clearTokenCaches();
  expect(getTokenCacheSize()).toBe(before + 2);
  const issue = root.issuesObserved().find(value => value.kind === 'dangling')!; expect(issue).toBeDefined();
  await root.dispose(); expect(getTokenCacheSize()).toBe(before + 2);
  const next = new Container();
  try {
    next.singleton('cache-history-provider', () => 3); next.singleton('cache-history-consumer', () => 4);
    expect(next.resolveSync(issue.at)).toBe(4); expect(next.resolveSync(issue.edge!.key)).toBe(3);
  } finally { await next.dispose(); }
});

test('historical issue retention is bounded and all unexposed log keys release on disposal', async () => {
  const root = new Container(); const before = getTokenCacheSize(); root.scoped('cache-bounded-provider', () => 1);
  try {
    for (let index = 0; index <= 20_000; index++) {
      const binding = root.singleton(`cache-bounded-consumer-${index}`, () => 1, { deps: ['cache-bounded-provider'] });
      await binding.unregister();
    }
    expect(getTokenCacheSize()).toBe(before + 10_001);
    await root.dispose(); expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
}, 20_000);

test('replacement freezes registry mutation and preserves pre-transaction diagnostic history', async () => {
  const root = new Container(); const before = getTokenCacheSize(); root.scoped('cache-rollback-history-provider', () => 1);
  const seed = root.singleton('cache-rollback-history-seed', () => 1, { deps: ['cache-rollback-history-provider'] }); await seed.unregister();
  const target = root.singleton('cache-rollback-history-target', () => 1); root.resolveSync('cache-rollback-history-target');
  try {
    await expect(target.replace(async () => {
      root.singleton('cache-rollback-history-temporary', () => 1, { deps: ['cache-rollback-history-provider'] });
    })).rejects.toBeInstanceOf(MutationConflictError);
    expect(root.isRegistered('cache-rollback-history-temporary')).toBe(false);
    expect(getTokenCacheSize()).toBe(before + 3);
    const issues = root.issuesObserved(); expect(issues).toHaveLength(1); expect(issues[0]!.message).toContain('cache-rollback-history-seed');
    expect(root.resolveSync('cache-rollback-history-target')).toBe(1);
    await root.dispose(); expect(getTokenCacheSize()).toBe(before + 2);
  } finally { await root.dispose(); }
}, 20_000);

test('invalid registration metadata collects zero-owner keys allocated before the failure', async () => {
  const root = new Container(); const before = getTokenCacheSize();
  try {
    expect(() => root.singleton('cache-invalid-registration', () => 1, { deps: ['cache-invalid-dependency', {} as never] })).toThrow('Invalid token type');
    expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('switching the last dynamic dependency does not retain an unbounded history of keys', async () => {
  const root = new Container(); const before = getTokenCacheSize(); let token = 'cache-dynamic-first';
  const user = root.transient('cache-dynamic-switch', c => c.resolveSync(token)); root.singleton('cache-dynamic-first', () => 1); root.singleton('cache-dynamic-second', () => 2);
  try {
    expect(root.resolveSync('cache-dynamic-switch')).toBe(1); token = 'cache-dynamic-second'; expect(root.resolveSync('cache-dynamic-switch')).toBe(2);
    await root.unregister('cache-dynamic-first'); expect(getTokenCacheSize()).toBe(before + 2);
    await root.unregister('cache-dynamic-second'); expect(getTokenCacheSize()).toBe(before + 2);
    await user.unregister(); expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('retained factory views cannot reacquire released binding ownership', async () => {
  const root = new Container(); const before = getTokenCacheSize(); let read!: () => unknown;
  const user = root.transient('cache-retained-view', c => { read = () => c.resolveSync('cache-retained-dependency'); return 1; });
  root.singleton('cache-retained-dependency', () => 2);
  try {
    root.resolveSync('cache-retained-view'); await user.unregister(); expect(read()).toBe(2);
    await root.unregister('cache-retained-dependency'); expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('a factory probing missing dynamic dependencies does not intern their names', async () => {
  const root = new Container(); const before = getTokenCacheSize(); root.transient('cache-probe-user', c => {
    expect(() => c.resolveSync('cache-probe-missing')).toThrow(NotRegisteredError); return 1;
  });
  expect(root.resolveSync('cache-probe-user')).toBe(1); expect(getTokenCacheSize()).toBe(before + 1);
  await root.dispose(); expect(getTokenCacheSize()).toBe(before);
});


test('public graph get, dependents and dependencies expose their exact query results', async () => {
  const root = new Container(); const base = Symbol('cache-query-base'); const user = Symbol('cache-query-user'); const missing = Symbol('cache-query-missing');
  root.singleton(base, () => 1); root.singleton(user, () => 2, { deps: [base] });
  try {
    const graph = root.graph(); expect(graph.get(base)?.token).toBe(base); expect(graph.get(missing)).toBeUndefined();
    expect(graph.dependents(base).map(node => node.token)).toEqual([user]); expect(graph.dependents(missing)).toEqual([]);
    expect(graph.dependencies(user).map(edge => edge.token)).toEqual([base]); expect(graph.dependencies(missing)).toEqual([]);
  } finally { await root.dispose(); }
});

test('a public graph query locates ancestor multi providers through their distinct graph identities', async () => {
  const root = new Container(); const child = root.createScope(); root.singletonMulti('cache-query-multi', () => 1);
  const key = root.getRegisteredTokens()[0]!;
  try {
    const node = child.graph().get(key)!; expect(node.key).toBe(key); expect(node.identity).not.toBe(key);
    expect(node.token).toBe('cache-query-multi'); expect(child.graph().get(Symbol('absent'))).toBeUndefined();
  } finally { await root.dispose(); }
});

test('default graph mutation errors expose diagnostic keys without exposing unrelated nodes', () => {
  const base = Symbol('cache-default-scoped'); const user = Symbol('cache-default-singleton'); const keys: symbol[] = [];
  const graph = new DiGraph({ onExpose: key => { keys.push(key); } });
  graph.add({ key: user, token: user, lifetime: ServiceLifetime.Singleton, declared: true, deps: [{ key: base, token: base, index: 0, optional: false, multi: false }] });
  expect(keys).toEqual([]);
  expect(() => graph.add({ key: base, token: base, lifetime: ServiceLifetime.Scoped, declared: true, deps: [] })).toThrow(DiGraphError);
  expect(keys).toEqual([user, base]);
});

test('graph ordering exposes the actual cycle diagnostic instead of unrelated traversal nodes', () => {
  const a = Symbol('cache-order-a'); const b = Symbol('cache-order-b'); const keys: symbol[] = [];
  const graph = new DiGraph({ onIssue: () => undefined, onExpose: key => { keys.push(key); } });
  graph.add({ key: a, token: a, lifetime: ServiceLifetime.Singleton, declared: true, deps: [{ key: b, token: b, index: 0, optional: false, multi: false }] });
  graph.add({ key: b, token: b, lifetime: ServiceLifetime.Singleton, declared: true, deps: [{ key: a, token: a, index: 0, optional: false, multi: false }] });
  expect(keys).toEqual([]); expect(() => graph.order()).toThrow(DiGraphError); expect(keys).toEqual([a]);
});


test('nested failed registration cannot collect an outer in-progress token key', async () => {
  const root = new Container(); const before = getTokenCacheSize(); let nested = 0;
  /** A declared class dependency whose getter reenters registration. */
  class Later {}
  const dependency = delay(() => {
    nested++;
    expect(() => root.singleton('cache-nested-invalid', () => 1, { deps: ['cache-nested-temporary', {} as never] })).toThrow('Invalid token type');
    return Later;
  });
  try {
    root.singleton('cache-outer-progress', () => 1, { deps: [dependency] });
    expect(root.resolveSync('cache-outer-progress')).toBe(1); expect(nested).toBe(1); expect(getTokenCacheSize()).toBe(before + 1);
    await root.dispose(); expect(getTokenCacheSize()).toBe(before);
  } finally { await root.dispose(); }
});

test('tracked disposal does not pin cold definitions that never produce a disposal result', async () => {
  const root = new Container(); const before = getTokenCacheSize(); root.singleton('cache-cold-tracking', () => 1);
  expect(await root.disposeWithTracking()).toEqual([]); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before);
});

test('tracked disposal preserves an inherited scoped record alias rather than only local definitions', async () => {
  const root = new Container(); const child = root.createScope(); root.scoped('cache-inherited-tracking', () => 1);
  try {
    child.resolveSync('cache-inherited-tracking'); const entries = await child.disposeWithTracking(); expect(entries).toHaveLength(1);
    await root.unregister('cache-inherited-tracking'); clearTokenCaches(); root.scoped('cache-inherited-tracking', () => 2);
    const next = root.createScope(); expect(next.resolveSync(entries[0]!.token)).toBe(2);
  } finally { await root.dispose(); }
});

test('joining untracked disposal exposes only actual retained result keys, including finished children', async () => {
  const root = new Container(); const child = root.createScope(); const before = getTokenCacheSize();
  let enter!: () => void; let release!: () => void;
  const entered = new Promise<void>(resolve => { enter = resolve; }); const gate = new Promise<void>(resolve => { release = resolve; });
  child.singleton('cache-joined-child', { factory: () => ({}), onDispose() { throw new Error('child disposal failed'); } });
  root.singleton('cache-joined-parent', { factory: () => ({}), onDispose() { enter(); return gate; } });
  child.resolveSync('cache-joined-child'); root.resolveSync('cache-joined-parent');
  const ordinary = root.dispose().catch(error => error);
  try {
    await entered; const tracked = root.disposeWithTracking(); release(); const entries = await tracked; expect(await ordinary).toBeInstanceOf(AggregateError);
    expect(entries).toHaveLength(1); expect(entries[0]!.status).toBe('failed'); expect(getTokenCacheSize()).toBe(before + 1);
    const next = new Container();
    try { next.singleton('cache-joined-child', () => 2); expect(next.resolveSync(entries[0]!.token)).toBe(2); }
    finally { await next.dispose(); }
  } finally { release(); await ordinary; }
});


test('late tracking calls on disposed containers return no keys and do not pin shared names', async () => {
  const first = new Container(); const other = new Container(); const before = getTokenCacheSize();
  first.singleton('cache-late-tracking-shared', { factory: () => ({}), onDispose() { throw new Error('already disposed'); } });
  other.singleton('cache-late-tracking-shared', () => 1); first.resolveSync('cache-late-tracking-shared');
  await expect(first.dispose()).rejects.toBeInstanceOf(AggregateError); expect(await first.disposeWithTracking()).toEqual([]);
  await other.dispose(); clearTokenCaches(); expect(getTokenCacheSize()).toBe(before);
});


test('unowned internal keys are collectible without changing public pins or live binding ownership', () => {
  const before = getTokenCacheSize(); const key = internToken('cache-unowned-collect');
  expect(lookupToken('cache-unowned-collect')).toBe(key); expect(getTokenCacheSize()).toBe(before + 1);
  clearTokenCaches(); expect(lookupToken('cache-unowned-collect')).toBeUndefined(); expect(getTokenCacheSize()).toBe(before);
  releaseTokenKey(key); expect(getTokenCacheSize()).toBe(before);
  const unowned = internToken('cache-unowned-release'); releaseTokenKey(unowned); expect(getTokenCacheSize()).toBe(before);
});
