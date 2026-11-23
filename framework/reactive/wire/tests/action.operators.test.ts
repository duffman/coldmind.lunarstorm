import { expect, test } from 'vitest';
import { Action, ActionCycleError, ActionDisposedError } from '@coldmind/wire';

const wait = (ms = 0) => new Promise<void>(resolve => setTimeout(resolve, ms));
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

test('map and filter derive values without changing the source', () => {
  const action = new Action<number>();
  const raw: number[] = [];
  const processed: string[] = [];
  const derived = action.filter(value => value > 1).map(value => `value:${value}`);
  action.onData(value => raw.push(value));
  derived.onData(value => processed.push(value));
  action.emit(1).emit(2).emit(3);
  expect(raw).toStrictEqual([1, 2, 3]);
  expect(processed).toStrictEqual(['value:2', 'value:3']);
  action.dispose();
  expect(derived.isDisposed).toBe(true);
});

test('derived pipelines are lazy and multicast each computation once', () => {
  const action = new Action<number>();
  let computations = 0;
  const derived = action.map(value => { computations++; return value * 2; });
  action.emit(1);
  expect(computations).toBe(0);
  const values: number[] = [];
  const a = derived.onData(value => values.push(value));
  const b = derived.onData(value => values.push(value));
  action.emit(2);
  expect(computations).toBe(1);
  expect(values).toStrictEqual([4, 4]);
  a(); b();
  action.emit(3);
  expect(computations).toBe(1);
  derived.dispose();
  expect(action.isDisposed).toBe(false);
  action.dispose();
});

test('disposing one derived branch leaves siblings and the parent alive', () => {
  const action = new Action<number>();
  const left = action.map(value => value + 1);
  const right = action.map(value => value + 2);
  const values: number[] = [];
  left.onData(value => values.push(value));
  right.onData(value => values.push(value));
  left.dispose();
  action.emit(1);
  expect(values).toStrictEqual([3]);
  expect(action.isDisposed).toBe(false);
  action.dispose();
});

test('an idle derived pipeline rejects next after its parent was disposed', async () => {
  const action = new Action<number>();
  const derived = action.map(value => value + 1).filter(value => value > 0);
  action.dispose();
  await expect(derived.next()).rejects.toThrow(ActionDisposedError);
  expect(derived.isDisposed).toBe(true);
});

test('transformation errors affect only that branch and remain nonterminal', () => {
  const action = new Action<number>();
  const error = new Error('map');
  const errors: unknown[] = [];
  const values: number[] = [];
  const derived = action.map(value => { if (value === 1) throw error; return value * 2; });
  derived.onError(reason => errors.push(reason));
  derived.onData(value => values.push(value));
  action.onData(value => values.push(value));
  action.emit(1).emit(2);
  expect(errors).toStrictEqual([error]);
  expect(values).toStrictEqual([1, 4, 2]);
  action.dispose();
});

test('upstream errors reach derived handlers without duplicate unhandled diagnostics', () => {
  const diagnostics: unknown[] = [];
  const action = new Action<number>({ onUnhandledError: error => diagnostics.push(error) });
  const error = new Error('upstream');
  const errors: unknown[] = [];
  action.map(value => value * 2).onData(() => {});
  action.filter(() => true).onError(reason => errors.push(reason));
  action.fromCallback((_emit, fail) => fail(error));
  expect(errors).toStrictEqual([error]);
  expect(diagnostics).toStrictEqual([]);
  action.dispose();
});

test('tap preserves data, runs once per shared value, and observes async failure', async () => {
  const action = new Action<number>();
  const error = new Error('tap');
  let taps = 0;
  const errors: unknown[] = [];
  const values: number[] = [];
  const derived = action.tap(async () => { taps++; throw error; });
  derived.onError(reason => errors.push(reason));
  derived.onData(value => values.push(value));
  derived.onData(value => values.push(value));
  action.emit(1);
  await wait();
  expect(taps).toBe(1);
  expect(values).toStrictEqual([1, 1]);
  expect(errors).toStrictEqual([error]);
  action.dispose();
});

