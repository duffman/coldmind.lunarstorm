import { expect, test } from 'vitest';
import { Action } from '@coldmind/wire';

const collect = globalThis.gc ?? (typeof Bun === 'undefined' ? undefined : () => Bun.gc(true));

async function releaseGarbage(): Promise<void> {
  if (!collect) throw new Error('no collector: Node needs --expose-gc (vitest.config.ts sets it)');
  for (let i = 0; i < 6; i++) {
    await new Promise<void>(resolve => setTimeout(resolve, 0));
    collect();
  }
}

test('an externally retained pending promise does not keep its disposed receiver alive', async () => {
  let settle!: (value: number) => void;
  const pending = new Promise<number>(resolve => { settle = resolve; });
  function attach(): WeakRef<object> {
    const marker = { payload: new Uint8Array(1024) };
    const action = new Action<number>({ onUnhandledError: () => marker });
    action.fromPromise(pending);
    action.dispose();
    return new WeakRef(marker);
  }
  const ref = attach();
  await releaseGarbage();
  expect(ref.deref(), 'disposed receiver is still retained').toBeUndefined();
  // The source promise remains live through this assertion and settles afterwards.
  settle(1);
  await pending;
});

test('an uncancellable async operation does not keep a disposed pipeline alive', async () => {
  let settle!: (value: number) => void;
  const pending = new Promise<number>(resolve => { settle = resolve; });
  function attach(): WeakRef<object> {
    const marker = { payload: new Uint8Array(1024) };
    const source = new Action<number>({ onUnhandledError: () => marker });
    const derived = source.switchMap(() => pending);
    derived.onData(() => {});
    source.emit(1);
    derived.dispose(); source.dispose();
    return new WeakRef(marker);
  }
  const ref = attach();
  await releaseGarbage();
  expect(ref.deref(), 'disposed pipeline is still retained').toBeUndefined();
  settle(1);
  await pending;
});

for (const kind of ['listener', 'tap', 'error handler'] as const) {
  test(`a pending ${kind} promise does not keep a disposed action alive`, async () => {
    let settle!: () => void;
    const pending = new Promise<void>(resolve => { settle = resolve; });
    function attach(): WeakRef<object> {
      const marker = { payload: new Uint8Array(1024) };
      const source = new Action<number>({ onUnhandledError: () => marker });
      if (kind === 'listener') source.onData(() => pending);
      if (kind === 'tap') source.tap(() => pending).onData(() => {});
      if (kind === 'error handler') {
        source.onError(() => pending);
        source.onData(() => { throw new Error('failure'); });
      }
      source.emit(1);
      source.dispose();
      return new WeakRef(marker);
    }
    const ref = attach();
    await releaseGarbage();
    expect(ref.deref(), 'disposed action is still retained').toBeUndefined();
    settle();
    await pending;
  });
}
