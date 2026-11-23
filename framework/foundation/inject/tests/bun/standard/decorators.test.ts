import { expect, test } from 'vitest';
import {
  Container, Injectable, Singleton, Inject, InjectAll, InjectOptional, defineDecorator,
  describe, DiGraphError, ServiceLifetime,
} from '@coldmind/inject';

const database = Symbol('database');
const hookOrder: string[] = [];

@Singleton({ token: 'service', deps: [database], tags: ['service'] })
class StandardService {
  @Inject('configuration') configuration!: { region: string };
  @InjectOptional('unknown-option') optional?: number;
  @InjectAll('providers') providers!: number[];
  constructor(readonly db: { connected: boolean }) {}
  onInit() {
    expect(this.db.connected).toBe(true);
    expect(this.configuration.region).toBe('eu');
    expect(this.optional).toBe(undefined);
    expect(this.providers).toStrictEqual([1, 2]);
    hookOrder.push('onInit');
  }
}

test('standard decorators wire parameters, fields, optional fields, and multi fields before init', async () => {
  const root = new Container();
  root.registerInstance(database, { connected: true });
  root.registerInstance('configuration', { region: 'eu' });
  root.singletonMulti('providers', () => 1);
  root.singletonMulti('providers', () => 2);
  root.registerClass(StandardService);
  root.validate();
  const value = await root.resolve<StandardService>('service');
  expect(value instanceof StandardService).toBeTruthy();
  expect(value.db.connected).toBe(true);
  expect(value.configuration.region).toBe('eu');
  expect(hookOrder).toStrictEqual(['onInit']);
  await root.dispose();
});

@Singleton()
class InjectedPrivate {
  @Inject('word') accessor #hidden!: string;
  get value() { return this.#hidden; }
}

test('standard private auto-accessor injection uses context.access', async () => {
  const c = new Container();
  c.registerInstance('word', 'secret');
  c.registerClass(InjectedPrivate);
  expect((await c.resolve(InjectedPrivate)).value).toBe('secret');
  await c.dispose();
});

const Observe = defineDecorator<[]>({
  name: '@Observe', on: ['method'],
  apply(t) { t.method({ source: 'observe' }); t.validate(() => 'validation warning'); }
});
const Wrap = defineDecorator<[]>({
  name: '@Wrap', on: ['method'],
  apply(t) { t.wrap((previous) => (...args) => `wrapped:${previous(...args)}`); }
});
const InjectDerived = defineDecorator<[string]>({
  name: '@InjectDerived', on: ['field'],
  apply(t, token) { t.inject({ token, derive: value => `derived:${value}` }); }
});

@Injectable({ tags: ['tagged'] })
class Customized {
  @InjectDerived('suffix') suffix!: string;
  @Observe() @Wrap() work(): string { return this.suffix; }
}

test('custom method records, per-instance wrapping, derive, validators, discover, hooks', async () => {
  const c = new Container();
  c.registerInstance('suffix', 'hello');
  c.registerClass(Customized);
  const discovered = c.discover('@Observe');
  expect(discovered.length).toBe(1);
  expect(discovered[0]!.methods[0]!.name).toBe('work');
  expect(c.discover('tagged').length).toBe(1);
  const called: string[] = [];
  c.onInstance('@Observe', () => { called.push('hook'); });
  const x = await c.resolve(Customized);
  expect(x.work()).toBe('wrapped:derived:hello');
  expect(Object.hasOwn(x, 'work')).toBe(true);
  expect(Object.hasOwn(Customized.prototype, 'work')).toBe(true);
  expect(new Customized().work()).toBe(undefined);
  expect(called).toStrictEqual(['hook']);
  expect(() => c.validate()).toThrow(expect.toSatisfy((e: unknown) => e instanceof DiGraphError && e.issues.some(i => i.kind === 'decorator')));
  await c.dispose();
});

test('registration decorator conflict with $injectable is discovered', () => {
  @Singleton() class Conflict { static $injectable = { lifetime: ServiceLifetime.Transient }; }
  expect(() => describe(Conflict)).toThrow(/registered by @Singleton and by static \$injectable/);
});

test('decorator placement checks include standard constructor hint', () => {
  expect(() => { class Bad { @Inject('x') method() {} } return Bad; }).toThrow(/static \$inject/);
});

test('subclass inherits injections but not its parent registration', async () => {
  @Singleton()
  class Parent {
    @Inject('parentValue') prop!: number;
  }
  class Child extends Parent { @Inject('childValue') override prop: number = 0; static $injectable = {}; }
  expect(describe(Child).registration?.source).toBe('static $injectable');
  const c = new Container(); c.registerInstance('childValue', 12); c.registerClass(Child);
  expect((await c.resolve(Child)).prop).toBe(12);
  await c.dispose();
});


test('undecorated subclass does not inherit standard class registration', () => {
  @Singleton()
  class Root {}
  class Derived extends Root {}
  expect(describe(Root).registration?.source).toBe('@Singleton');
  expect(describe(Derived).registration).toBe(undefined);
  const c = new Container();
  expect(() => c.registerClass(Derived)).toThrow(/has no registration/);
});

const PrivateMethod = defineDecorator<[]>({
  name: '@PrivateMethod', on: ['method'],
  apply(target) { target.method(); },
});

@Injectable()
class HiddenMethod {
  @PrivateMethod() #calculate() { return 42; }
}

test('method records resolve standard #private methods through context.access', async () => {
  const c = new Container(); c.registerClass(HiddenMethod);
  const instance = await c.resolve(HiddenMethod);
  const record = c.discover('@PrivateMethod')[0]!.methods[0]!;
  expect(record.get(instance)()).toBe(42);
  await c.dispose();
});

test('injectMembers fills the fields of an instance the container did not construct', async () => {
  @Injectable()
  class Owned {
    @Inject('configuration') configuration!: { region: string };
    @InjectOptional('unknown-option') optional?: number;
    constructor(readonly owner: string) {}
  }
  const root = new Container();
  root.registerInstance('configuration', { region: 'eu' });
  const value = root.injectMembers(new Owned('app'));
  expect(value.owner).toBe('app');
  expect(value.configuration.region).toBe('eu');
  expect(value.optional).toBe(undefined);
  await root.dispose();
});

test('injectMembers throws for an unregistered, an asynchronous and a multi member', async () => {
  @Injectable() class Missing { @Inject('missing') missing!: unknown; }
  @Injectable() class Slow { @Inject('slow') slow!: unknown; }
  @Injectable() class Many { @InjectAll('providers') providers!: number[]; }
  const root = new Container();
  root.singleton('slow', async () => 1);
  root.singletonMulti('providers', () => 1);
  expect(() => root.injectMembers(new Missing())).toThrow(/missing/);
  expect(() => root.injectMembers(new Slow())).toThrow(/asynchronous/);
  expect(() => root.injectMembers(new Many())).toThrow(/asynchronous/);
  await root.dispose();
});