test('distinct supports undefined, NaN, signed zero, custom equality, and idle reset', () => {
  const action = new Action<number | undefined>();
  const derived = action.distinct();
  const values: Array<number | undefined> = [];
  const remove = derived.onData(value => values.push(value));
  action.emit(undefined).emit(undefined).emit(NaN).emit(NaN).emit(0).emit(-0);
  remove();
  derived.onData(value => values.push(value));
  action.emit(-0);
  expect(values).toStrictEqual([undefined, NaN, 0, -0, -0]);
  action.dispose();
  const objects = new Action<{ id: number }>();
  const ids: number[] = [];
  objects.distinct((a, b) => a.id === b.id).onData(value => ids.push(value.id));
  objects.emit({ id: 1 }).emit({ id: 1 }).emit({ id: 2 });
  expect(ids).toStrictEqual([1, 2]);
  objects.dispose();
});

test('distinct comparator failures do not replace the previous accepted value', () => {
  const action = new Action<number>();
  const error = new Error('equals');
  const errors: unknown[] = [];
  const values: number[] = [];
  const derived = action.distinct((a, b) => { if (b === 2) throw error; return a === b; });
  derived.onError(reason => errors.push(reason));
  derived.onData(value => values.push(value));
  action.emit(1).emit(2).emit(1).emit(3);
  expect(errors).toStrictEqual([error]);
  expect(values).toStrictEqual([1, 3]);
  action.dispose();
});

test('debounce emits the last value after quiet and cancels pending work when idle', async () => {
  const action = new Action<number>();
  const derived = action.debounce(10);
  const values: number[] = [];
  const remove = derived.onData(value => values.push(value));
  action.emit(1).emit(2).emit(3);
  expect(values.length).toBe(0);
  await wait(30);
  expect(values).toStrictEqual([3]);
  action.emit(4);
  remove();
  await wait(30);
  expect(values).toStrictEqual([3]);
  derived.onData(value => values.push(value));
  action.emit(5);
  await wait(30);
  expect(values).toStrictEqual([3, 5]);
  action.dispose();
});

test('debounce zero remains asynchronous and source disposal discards pending values', async () => {
  const action = new Action<number>();
  const derived = action.debounce(0);
  const values: number[] = [];
  derived.onData(value => values.push(value));
  action.emit(1);
  expect(values).toStrictEqual([]);
  await wait(5);
  expect(values).toStrictEqual([1]);
  action.emit(2);
  action.dispose();
  await wait(5);
  expect(values).toStrictEqual([1]);
});

test('debounce validates all unsupported timer values', () => {
  const action = new Action<number>();
  for (const ms of [-1, NaN, Infinity, 0.5, 2_147_483_648]) {
    expect(() => action.debounce(ms)).toThrow(RangeError);
  }
  action.dispose();
});

test('switchMap aborts superseded work and ignores stale results and failures', async () => {
  const action = new Action<number>();
  const jobs = [deferred<string>(), deferred<string>(), deferred<string>()];
  const signals: AbortSignal[] = [];
  const values: string[] = [];
  const errors: unknown[] = [];
  const derived = action.switchMap((id, signal) => { signals.push(signal); return jobs[id]!.promise; });
  derived.onData(value => values.push(value));
  derived.onError(error => errors.push(error));
  action.emit(0).emit(1).emit(2);
  expect(signals[0]!.aborted).toBe(true);
  expect(signals[1]!.aborted).toBe(true);
  expect(signals[2]!.aborted).toBe(false);
  jobs[0]!.resolve('old');
  jobs[1]!.reject(new Error('old failure'));
  jobs[2]!.resolve('current');
  await wait();
  expect(values).toStrictEqual(['current']);
  expect(errors).toStrictEqual([]);
  action.dispose();
});

test('switchMap shares work and aborts the current operation on disposal', async () => {
  const action = new Action<number>();
  const work = deferred<number>();
  let calls = 0;
  let signal: AbortSignal | undefined;
  const derived = action.switchMap((_value, token) => { calls++; signal = token; return work.promise; });
  const values: number[] = [];
  derived.onData(value => values.push(value));
  derived.onData(value => values.push(value));
  action.emit(1);
  expect(calls).toBe(1);
  derived.dispose();
  expect(signal?.aborted).toBe(true);
  work.resolve(9);
  await wait();
  expect(values).toStrictEqual([]);
  expect(action.isDisposed).toBe(false);
  action.dispose();
});

test('switchMap current errors are nonterminal, including synchronous mapper exceptions', async () => {
  const action = new Action<number>();
  const sync = new Error('sync');
  const async = new Error('async');
  const errors: unknown[] = [];
  const values: number[] = [];
  const derived = action.switchMap(value => {
    if (value === 1) throw sync;
    return value === 2 ? Promise.reject(async) : Promise.resolve(value);
  });
  derived.onError(reason => errors.push(reason));
  derived.onData(value => values.push(value));
  action.emit(1).emit(2);
  await wait();
  action.emit(3);
  await wait();
  expect(errors).toStrictEqual([sync, async]);
  expect(values).toStrictEqual([3]);
  action.dispose();
});

