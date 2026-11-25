import { expect, test } from 'vitest';
import * as API from '@coldmind/wire';

test('component factory wires typed ports after observers attach and releases every owned resource', async () => {
  expect(typeof API.defineActionComponent).toBe('function');
  const order: string[] = [];
  const definition = API.defineActionComponent({
    name: 'multiplier',
    inputs: { value: API.actionPort<number>() },
    outputs: { result: API.actionPort<number>() },
    setup(ctx) {
      ctx.own(() => { order.push('first'); });
      ctx.own(() => { order.push('second'); });
      ctx.outputs.result.connect(ctx.inputs.value.map(value => value * 2));
    },
  });
  const component = definition.create({
    onCreate: () => { order.push('onCreate'); },
    onStart: () => { order.push('onStart'); },
    onDestroy: () => { order.push('onDestroy'); },
  });
  const received: number[] = [];
  component.outputs.result.onData(value => received.push(value));
  expect(component.start()).toBe(component.start());
  await component.start();
  component.inputs.value.emit(21);
  expect(received).toStrictEqual([42]);
  expect(component.destroy()).toBe(component.destroy());
  await component.destroy();
  expect(order).toStrictEqual(['onCreate', 'onStart', 'second', 'first', 'onDestroy']);
  expect(component.inputs.value.isDisposed).toBe(true);
  expect(component.outputs.result.isDisposed).toBe(true);
  await expect(component.start()).rejects.toThrow(API.ActionDisposedError);
});

test('class template and functional template share the same typed Action ports', async () => {
  expect(typeof API.ActionComponent).toBe('function');
  class Multiply extends API.ActionComponent<{ value: number }, { result: number }> {
    constructor() {
      super({ name: 'multiply', inputs: { value: API.actionPort<number>() }, outputs: { result: API.actionPort<number>() } });
    }
    protected override configure(ctx: API.ActionComponentContext<{ value: number }, { result: number }>): void {
      ctx.outputs.result.connect(ctx.inputs.value.map(value => value * 3));
    }
  }
  const component = new Multiply();
  await component.start();
  const value = component.outputs.result.next();
  component.inputs.value.emit(7);
  expect(await value).toBe(21);
  await component.destroy();
});

test('destroying during async setup rejects startup and releases late resources', async () => {
  expect(typeof API.defineActionComponent).toBe('function');
  let acquire!: (value: object) => void;
  let entered!: () => void;
  const ready = new Promise<void>(resolve => { entered = resolve; });
  const resource = new Promise<object>(resolve => { acquire = resolve; });
  let released = false;
  const component = API.defineActionComponent({
    name: 'late', inputs: {}, outputs: {},
    async setup(ctx) {
      entered();
      await ctx.resource(() => resource, () => { released = true; });
    },
  }).create();
  const startup = component.start();
  await ready;
  await component.destroy();
  await expect(startup).rejects.toThrow(API.ActionDisposedError);
  acquire({});
  await new Promise<void>(resolve => setImmediate(resolve));
  expect(released).toBe(true);
  expect(component.state).toBe('destroyed');
});

test('cleanup failures do not prevent other cleanups or port disposal', async () => {
  expect(typeof API.defineActionComponent).toBe('function');
  let released = false;
  const component = API.defineActionComponent({
    name: 'cleanup', inputs: { value: API.actionPort<number>() }, outputs: {},
    setup(ctx) {
      ctx.own(() => { released = true; });
      ctx.own(() => { throw new Error('cleanup failure'); });
    },
  }).create();
  await component.start();
  await expect(component.destroy()).rejects.toThrow(AggregateError);
  expect(released).toBe(true);
  expect(component.inputs.value.isDisposed).toBe(true);
});

test('destruction from onStart cannot reopen the component', async () => {
  let component!: API.ActionComponent<{ value: number }, object>;
  component = API.defineActionComponent({
    name: 'reentrant.start', inputs: { value: API.actionPort<number>() }, outputs: {},
  }).create({ onStart: () => component.destroy() });
  await expect(component.start()).rejects.toThrow(API.ActionDisposedError);
  await component.destroy();
  expect(component.state).toBe('destroyed');
  expect(component.inputs.value.isDisposed).toBe(true);
});
