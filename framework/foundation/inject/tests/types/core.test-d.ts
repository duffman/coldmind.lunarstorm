import { expectTypeOf, test } from 'vitest';
import {
  Container, createToken, delay, defineDecorator, Inject, Injectable, InjectAll, InjectOptional,
  Request, Scoped, Singleton, Tenant, Transient,
  type AbstractConstructor, type BindingHandle, type Constructor, type IContainer,
  type InjectionToken, type ServiceFactory, type ServiceLifecycle, type Token, type UniversalDecorator,
} from '@coldmind/inject';

class Service {
  constructor(readonly name: string, readonly count: number) {}
}
abstract class AbstractService {
  abstract readonly name: string;
}
const SERVICE = createToken<Service>('service');
declare const container: Container;
declare const interfaceContainer: IContainer;
declare const accessorContext: ClassAccessorDecoratorContext<Service, Service | undefined>;
declare const accessorTarget: ClassAccessorDecoratorTarget<Service, Service | undefined>;
declare const numericAccessorContext: ClassAccessorDecoratorContext<Service, number>;
declare const numericAccessorTarget: ClassAccessorDecoratorTarget<Service, number>;

test('required constructor arguments remain compatible with class tokens', () => {
  expectTypeOf<ConstructorParameters<Constructor<Service>>>().toEqualTypeOf<never[]>();
  expectTypeOf<ConstructorParameters<AbstractConstructor<AbstractService>>>().toEqualTypeOf<never[]>();
  expectTypeOf<typeof Service>().toExtend<Constructor<Service>>();
  expectTypeOf<typeof AbstractService>().toExtend<AbstractConstructor<AbstractService>>();
  expectTypeOf<typeof AbstractService>().toExtend<InjectionToken<AbstractService>>();
  expectTypeOf(container.singleton(Service)).toEqualTypeOf<BindingHandle<Service>>();
  expectTypeOf(container.registerClass(Service)).toEqualTypeOf<BindingHandle<Service>>();
  expectTypeOf(container.resolve(Service)).toEqualTypeOf<Promise<Service>>();
  expectTypeOf(container.resolve(delay(() => Service))).toEqualTypeOf<Promise<Service>>();
  expectTypeOf(container.resolve(AbstractService)).toEqualTypeOf<Promise<AbstractService>>();
  container.singleton(AbstractService, () => ({ name: 'implemented' }));
  // @ts-expect-error Abstract tokens require a concrete implementation factory.
  container.singleton(AbstractService);
  // @ts-expect-error Abstract classes cannot be registered as concrete constructors.
  container.registerClass(AbstractService);
  // @ts-expect-error An abstract token is not a concrete implementation constructor.
  const implementation: Constructor<AbstractService> = AbstractService;
});

test('typed tokens and multi-resolution retain the service type', () => {
  expectTypeOf(SERVICE).toEqualTypeOf<Token<Service>>();
  expectTypeOf(container.resolve(SERVICE)).toEqualTypeOf<Promise<Service>>();
  expectTypeOf(container.resolveSync(SERVICE)).toEqualTypeOf<Service>();
  expectTypeOf(container.resolveAll(SERVICE)).toEqualTypeOf<Promise<Service[]>>();
  expectTypeOf(container.resolveAll(Service)).toEqualTypeOf<Promise<Service[]>>();
  expectTypeOf(interfaceContainer.resolve(SERVICE)).toEqualTypeOf<Promise<Service>>();
  expectTypeOf(interfaceContainer.resolveAll(SERVICE)).toEqualTypeOf<Promise<Service[]>>();
  expectTypeOf(container.singletonMulti(SERVICE, () => new Service('one', 1)))
    .toEqualTypeOf<BindingHandle<Service>>();
  // @ts-expect-error A typed token cannot resolve as an unrelated value.
  const wrong: Promise<number> = container.resolve(SERVICE);
});

