import { expect, test } from 'vitest';
import * as API from '@coldmind/wire';

test('telemetry measures real action publication and async cancellation without changing delivery', async () => {
  expect(typeof API.ActionTelemetry).toBe('function');
  const spans: API.ActionSpanRecord[] = [];
  const logs: API.ActionLogRecord[] = [];
  const telemetry = new API.ActionTelemetry({ onSpan: span => { spans.push(span); }, onLog: log => { logs.push(log); } });
  const source = new API.Action<number>({ name: 'search', telemetry });
  const output = source.switchMap(async (value, signal) => {
    if (value === 1) await new Promise<void>((_resolve, reject) => {
      signal.addEventListener('abort', () => reject(signal.reason), { once: true });
    });
    return value * 2;
  });
  const next = output.next();
  source.emit(1).emit(2);
  expect(await next).toBe(4);
  source.dispose();
  expect(spans.some(span => span.name === 'search.emit' && span.status === 'ok')).toBeTruthy();
  expect(spans.some(span => span.name === 'search.switchMap.work' && span.status === 'cancelled')).toBeTruthy();
  expect(spans.some(span => span.name === 'search.switchMap.work' && span.status === 'ok')).toBeTruthy();
  expect(telemetry.metrics().some(metric => metric.count > 0 && metric.totalMs >= 0)).toBeTruthy();
  expect(logs.every(log => !('payload' in log))).toBeTruthy();
});

test('explicit parent contexts and bounded metrics isolate telemetry sink failures', async () => {
  expect(typeof API.ActionTelemetry).toBe('function');
  const spans: API.ActionSpanRecord[] = [];
  const failures: unknown[] = [];
  const telemetry = new API.ActionTelemetry({
    maxMetrics: 2,
    onSpan(span) { spans.push(span); throw new Error('sink failed'); },
    onLog: () => Promise.reject(new Error('async sink failed')),
    onDiagnostic: error => { failures.push(error); },
  });
  const root = telemetry.startSpan('root');
  const child = telemetry.startSpan('child', { parent: root.context });
  child.log('info', 'hello');
  child.end();
  root.end();
  telemetry.startSpan('third').end();
  await Promise.resolve();
  expect(spans[0]?.traceId).toBe(root.context.traceId);
  expect(spans[0]?.parentSpanId).toBe(root.context.spanId);
  expect(telemetry.metrics().length).toBe(2);
  expect(telemetry.droppedMetrics).toBe(1);
  expect(failures.length >= 4).toBeTruthy();
  expect(() => new API.ActionTelemetry({ maxMetrics: 0 })).toThrow(RangeError);
});

test('async output publication preserves the work trace and sync listener failures mark errors', async () => {
  const spans: API.ActionSpanRecord[] = [];
  const telemetry = new API.ActionTelemetry({ onSpan: span => { spans.push(span); } });
  const source = new API.Action<number>({ name: 'async', telemetry });
  const output = source.switchMap(async value => value);
  const pending = output.next();
  source.emit(1);
  expect(await pending).toBe(1);
  const work = spans.find(span => span.name === 'async.switchMap.work');
  const emitted = spans.find(span => span.name === 'async.switchMap.emit');
  expect(emitted?.parentSpanId).toBe(work?.spanId);
  expect(emitted?.traceId).toBe(work?.traceId);
  source.onError(() => {});
  source.onData(() => { throw new Error('callback error'); });
  source.emit(2);
  expect(spans.some(span => span.name === 'async.emit' && span.status === 'error')).toBeTruthy();
  source.dispose();
});

test('long component and action names do not break instrumented delivery', () => {
  const telemetry = new API.ActionTelemetry();
  const action = new API.Action<number>({ name: 'x'.repeat(200), telemetry });
  let pipeline = action;
  for (let index = 0; index < 20; index++) pipeline = pipeline.map(value => value + 1);
  let value = 0;
  pipeline.onData(received => { value = received; });
  action.emit(1);
  expect(value).toBe(21);
  action.dispose();
});

test('telemetry reentrancy cannot deliver cancelled work to a replacement activation', async () => {
  let release!: () => void;
  let output!: API.Action<number>;
  let replaced = false;
  const values: number[] = [];
  const telemetry = new API.ActionTelemetry({
    onSpan(span) {
      if (span.name === 'reentrant.switchMap.work' && !replaced) {
        replaced = true;
        release();
        release = output.onData(value => { values.push(value); });
      }
    },
  });
  const source = new API.Action<number>({ name: 'reentrant', telemetry });
  output = source.switchMap(async value => value);
  release = output.onData(() => {});
  source.emit(123);
  await new Promise<void>(resolve => setImmediate(resolve));
  expect(replaced).toBe(true);
  expect(values).toStrictEqual([]);
  const next = output.next({ timeout: 1000 });
  source.emit(456);
  expect(await next).toBe(456);
  expect(values).toStrictEqual([456]);
  release();
  source.dispose();
});

test('error logs contain useful summaries and the failing publication context', () => {
  const logs: API.ActionLogRecord[] = [];
  const telemetry = new API.ActionTelemetry({ onLog: log => { logs.push(log); } });
  const action = new API.Action<number>({ name: 'logging', telemetry });
  action.onError(() => {});
  action.onData(() => { throw new RangeError('Invalid amount'); });
  action.emit(1);
  expect(logs[0]?.attributes.errorName).toBe('RangeError');
  expect(logs[0]?.attributes.errorMessage).toBe('Invalid amount');
  expect(logs[0]?.context?.traceId).toBeTruthy();
  expect(logs[0]?.attributes.stack).toBe(undefined);
  action.dispose();
});
