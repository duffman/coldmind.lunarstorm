import { expect, test } from 'vitest';
import { Action, ActionDisposedError, ActionTimeoutError } from '@coldmind/wire';

const tick = () => new Promise<void>(resolve => setTimeout(resolve, 0));

test('emit is synchronous, chainable, and independently registers duplicate callbacks', () => {
  const action = new Action<number>();
  const values: number[] = [];
  const handler = (value: number) => values.push(value);
  const first = action.onData(handler);
  const second = action.onData(handler);
  expect(action.emit(1)).toBe(action);
  first(); first();
  action.emit(2);
  second();
  action.emit(3);
  expect(values).toStrictEqual([1, 1, 2]);
  action.dispose();
});

test('plain actions do not replay previous values', () => {
  const action = Action.create<number>();
  action.emit(1);
  const values: number[] = [];
  action.onData(value => values.push(value));
  action.emit(2);
  expect(values).toStrictEqual([2]);
  action.dispose();
});

test('reentrant emissions preserve FIFO order without overflowing the stack', () => {
  const action = new Action<number>();
  const order: string[] = [];
  action.onData(value => {
    if (value < 2) order.push(`a${value}`);
    if (value < 10_000) action.emit(value + 1);
  });
  let count = 0;
  action.onData(value => {
    count++;
    if (value < 2) order.push(`b${value}`);
  });
  action.emit(0);
  expect(order).toStrictEqual(['a0', 'b0', 'a1', 'b1']);
  expect(count).toBe(10_001);
  action.dispose();
});

test('unsubscribe skips a queued observer and additions wait for a subsequent emission', () => {
  const action = new Action<number>();
  const values: string[] = [];
  let added = false;
  let removeSecond = () => {};
  action.onData(value => {
    values.push(`first:${value}`);
    removeSecond();
    if (!added) {
      added = true;
      action.onData(n => values.push(`new:${n}`));
    }
  });
  removeSecond = action.onData(value => values.push(`second:${value}`));
  action.emit(1).emit(2);
  expect(values).toStrictEqual(['first:1', 'first:2', 'new:2']);
  action.dispose();
});

test('once removes itself before running reentrant user code', () => {
  const action = new Action<number>();
  const values: number[] = [];
  action.once(value => { values.push(value); action.emit(2); });
  action.emit(1).emit(3);
  expect(values).toStrictEqual([1]);
  action.dispose();
});

test('disposal during delivery skips later callbacks and queued emissions', () => {
  const action = new Action<number>();
  const values: number[] = [];
  action.onData(() => { action.emit(2); action.dispose(); });
  action.onData(value => values.push(value));
  action.emit(1);
  expect(values).toStrictEqual([]);
  expect(action.isDisposed).toBe(true);
  action.dispose();
  expect(() => action.emit(2)).toThrow(ActionDisposedError);
  expect(() => action.onData(() => {})).toThrow(ActionDisposedError);
  expect(() => action.onError(() => {})).toThrow(ActionDisposedError);
  expect(() => action.once(() => {})).toThrow(ActionDisposedError);
});

test('listener errors reach onError without preventing other data observers', () => {
  const action = new Action<number>();
  const error = new Error('listener failed');
  const errors: unknown[] = [];
  const values: number[] = [];
  action.onError(reason => errors.push(reason));
  action.onData(() => { throw error; });
  action.onData(value => values.push(value));
  action.emit(1).emit(2);
  expect(errors).toStrictEqual([error, error]);
  expect(values).toStrictEqual([1, 2]);
  action.dispose();
});

test('async listener rejections are observed and routed to onError', async () => {
  const action = new Action<number>();
  const error = new Error('async listener');
  const errors: unknown[] = [];
  action.onError(reason => errors.push(reason));
  action.onData(async () => { throw error; });
  action.emit(1);
  await tick();
  expect(errors).toStrictEqual([error]);
  action.dispose();
});

test('error-handler failures use the diagnostic sink without recursive onError calls', async () => {
  const diagnostics: unknown[] = [];
  const action = new Action<number>({ onUnhandledError: error => diagnostics.push(error) });
  const original = new Error('original');
  const syncFailure = new Error('error handler');
  const asyncFailure = new Error('async error handler');
  let calls = 0;
  action.onError(() => { calls++; throw syncFailure; });
  action.onError(async () => { throw asyncFailure; });
  action.onData(() => { throw original; });
  action.emit(1);
  await tick();
  expect(calls).toBe(1);
  expect(diagnostics).toStrictEqual([syncFailure, asyncFailure]);
  action.dispose();
});

test('unhandled errors are sent to the configured diagnostic sink', () => {
  const errors: unknown[] = [];
  const action = new Action<number>({ onUnhandledError: error => errors.push(error) });
  const error = new Error('unhandled');
  action.onData(() => { throw error; });
  action.emit(1);
  expect(errors).toStrictEqual([error]);
  action.dispose();
});

test('AbortSignal removes data and error subscriptions', () => {
  const action = new Action<number>();
  const control = new AbortController();
  const values: number[] = [];
  action.onData(value => values.push(value), { signal: control.signal });
  action.emit(1);
  control.abort();
  action.emit(2);
  action.onData(value => values.push(value), { signal: control.signal });
  action.emit(3);
  expect(values).toStrictEqual([1]);
  action.dispose();
});

test('next waits for exactly the next value', async () => {
  const action = new Action<number>();
  const result = action.next();
  action.emit(1).emit(2);
  expect(await result).toBe(1);
  action.dispose();
});

test('next rejects with the actual source error', async () => {
  const action = new Action<number>();
  const error = new Error('source');
  const rejected = expect(action.next()).rejects.toBe(error);
  action.fromPromise(Promise.reject(error));
  await rejected;
  action.dispose();
});

test('next rejects when disposed, including a wait created after disposal', async () => {
  const action = new Action<number>();
  const pending = expect(action.next()).rejects.toThrow(ActionDisposedError);
  action.dispose();
  await pending;
  await expect(action.next()).rejects.toThrow(ActionDisposedError);
});

test('next supports abort reasons and already-aborted signals', async () => {
  const action = new Action<number>();
  const control = new AbortController();
  const reason = new Error('cancelled');
  const pending = expect(action.next({ signal: control.signal })).rejects.toBe(reason);
  control.abort(reason);
  await pending;
  await expect(action.next({ signal: control.signal })).rejects.toBe(reason);
  action.dispose();
});

test('next times out and validates timer bounds', async () => {
  const action = new Action<number>();
  await expect(action.next({ timeout: 5 })).rejects.toThrow(ActionTimeoutError);
  for (const timeout of [-1, NaN, Infinity, 1.5, 2_147_483_648]) {
    await expect(action.next({ timeout })).rejects.toThrow(RangeError);
  }
  const pending = action.next({ timeout: 0 });
  action.emit(7);
  expect(await pending).toBe(7);
  action.dispose();
});

test('void actions accept emit without an argument', async () => {
  const action = new Action<void>();
  const pending = action.next();
  action.emit();
  expect(await pending).toBe(undefined);
  action.dispose();
});
