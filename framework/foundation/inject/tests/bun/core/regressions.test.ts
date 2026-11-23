import { describe, expect, test } from 'vitest';
import { Container, DiError, CircularDependencyError, DiGraphError, LifecycleError, NotRegisteredError, createToken } from '@coldmind/inject';

const within = <T>(promise: Promise<T>, ms = 1000): Promise<T> =>
  Promise.race([promise, new Promise<T>((_, reject) => setTimeout(() => reject(new Error('timed out (hang)')), ms))]);

describe('concurrent resolution', () => {
  test('concurrent resolves of independent chains do not report a false cycle', async () => {
    const root = new Container();
    const A = createToken<number>('A'); const B = createToken<number>('B'); const C = createToken<number>('C');
    root.singleton(C, async () => 3);
    root.singleton(B, async c => (await c.resolve(C)) + 1);
    root.singleton(A, async c => (await c.resolve(B)) + 1);
    const [a, b] = await Promise.all([root.resolve(A), root.resolve(B)]);
    expect(a).toBe(5);
    expect(b).toBe(4);
  });

  test('a cross-chain A↔B cycle rejects instead of hanging when factories await first', async () => {
    const root = new Container();
    const A = createToken<number>('A'); const B = createToken<number>('B');
    root.singleton(A, async c => { await Promise.resolve(); return (await c.resolve(B)) + 1; });
    root.singleton(B, async c => { await Promise.resolve(); return (await c.resolve(A)) + 1; });
    const results = await within(Promise.allSettled([root.resolve(A), root.resolve(B)]));
    expect(results.every(r => r.status === 'rejected')).toBe(true);
    const reasons = results.map(r => (r as PromiseRejectedResult).reason);
    expect(reasons.some(e => e instanceof CircularDependencyError)).toBe(true);
  });

  test('a synchronous factory throw runs the factory once for all concurrent callers', async () => {
    const root = new Container();
    const T = createToken<number>('T');
    let calls = 0;
    root.singleton(T, () => { calls++; throw new Error('boom'); });
    const results = await Promise.allSettled(Array.from({ length: 50 }, () => root.resolve(T)));
    expect(results.every(r => r.status === 'rejected')).toBe(true);
    expect(calls).toBe(1);
  });

  test('resolveAll does not share one batch between callers', async () => {
    const root = new Container();
    const M = createToken<{ id: number }>('M');
    let built = 0;
    root.transientMulti(M, () => ({ id: ++built }));
    await root.resolveAll(M);
    await root.resolveAll(M);
    expect(built).toBe(2);
  });
});

