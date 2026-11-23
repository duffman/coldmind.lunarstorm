import { expect, test } from 'vitest';
import { Container, Injectable, Inject, InjectAll, InjectOptional, Singleton, describe,
  INJECT_METADATA_KEY, ServiceLifetime } from '@coldmind/inject';

@Singleton({ token: 'legacy' })
class Legacy {
  @Inject('property') property!: string;
  @InjectOptional('missing') optional?: string;
  @InjectAll('items') all!: number[];
  constructor(@Inject('service') readonly service: { valid: boolean }) {}
  onInit(): void {
    expect(this.service.valid).toBe(true);
    expect(this.property).toBe('ready');
    expect(this.all).toStrictEqual([1, 2]);
    expect(this.optional).toBe(undefined);
  }
}

test('legacy decorators and reflected constructor metadata', async () => {
  const c = new Container();
  c.registerInstance('property', 'ready'); c.registerInstance('service', { valid: true });
  c.singletonMulti('items', () => 1); c.singletonMulti('items', () => 2);
  c.registerClass(Legacy);
  const value = await c.resolve<Legacy>('legacy');
  expect(value instanceof Legacy).toBeTruthy();
  expect(describe(Legacy).params[0]!.token).toBe('service');
  expect((Reflect as any).getMetadata(INJECT_METADATA_KEY, Legacy)[0]).toBe('service');
  await c.dispose();
});

test('legacy method parameter decorator rejects method injection', () => {
  expect(() => {
    class Invalid { method(@Inject('x') _item: number) {} }
    return Invalid;
  }).toThrow(/constructor parameters only/);
});

test('two class decorators cannot both register a service', () => {
  expect(() => {
    @Injectable() @Singleton()
    class Both {}
    return Both;
  }).toThrow(/takes one registration/);
});

test('legacy parameter metadata inherited by subclasses', async () => {
  @Injectable()
  class Base { constructor(@Inject('x') readonly arg: number) {} }
  @Injectable({ lifetime: ServiceLifetime.Transient })
  class Child extends Base {}
  const c = new Container(); c.registerInstance('x', 7); c.registerClass(Child);
  expect((await c.resolve(Child)).arg).toBe(7);
  await c.dispose();
});

test('legacy injectMembers fills the fields of an instance the container did not construct', async () => {
  class Owned {
    @Inject('property') property!: string;
    @InjectOptional('missing') optional?: string;
    constructor(readonly owner: string) {}
  }
  const c = new Container(); c.registerInstance('property', 'ready');
  const value = c.injectMembers(new Owned('app'));
  expect([value.owner, value.property, value.optional]).toStrictEqual(['app', 'ready', undefined]);
  await c.dispose();
});

test('legacy: a parameter typed as a primitive is reported as unresolvable, naming the class and index', () => {
  @Injectable()
  class Loose { constructor(readonly name: string) {} }
  const c = new Container();
  expect(() => c.instantiate(Loose)).toThrow(/index 0 for Loose/);
});

test('reflected constructor types resolve through reflect-metadata', async () => {
  expect(typeof (Reflect as { getMetadata?: unknown }).getMetadata).toBe('function');
  class Engine {}
  @Singleton()
  class Car { constructor(readonly engine: Engine) {} }
  const c = new Container();
  c.singleton(Engine);
  c.registerClass(Car);
  expect((await c.resolve(Car)).engine).toBe(await c.resolve(Engine));
  await c.dispose();
});
