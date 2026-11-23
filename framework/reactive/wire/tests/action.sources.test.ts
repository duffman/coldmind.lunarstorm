import { expect, test } from 'vitest';
import { Action, ActionDisposedError } from '@coldmind/wire';

const tick = () => new Promise<void>(resolve => setTimeout(resolve, 0));

test('event adapters map events and detach on disposal', () => {
  const target = new EventTarget();
  const action = new Action<number>();
  const values: number[] = [];
  let mapped = 0;
  action.onData(value => values.push(value));
  action.fromEvent<CustomEvent<number>>(target, 'value', event => { mapped++; return event.detail; });
  target.dispatchEvent(new CustomEvent('value', { detail: 4 }));
  action.dispose();
  target.dispatchEvent(new CustomEvent('value', { detail: 9 }));
  expect(values).toStrictEqual([4]);
  expect(mapped).toBe(1);
});

test('event mapper errors reach the receiving action', () => {
  const target = new EventTarget();
  const action = new Action<number>();
  const error = new Error('bad event');
  const errors: unknown[] = [];
  action.onError(reason => errors.push(reason));
  action.fromEvent(target, 'value', () => { throw error; });
  target.dispatchEvent(new Event('value'));
  expect(errors).toStrictEqual([error]);
  action.dispose();
});

test('event options support external abort and once without reentrant duplicates', () => {
  const target = new EventTarget();
  const action = new Action<number>();
  const control = new AbortController();
  let count = 0;
  action.fromEvent(target, 'value', () => ++count, { signal: control.signal });
  target.dispatchEvent(new Event('value'));
  control.abort();
  target.dispatchEvent(new Event('value'));
  action.fromEvent(target, 'once', () => { target.dispatchEvent(new Event('once')); return ++count; }, { once: true });
  target.dispatchEvent(new Event('once'));
  target.dispatchEvent(new Event('once'));
  expect(count).toBe(2);
  action.dispose();
});

test('static event sources activate lazily, share delivery, and reactivate after idle', () => {
  const target = new EventTarget();
  const action = Action.fromEvent(target, 'value');
  const values: Event[] = [];
  const event = new Event('value');
  target.dispatchEvent(event);
  const a = action.onData(value => values.push(value));
  const b = action.onData(value => values.push(value));
  target.dispatchEvent(event);
  a(); b();
  target.dispatchEvent(event);
  const c = action.onData(value => values.push(value));
  target.dispatchEvent(event);
  c();
  expect(values).toStrictEqual([event, event, event]);
  action.dispose();
});

test('callback instance adapters register immediately and abort before cleaning up once', () => {
  const action = new Action<number>();
  const values: number[] = [];
  const lifecycle: string[] = [];
  let send: (value: number) => void = () => {};
  action.onData(value => values.push(value));
  action.fromCallback((emit, _error, signal) => {
    send = emit;
    lifecycle.push('registered');
    signal.addEventListener('abort', () => lifecycle.push('aborted'), { once: true });
    emit(1);
    return () => lifecycle.push('cleaned');
  });
  send(2);
  action.dispose(); action.dispose();
  send(3);
  expect(values).toStrictEqual([1, 2]);
  expect(lifecycle).toStrictEqual(['registered', 'aborted', 'cleaned']);
});

test('callback cleanup returned after synchronous disposal still executes', () => {
  const action = new Action<number>();
  let cleaned = 0;
  action.onData(() => action.dispose());
  action.fromCallback(emit => { emit(1); return () => { cleaned++; }; });
  expect(cleaned).toBe(1);
});

test('callback registration exceptions and cleanup exceptions reach onError', () => {
  const action = new Action<number>();
  const registration = new Error('registration');
  const cleanup = new Error('cleanup');
  const errors: unknown[] = [];
  action.onError(error => errors.push(error));
  action.fromCallback(() => { throw registration; });
  action.fromCallback(() => () => { throw cleanup; });
  action.dispose();
  expect(errors).toStrictEqual([registration, cleanup]);
});

test('static callbacks handle synchronous once and next without leaving a registration alive', async () => {
  let registered = 0;
  let cleaned = 0;
  const action = Action.fromCallback<number>(emit => {
    registered++;
    emit(1); emit(2);
    return () => { cleaned++; };
  });
  const values: number[] = [];
  expect(registered).toBe(0);
  action.once(value => values.push(value));
  expect(values).toStrictEqual([1]);
  expect(cleaned).toBe(1);
  expect(await action.next()).toBe(1);
  expect(registered).toBe(2);
  expect(cleaned).toBe(2);
  action.dispose();
});

