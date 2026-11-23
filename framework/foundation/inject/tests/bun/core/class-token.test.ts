import { describe, expect, test } from 'vitest';
import { LifecycleError, Container, DiGraphError, delay } from '@coldmind/inject';

class Clock { now(): number { return 42; } }
class Greeter {
  constructor(readonly clock: Clock) {}
  greet(): string { return `hi@${this.clock.now()}`; }
}

describe('a class as its own token', () => {
  test('singleton(Class) builds the class once and resolves synchronously', () => {
    const root = new Container();
    root.singleton(Clock);
    const clock = root.resolveSync(Clock);
    expect(clock).toBeInstanceOf(Clock);
    expect(clock.now()).toBe(42);
    expect(root.get(Clock)).toBe(clock);
  });

  test('transient(Class) builds a new instance per resolve', () => {
    const root = new Container();
    root.transient(Clock);
    expect(root.resolveSync(Clock)).not.toBe(root.resolveSync(Clock));
  });

  test('scoped(Class) builds one instance per scope', () => {
    const root = new Container();
    root.scoped(Clock);
    const a = root.createScope(); const b = root.createScope();
    expect(a.resolveSync(Clock)).toBe(a.resolveSync(Clock));
    expect(a.resolveSync(Clock)).not.toBe(b.resolveSync(Clock));
  });

  test('options.deps are passed as constructor arguments and declared in the graph', async () => {
    const root = new Container();
    root.singleton(Clock);
    root.singleton(Greeter, { deps: [Clock] });
    root.validate();
    const greeter = await root.resolve(Greeter);
    expect(greeter.greet()).toBe('hi@42');
    expect(greeter.clock).toBe(await root.resolve(Clock));
  });

  test('a class whose dependencies are synchronous resolves through resolveSync', () => {
    const root = new Container();
    root.singleton(Clock);
    root.singleton(Greeter, { deps: [Clock] });
    const greeter = root.resolveSync(Greeter);
    expect(greeter.greet()).toBe('hi@42');
    expect(greeter.clock).toBe(root.resolveSync(Clock));
  });

  test('a class with an asynchronous dependency does not resolve through resolveSync', () => {
    const root = new Container();
    root.singleton(Clock, async () => new Clock());
    root.singleton(Greeter, { deps: [Clock] });
    expect(() => root.resolveSync(Greeter)).toThrow(LifecycleError);
  });

  test('a missing constructor dependency is a graph issue', () => {
    const root = new Container();
    root.singleton(Greeter, { deps: [Clock] });
    expect(() => root.validate()).toThrow(DiGraphError);
  });

  test('static $inject supplies constructor arguments without a registration decorator', async () => {
    class Report { static $inject = [Clock]; constructor(readonly clock: Clock) {} }
    const root = new Container();
    root.singleton(Clock);
    root.transient(Report);
    expect((await root.resolve(Report)).clock).toBe(await root.resolve(Clock));
  });

  test('a delayed class reference registers under the class', () => {
    const root = new Container();
    root.singleton(delay(() => Clock));
    expect(root.resolveSync(Clock)).toBeInstanceOf(Clock);
  });

  test('an abstract class is a token for an implementation', () => {
    abstract class Store { abstract read(): string; }
    class MemoryStore extends Store { read(): string { return 'mem'; } }
    const root = new Container();
    root.singleton(Store, () => new MemoryStore());
    const store: Store = root.resolveSync(Store);
    expect(store.read()).toBe('mem');
  });

  test('a lifecycle object is still a factory, not options', () => {
    const root = new Container();
    root.singleton(Clock, { factory: () => new Clock() });
    expect(root.resolveSync(Clock)).toBeInstanceOf(Clock);
  });

  test('a multi registration under the class token', () => {
    const root = new Container();
    root.singletonMulti(Clock);
    root.singletonMulti(Clock, () => new Clock());
    return root.resolveAll(Clock).then(all => expect(all).toHaveLength(2));
  });

  test('a string or symbol token without a factory is rejected', () => {
    const root = new Container();
    expect(() => (root as any).singleton('clock')).toThrow(TypeError);
    expect(() => (root as any).singleton(Symbol('clock'))).toThrow(TypeError);
  });
});