test('factory data, container context, and binding replacement stay typed', () => {
  type Data = { name: string; count: number };
  const factory: ServiceFactory<Service, Data> = (scope, data) => {
    expectTypeOf(scope).toEqualTypeOf<IContainer>();
    expectTypeOf(data).toEqualTypeOf<Data>();
    return new Service(data.name, data.count);
  };
  const handle = container.transient<Service, Data>(SERVICE, factory);
  expectTypeOf(handle).toEqualTypeOf<BindingHandle<Service, Data>>();
  expectTypeOf(container.resolve<Service, Data>(SERVICE, { name: 'typed', count: 1 }))
    .toEqualTypeOf<Promise<Service>>();
  expectTypeOf(container.resolveAll<Service, Data>(SERVICE, { name: 'typed', count: 1 }))
    .toEqualTypeOf<Promise<Service[]>>();
  handle.replace((scope, data) => {
    expectTypeOf(scope).toEqualTypeOf<IContainer>();
    expectTypeOf(data).toEqualTypeOf<Data>();
    return new Service(data.name, data.count);
  });
  // @ts-expect-error Factory data must satisfy the explicit data contract.
  container.resolve<Service, Data>(SERVICE, { name: 'missing count' });
  // @ts-expect-error Factory data cannot change its declared field types.
  container.transient<Service, Data>(SERVICE, (_scope, data: { name: number }) => new Service(String(data.name), 1));
  // @ts-expect-error A factory must produce the declared service type.
  const wrongFactory: ServiceFactory<Service, Data> = () => 123;
});

test('standard decorators accept class, field, accessor, and setter placement', () => {
  @Injectable()
  class Standard {
    @Inject(SERVICE) field!: Service;
    optional: Service | undefined;
    @InjectAll(SERVICE) set services(value: Service[]) {}
  }
  // Direct context checks avoid Vitest 5's AST walker limitation for auto-accessor syntax.
  InjectOptional(SERVICE)(accessorTarget, accessorContext);
  @Singleton() class Single {}
  @Transient() class TransientService {}
  @Scoped() class ScopedService {}
  @Tenant() class TenantService {}
  @Request() class RequestService {}
  expectTypeOf<Standard['field']>().toEqualTypeOf<Service>();
  class Invalid {
    // @ts-expect-error Registration decorators are class-only.
    @Injectable() field!: Service;
    // @ts-expect-error Injection decorators cannot decorate methods.
    @Inject(SERVICE) method() {}
    // @ts-expect-error Injection decorators cannot decorate getters.
    @Inject(SERVICE) get service(): Service { return new Service('invalid', 0); }
  }
  // @ts-expect-error Injection decorators cannot decorate classes.
  @Inject(SERVICE)
  class InvalidClass {}
});

test('legacy decorators accept class, property, setter, and constructor parameter calls', () => {
  Injectable()(Service);
  Singleton()(Service);
  Inject(SERVICE)(Service.prototype, 'field');
  InjectOptional(SERVICE)(Service.prototype, 'optional');
  InjectAll(SERVICE)(Service.prototype, 'services', { set(value: Service[]) {} });
  Inject(SERVICE)(Service, undefined, 0);
  // @ts-expect-error Legacy registration decorators cannot decorate a property.
  Injectable()(Service.prototype, 'field');
  // @ts-expect-error Legacy injection decorators cannot decorate a class.
  Inject(SERVICE)(Service);
  // @ts-expect-error Legacy injection only supports constructor parameters, not method parameters.
  Inject(SERVICE)(Service.prototype, 'method', 0);
});

test('universal decorators expose each dialect placement without an arbitrary-call escape hatch', () => {
  const Decorator = defineDecorator({
    name: '@Universal',
    on: ['class', 'field', 'accessor', 'getter', 'setter', 'method', 'parameter'],
    apply() {},
  });
  const universal: UniversalDecorator = Decorator();
  universal(Service);
  universal(Service.prototype, 'field');
  universal(Service.prototype, 'method', { value() {} });
  universal(Service, undefined, 0);
  universal(numericAccessorTarget, numericAccessorContext);
  @Decorator()
  class Standard {
    @Decorator() field = 1;
    value = 1;
    @Decorator() get read(): number { return 1; }
    @Decorator() set write(value: number) {}
    @Decorator() method(value: string): number { return value.length; }
  }
  // @ts-expect-error Neither dialect permits an arbitrary non-decorator context.
  universal(123, { kind: 'not-a-placement' });
  // @ts-expect-error Legacy member keys must be strings or symbols.
  universal(Service.prototype, 123);
});