test('already-aborted subscription does not activate a callback factory', () => {
  let registered = 0;
  const action = Action.fromCallback<number>(() => { registered++; });
  const control = new AbortController();
  control.abort();
  action.onData(() => {}, { signal: control.signal });
  action.onError(() => {}, { signal: control.signal });
  expect(registered).toBe(0);
  action.dispose();
});

test('static callbacks share registration until the last observer leaves', () => {
  let registered = 0;
  let cleaned = 0;
  const action = Action.fromCallback<number>(() => {
    registered++;
    return () => { cleaned++; };
  });
  const a = action.onData(() => {});
  const b = action.onError(() => {});
  expect(registered).toBe(1);
  a();
  expect(cleaned).toBe(0);
  b();
  expect(cleaned).toBe(1);
  action.dispose();
});

test('promise instance adapters are hot and static factories retain their settlement', async () => {
  const instance = new Action<number>();
  instance.fromPromise(Promise.resolve(1));
  const factory = Action.fromPromise(Promise.resolve(2));
  await tick();
  const values: number[] = [];
  instance.onData(value => values.push(value));
  factory.onData(value => values.push(value));
  expect(values).toStrictEqual([2]);
  expect(await factory.next()).toBe(2);
  const once: number[] = [];
  factory.once(value => once.push(value));
  factory.emit(3);
  expect(once).toStrictEqual([2]);
  instance.dispose(); factory.dispose();
});

test('promise factories retain rejection for late error observers and next', async () => {
  const error = new Error('failed');
  const action = Action.fromPromise<number>(Promise.reject(error));
  await tick();
  const errors: unknown[] = [];
  action.onError(reason => errors.push(reason));
  expect(errors).toStrictEqual([error]);
  await expect(action.next()).rejects.toBe(error);
  action.dispose();
});

test('a rejection with no error handler at settlement is reported and stays retained', async () => {
  const error = new Error('unobserved');
  const reported: unknown[] = [];
  const action = Action.fromPromise<number>(Promise.reject(error), { onUnhandledError: reason => { reported.push(reason); } });
  await tick();
  expect(reported).toStrictEqual([error]);
  const data: number[] = [];
  action.onData(value => data.push(value));
  expect(data).toStrictEqual([]);
  await expect(action.next()).rejects.toBe(error);
  expect(reported).toStrictEqual([error]);
  action.dispose();
});

test('a rejection handled by an error observer before settlement is not reported', async () => {
  let fail: (reason: unknown) => void = () => {};
  const pending = new Promise<number>((_done, reject) => { fail = reject; });
  const reported: unknown[] = [];
  const action = Action.fromPromise(pending, { onUnhandledError: reason => { reported.push(reason); } });
  const seen: unknown[] = [];
  action.onError(reason => { seen.push(reason); });
  const error = new Error('handled');
  fail(error);
  await tick();
  expect(seen).toStrictEqual([error]);
  expect(reported).toStrictEqual([]);
  action.dispose();
});

test('promise factories replay undefined as a real value', async () => {
  const action = Action.fromPromise(Promise.resolve(undefined));
  await tick();
  expect(await action.next({ timeout: 20 })).toBe(undefined);
  action.dispose();
});

test('late resolution and rejection after disposal are safely ignored', async () => {
  let resolve: (value: number) => void = () => {};
  let reject: (reason: unknown) => void = () => {};
  const errors: unknown[] = [];
  const action = new Action<number>({ onUnhandledError: error => errors.push(error) });
  action.fromPromise(new Promise<number>(done => { resolve = done; }));
  action.fromPromise(new Promise<number>((_done, fail) => { reject = fail; }));
  action.dispose();
  resolve(1); reject(new Error('late'));
  await tick();
  expect(errors).toStrictEqual([]);
});

test('disposed actions reject source attachments', () => {
  const action = new Action<number>();
  action.dispose();
  expect(() => action.fromEvent(new EventTarget(), 'x', () => 1)).toThrow(ActionDisposedError);
  expect(() => action.fromPromise(Promise.resolve(1))).toThrow(ActionDisposedError);
  expect(() => action.fromCallback(() => {})).toThrow(ActionDisposedError);
});
