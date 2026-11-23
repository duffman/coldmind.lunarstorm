import { Container, createToken, lazy, provider, type Lazy } from '@coldmind/inject';

/** Joins async singleton builds and breaks a scoped cycle with a deferred slot. */
export async function runExample(): Promise<string> {
  const Connection = createToken<{ ready: boolean }>('Connection');
  const Serial = createToken<number>('Serial');
  const Next = createToken<() => Promise<number>>('Next');
  class A { constructor(readonly other: Lazy<B>) {} }
  class B { constructor(readonly other: A) {} }
  const root = new Container();
  let builds = 0;
  let serial = 0;
  let disposed = 0;
  let result = '';
  try {
    root.buildTimeout = 2_000;
    root.singleton(Connection, {
      factory: async c => { c.signal.throwIfAborted(); builds++; await Promise.resolve(); return { ready: false }; },
      onInit: async value => { await Promise.resolve(); value.ready = true; },
      onDispose: value => { value.ready = false; disposed++; },
    }, { deps: [], timeoutMs: 500 });
    root.scoped(A, { deps: [lazy(B)] });
    root.scoped(B, { deps: [A] });
    root.transient(Serial, () => ++serial, { deps: [] });
    root.scoped(Next, { inject: [provider(Serial)] as const, factory: next => next });
    const scope = root.createScope();
    const [first, second] = await Promise.all([scope.resolve(Connection), scope.resolve(Connection)]);
    const a = await scope.resolve(A);
    if (a.other.peek() !== undefined) throw new Error('Lazy peek built its dependency');
    const b = await a.other.get();
    const next = await scope.resolve(Next);
    result = `shared:${first === second && first.ready && builds === 1}|cycle:${b.other === a}|provider:${await next()},${await next()}`;
  } finally {
    await root.dispose();
  }
  return `${result}|disposed:${disposed}`;
}