test('connect forwards values/errors, is chainable, and does not own the source', () => {
  const source = new Action<number>();
  const receiver = new Action<number>();
  const values: number[] = [];
  const errors: unknown[] = [];
  const error = new Error('input');
  receiver.onData(value => values.push(value));
  receiver.onError(reason => errors.push(reason));
  expect(receiver.connect(source)).toBe(receiver);
  source.emit(1);
  source.fromCallback((_emit, fail) => fail(error));
  receiver.dispose();
  source.emit(2);
  expect(values).toStrictEqual([1]);
  expect(errors).toStrictEqual([error]);
  expect(source.isDisposed).toBe(false);
  source.dispose();
});

test('ending a connected source leaves the receiver usable', () => {
  const source = new Action<number>();
  const receiver = new Action<number>().connect(source);
  source.dispose();
  const values: number[] = [];
  receiver.onData(value => values.push(value));
  receiver.emit(8);
  expect(values).toStrictEqual([8]);
  receiver.dispose();
});

test('connect rejects direct, indirect, and derived dependency cycles', () => {
  const a = new Action<number>();
  const b = new Action<number>();
  const c = new Action<number>();
  expect(() => a.connect(a)).toThrow(ActionCycleError);
  b.connect(a); c.connect(b);
  expect(() => a.connect(c)).toThrow(ActionCycleError);
  const mapped = a.map(value => value * 2);
  expect(() => a.connect(mapped)).toThrow(ActionCycleError);
  a.dispose(); b.dispose(); c.dispose(); mapped.dispose();
});

test('merge forwards every source and waits for all inputs to end', async () => {
  const a = new Action<number>();
  const b = new Action<string>();
  const merged = Action.merge(a, b);
  const values: Array<number | string> = [];
  merged.onData(value => values.push(value));
  a.emit(1); b.emit('two');
  a.dispose();
  expect(merged.isDisposed).toBe(false);
  b.emit('three');
  const pending = expect(merged.next()).rejects.toThrow(ActionDisposedError);
  b.dispose();
  await pending;
  expect(values).toStrictEqual([1, 'two', 'three']);
  expect(merged.isDisposed).toBe(true);
});

test('instance merge works and disposing it leaves its inputs alone', () => {
  const a = new Action<number>();
  const b = new Action<string>();
  const c = new Action<boolean>();
  const merged = a.merge(b, c);
  const values: Array<number | string | boolean> = [];
  merged.onData(value => values.push(value));
  a.emit(1); b.emit('two'); c.emit(true);
  expect(values).toStrictEqual([1, 'two', true]);
  merged.dispose();
  expect(a.isDisposed).toBe(false);
  expect(b.isDisposed).toBe(false);
  expect(c.isDisposed).toBe(false);
  a.dispose(); b.dispose(); c.dispose();
});

test('empty merge and merging entirely disposed inputs cannot leave waits pending', async () => {
  await expect(Action.merge().next()).rejects.toThrow(ActionDisposedError);
  const a = new Action<number>();
  a.dispose();
  await expect(Action.merge(a).next()).rejects.toThrow(ActionDisposedError);
});

test('manual emission into a derived action stays downstream', () => {
  const action = new Action<number>();
  const derived = action.map(value => `value:${value}`);
  const values: string[] = [];
  action.onData(() => expect.unreachable('must not emit upstream'));
  derived.onData(value => values.push(value));
  derived.emit('manual');
  expect(values).toStrictEqual(['manual']);
  action.dispose();
});

test('operators reject creation from disposed actions', () => {
  const action = new Action<number>();
  action.dispose();
  expect(() => action.map(value => value)).toThrow(ActionDisposedError);
  expect(() => action.filter(() => true)).toThrow(ActionDisposedError);
  expect(() => action.tap(() => {})).toThrow(ActionDisposedError);
  expect(() => action.debounce(0)).toThrow(ActionDisposedError);
  expect(() => action.distinct()).toThrow(ActionDisposedError);
  expect(() => action.switchMap(value => Promise.resolve(value))).toThrow(ActionDisposedError);
  const live = new Action<number>();
  expect(() => action.merge(live)).toThrow(ActionDisposedError);
  live.dispose();
});