test('factory data retains unions and void calls without conditional-type escape hatches', () => {
  const ordinary: ServiceFactory<Service> = scope => new Service(scope.getScopeId(), 1);
  expectTypeOf(ordinary(interfaceContainer)).toEqualTypeOf<Service | Promise<Service>>();
  type Data = { name: string; count: number } | undefined;
  const union: ServiceFactory<Service, Data> = (_scope, data) => new Service(data?.name ?? 'default', data?.count ?? 0);
  union(interfaceContainer, { name: 'typed', count: 1 }); union(interfaceContainer, undefined);
  // @ts-expect-error Union data still rejects unrelated payloads.
  union(interfaceContainer, 123);
  // @ts-expect-error Nonvoid required factory data cannot be omitted.
  ((scope: IContainer, data: { count: number }) => data.count)(interfaceContainer);
});

test('abstract class declarations and each universal TC39 method signature remain valid', () => {
  @Injectable()
  abstract class AbstractDeclaration { constructor(readonly dependency: Service) {} abstract read(): string; }
  Inject(SERVICE)(AbstractDeclaration, undefined, 0);
  expectTypeOf<typeof AbstractDeclaration>().toExtend<AbstractConstructor<AbstractDeclaration>>();
  const Universal = defineDecorator({ name: '@TypedMethod', on: ['method'], apply() {} });
  class Declaration {
    @Universal() method(this: Declaration, value: string, count: number): string { return value.repeat(count); }
  }
  // @ts-expect-error Lifetime decorators remain class-only for TC39 accessors.
  Singleton()(numericAccessorTarget, numericAccessorContext);
  // @ts-expect-error Injection decorators reject legacy method descriptors.
  Inject(SERVICE)(Service.prototype, 'method', { value() {} });
});


test('explicit undefined and void no-data factories preserve one-argument arity', () => {
  const explicit: ServiceFactory<number, undefined> = () => 1;
  const noData: ServiceFactory<number, void | undefined> = () => 2;
  const ordinary: ServiceFactory<number> = () => 3;
  explicit(interfaceContainer); noData(interfaceContainer); ordinary(interfaceContainer);
  expectTypeOf<Parameters<typeof explicit>>().toEqualTypeOf<[IContainer]>();
  expectTypeOf<Parameters<typeof noData>>().toEqualTypeOf<[IContainer]>();
  expectTypeOf<Parameters<typeof ordinary>>().toEqualTypeOf<[IContainer]>();
  // @ts-expect-error No-data factories do not acquire an extra argument slot.
  ordinary(interfaceContainer, undefined);
});


test('caller-supplied any factory data preserves the previous required payload call shape', () => {
  const direct: ServiceFactory<number, any> = (_scope: IContainer, data: any) => data.value;
  const ignoring: ServiceFactory<number, any> = () => 1;
  const lifecycle: ServiceLifecycle<number, any> = { factory: (_scope: IContainer, data: any) => data.value };
  direct(interfaceContainer, { value: 1 }); ignoring(interfaceContainer, { value: 2 }); lifecycle.factory(interfaceContainer, { value: 3 });
  expectTypeOf<Parameters<typeof direct>>().toEqualTypeOf<[IContainer, any]>();
  // @ts-expect-error Previous conditional-any factories required the payload at call sites.
  direct(interfaceContainer);
  // @ts-expect-error Ignoring a parameter does not alter an annotated factory's payload contract.
  ignoring(interfaceContainer);
  // @ts-expect-error Lifecycle factories have the same required caller-any payload slot.
  lifecycle.factory(interfaceContainer);
});