describe('disposal', () => {
  test('initialization cannot start another service after disposal begins', async () => {
    const root = new Container();
    let finish!: (value: object) => void;
    const first = new Promise<object>(resolve => { finish = resolve; });
    let disposed = 0;
    let laterBuilds = 0;
    root.singleton('first', { factory: () => first, onDispose: () => { disposed++; } });
    root.singleton('later', async () => { laterBuilds++; return {}; });
    const initialization = root.initializeOwned();
    const outcome = initialization.then(() => undefined, error => error);
    const disposal = root.dispose();
    finish({});
    const [error] = await within(Promise.all([outcome, disposal]));
    expect(error).toBeInstanceOf(DiError);
    expect(error.code).toBe('DI_BUILD_CANCELLED');
    expect(laterBuilds).toBe(0);
    expect(disposed).toBe(1);
    expect(root.isDisposed).toBe(true);
  });

  test('disposal cancels a pending singleton and cleans its late value once', async () => {
    const root = new Container(); let finish!: (value: object) => void; let disposed = 0;
    root.singleton('pending', { factory: () => new Promise<object>(resolve => { finish = resolve; }), onDispose: () => { disposed++; } });
    const result = root.resolve('pending').catch(error => error);
    await within(root.dispose()); expect(await result).toMatchObject({ code: 'DI_BUILD_CANCELLED' }); expect(disposed).toBe(0);
    finish({}); await new Promise(resolve => setTimeout(resolve, 0)); expect(disposed).toBe(1);
  });

  test('a cancelled pending factory rejection is consumed without disposal', async () => {
    const root = new Container(); let fail!: (error: Error) => void; let disposed = 0;
    root.singleton('failing-pending', { factory: () => new Promise<object>((_, reject) => { fail = reject; }), onDispose: () => { disposed++; } });
    const result = root.resolve('failing-pending').catch(error => error);
    await within(root.dispose()); expect(await result).toMatchObject({ code: 'DI_BUILD_CANCELLED' });
    fail(new Error('construction failed')); await new Promise(resolve => setTimeout(resolve, 0)); expect(disposed).toBe(0);
  });

  test('cancelled pending onInit cleans its late failed instance once', async () => {
    const root = new Container(); let fail!: (error: Error) => void; let disposed = 0;
    const init = new Promise<void>((_, reject) => { fail = reject; });
    root.singleton('failing-init', { factory: () => ({}), onInit: () => init, onDispose: () => { disposed++; } });
    const result = root.resolve('failing-init').catch(error => error);
    await within(root.dispose()); expect(await result).toMatchObject({ code: 'DI_BUILD_CANCELLED' });
    fail(new Error('initialization failed')); await new Promise(resolve => setTimeout(resolve, 0)); expect(disposed).toBe(1);
  });

  test('concurrent dispose runs each onDispose once', async () => {
    const root = new Container();
    const S = createToken<object>('S');
    let disposed = 0;
    root.singleton(S, { factory: () => ({}), onDispose: () => { disposed++; } });
    await root.resolve(S);
    await Promise.all([root.dispose(), root.dispose()]);
    expect(disposed).toBe(1);
  });

  test('resolve while disposing throws LifecycleError', async () => {
    const root = new Container();
    const S = createToken<number>('S');
    const T = createToken<number>('T');
    let during: unknown;
    root.singleton(S, { factory: () => 1, onDispose: async () => {
      try { await root.resolve(T); } catch (error) { during = error; }
    } });
    root.singleton(T, () => 2);
    await root.resolve(S);
    await root.dispose();
    expect(during).toBeInstanceOf(LifecycleError);
  });

  test('a falsy singleton is a valid instance', async () => {
    const root = new Container();
    const Zero = createToken<number>('zero');
    root.singleton(Zero, () => 0);
    await root.resolve(Zero);
    expect(root.get(Zero)).toBe(0);
    expect(root.tryGet(Zero)).toBe(0);
  });
});

describe('registrations', () => {
  test('a child single shadows a parent multi registration for resolve', async () => {
    const root = new Container();
    const P = createToken<string>('P');
    root.transientMulti(P, () => 'parent');
    const child = root.createScope();
    child.singleton(P, () => 'child');
    expect(await child.resolve(P)).toBe('child');
  });

  test('a multi registration at the single\'s owner conflicts', async () => {
    const root = new Container();
    const P = createToken<string>('P');
    const child = root.createScope();
    child.transientMulti(P, () => 'child-multi');
    root.singleton(P, () => 'root-single');
    await expect(child.resolve(P)).rejects.toThrow(/resolveAll/);
  });
});

describe('graph', () => {
  test('a 20-layer diamond validates with bounded edge traversals and dependency-first order', () => {
    const root = new Container();
    const layers: symbol[][] = [];
    for (let layer = 0; layer < 20; layer++) {
      const row: symbol[] = [];
      for (let i = 0; i < 2; i++) {
        const token = createToken<unknown>(`L${layer}_${i}`);
        row.push(token as unknown as symbol);
        const deps = layer === 0 ? [] : layers[layer - 1]!.map(t => t as never);
        root.singleton(token, { factory: () => ({}), onInit: undefined }, { deps: deps as never });
      }
      layers.push(row);
    }
    const graph = root.graph();
    const nodes = graph.nodes();
    let reads = 0;
    for (const node of nodes) {
      const deps = node.deps;
      Object.defineProperty(node, 'deps', { get() { reads++; return deps; } });
    }
    graph.assertSound();
    const ordered = graph.order();
    expect(reads).toBeLessThanOrEqual(nodes.length * 12);
    expect(ordered).toHaveLength(40);
    expect(new Set(ordered).size).toBe(40);
    const positions = new Map(ordered.map((node, index) => [node.key, index]));
    for (const node of ordered) for (const edge of node.deps) expect(positions.get(edge.key)).toBeLessThan(positions.get(node.key)!);
  });

  test('captive singleton depending on scoped is reported', () => {
    const root = new Container();
    const Scoped = createToken<number>('scoped');
    const Captive = createToken<number>('captive');
    root.scoped(Scoped, () => 1);
    root.singleton(Captive, () => 2, { deps: [Scoped] });
    expect(root.graph().issues().some(i => i.kind === 'captive')).toBe(true);
    expect(() => root.graph().assertSound()).toThrow(DiGraphError);
  });

  test('a declared cycle is reported once, not once per member', () => {
    const root = new Container();
    const A = createToken<unknown>('A'); const B = createToken<unknown>('B'); const C = createToken<unknown>('C');
    root.transient(A, () => 1, { deps: [B] });
    root.transient(B, () => 1, { deps: [C] });
    root.transient(C, () => 1, { deps: [A] });
    const cycles = root.graph().issues().filter(i => i.kind === 'cycle');
    expect(cycles.length).toBe(1);
    expect(cycles[0]!.path?.length).toBe(4);
  });
});

describe('lifetimes', () => {
  test('a cached scoped service cannot be captured by a singleton factory', async () => {
    const root = new Container();
    const Scoped = createToken<object>('cached-scoped');
    const Captive = createToken<object>('captive');
    root.scoped(Scoped, () => ({}));
    root.singleton(Captive, async c => c.resolve(Scoped));
    await root.resolve(Scoped);
    await expect(root.resolve(Captive)).rejects.toBeInstanceOf(DiGraphError);
    await root.dispose();
  });

  test('a request-scoped service builds on first resolve, not on acquire', async () => {
    const root = new Container();
    const R = createToken<number>('R');
    let built = 0;
    root.request(R, { factory: () => ++built });
    const request = root.createRequestScope();
    expect(built).toBe(0);
    expect(await request.resolve(R)).toBe(1);
  });
});

describe('optional class injection', () => {
  test('an absent optional dependency resolves to undefined', async () => {
    class Consumer { static $members = { optional: { token: 'absent', optional: true } }; optional?: unknown; }
    const root = new Container();
    root.singleton(Consumer);
    expect((await root.resolve(Consumer)).optional).toBeUndefined();
    await root.dispose();
  });

  test('a registered optional dependency propagates a missing transitive dependency', async () => {
    class Consumer { static $members = { optional: { token: 'optional-service', optional: true } }; optional?: unknown; }
    const root = new Container();
    root.singleton('optional-service', c => c.resolve('missing-transitive'));
    root.singleton(Consumer);
    await expect(root.resolve(Consumer)).rejects.toMatchObject({ token: 'missing-transitive' } satisfies Partial<NotRegisteredError>);
    await root.dispose();
  });

  test('a registered optional dependency does not hide its own NotRegisteredError', async () => {
    class Consumer { static $members = { optional: { token: 'optional-service', optional: true } }; optional?: unknown; }
    const root = new Container();
    root.singleton('optional-service', () => { throw new NotRegisteredError('optional-service', 'optional-service'); });
    root.singleton(Consumer);
    await expect(root.resolve(Consumer)).rejects.toBeInstanceOf(NotRegisteredError);
    await root.dispose();
  });
});
