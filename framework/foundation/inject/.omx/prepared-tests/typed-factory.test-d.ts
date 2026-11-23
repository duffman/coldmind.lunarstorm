import { createToken, delay } from '../../src/core/cminject.token.ts';
import type { BindingHandle, IContainer, ServiceFactory, ServiceLifecycle } from '../../src/core/cminject.contracts.ts';
import type { TypedFactoryContainer } from './typed-factory.facade.ts';
import { lazy, provider, type Lazy, type Provider } from '../../src/core/cminject.lazy.ts';
type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends (<T>() => T extends B ? 1 : 2) ? true : false;
type Assert<T extends true> = T;
declare const c: TypedFactoryContainer;
const N = createToken<number>('n'); const S = createToken<string>('s');
const h: BindingHandle<string> = c.singleton(S, { inject: [N] as const, factory: n => n.toFixed() });
// @ts-expect-error too few
c.singleton(S, { inject: [N] as const, factory: () => '' });
// @ts-expect-error wrong explicit parameter
c.singleton(S, { inject: [N] as const, factory: (n: string) => n });
const h2: BindingHandle<string> = c.transient(S, { inject: [N,S] as const, factory: (n,s) => s + n.toFixed(), onInit(value, scope) { value.toUpperCase(); scope.getScopeId(); }, onDispose(value) { value.toUpperCase(); } });
// @ts-expect-error one of two is too few
c.singleton(S, { inject: [N,S] as const, factory: n => n.toFixed() });
// @ts-expect-error too many
c.singleton(S, { inject: [N] as const, factory: (n, s: string) => n.toFixed() + s });

class Concrete { constructor(readonly required: string) {} }
abstract class Abstract { abstract readonly abstractValue: boolean; }
const mixed = [N, S, Concrete, Abstract, lazy(N), provider(S), delay(() => Concrete)] as const;
const hm = c.scoped(S, { inject: mixed, factory: (n, s, concrete, abstract, lazyNumber, stringProvider, delayed) => {
  type Values = Assert<Equal<[typeof n, typeof s, typeof concrete, typeof abstract, typeof lazyNumber, typeof stringProvider, typeof delayed], [number, string, Concrete, Abstract, Lazy<number>, Provider<string>, Concrete]>>;
  const deferred: Promise<string> = stringProvider();
  const deferredNumber: Promise<number> = lazyNumber.get();
  return s + n.toFixed() + concrete.required + abstract.abstractValue + delayed.required;
} });
type MixedHandle = Assert<Equal<typeof hm, BindingHandle<string>>>;
const he = c.tenant(S, { inject: [] as const, factory: () => 'empty' });
const ha = c.request(S, { inject: [N] as const, factory: async n => n.toFixed() });
type AsyncHandle = Assert<Equal<typeof ha, BindingHandle<string>>>;
const h3 = c.singletonMulti(S, { inject: [N, S] as const, factory: (n, s) => n.toFixed() + s.toUpperCase() });
type Handle3 = Assert<Equal<typeof h3, BindingHandle<string>>>;
const h4 = c.transientMulti(S, { inject: [N, S] as const, factory: async (n, s) => n.toFixed() + s.toUpperCase() });
type Handle4 = Assert<Equal<typeof h4, BindingHandle<string>>>;
const h5 = c.scopedMulti(S, { inject: [N, S] as const, factory: (n, s) => n.toFixed() + s.toUpperCase() });
type Handle5 = Assert<Equal<typeof h5, BindingHandle<string>>>;
const h6 = c.tenantMulti(S, { inject: [N, S] as const, factory: async (n, s) => n.toFixed() + s.toUpperCase() });
type Handle6 = Assert<Equal<typeof h6, BindingHandle<string>>>;
const h7 = c.requestMulti(S, { inject: [N, S] as const, factory: (n, s) => n.toFixed() + s.toUpperCase() });
type Handle7 = Assert<Equal<typeof h7, BindingHandle<string>>>;
// @ts-expect-error singleton: exact tuple arity rejects omitted dependency argument.
c.singleton(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error transient: exact tuple arity rejects omitted dependency argument.
c.transient(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error scoped: exact tuple arity rejects omitted dependency argument.
c.scoped(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error tenant: exact tuple arity rejects omitted dependency argument.
c.tenant(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error request: exact tuple arity rejects omitted dependency argument.
c.request(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error singletonMulti: exact tuple arity rejects omitted dependency argument.
c.singletonMulti(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error transientMulti: exact tuple arity rejects omitted dependency argument.
c.transientMulti(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error scopedMulti: exact tuple arity rejects omitted dependency argument.
c.scopedMulti(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error tenantMulti: exact tuple arity rejects omitted dependency argument.
c.tenantMulti(S, { inject: [N, S] as const, factory: n => n.toFixed() });
// @ts-expect-error requestMulti: exact tuple arity rejects omitted dependency argument.
c.requestMulti(S, { inject: [N, S] as const, factory: n => n.toFixed() });

// @ts-expect-error Empty tuple cannot supply an argument.
c.singleton(S, { inject: [] as const, factory: (n: number) => n.toFixed() });
// @ts-expect-error Service return must match typed registration token.
c.singleton(S, { inject: [N] as const, factory: n => n + 1 });
// @ts-expect-error Async service return must match typed token.
c.singleton(S, { inject: [N] as const, factory: async n => n + 1 });
// @ts-expect-error Lifecycle onDispose receives the service, not dependency.
c.singleton(S, { inject: [N] as const, factory: n => n.toFixed(), onDispose(value: number) {} });
// @ts-expect-error Optional argument gives a nonexact 0|1 length.
c.singleton(S, { inject: [N] as const, factory: (n?: number) => String(n) });
// @ts-expect-error Rest array has no exact finite length.
c.singleton(S, { inject: [N] as const, factory: (...n: number[]) => String(n) });
const widened: readonly (typeof N)[] = [N];
// @ts-expect-error inject must be an exact readonly tuple, not a widened array.
c.singleton(S, { inject: widened, factory: (...n: number[]) => String(n) });
const literalOptions = c.singleton(S, { inject: [N] as const, factory: n => n.toFixed() }, { timeoutMs: 10 });
const immutable: readonly [typeof N] = [N];
c.singleton(S, { inject: immutable, factory: n => n.toFixed() });
const old = c.singleton(S, scope => scope.getScopeId());
const oldLifecycle = c.transient(S, { factory: scope => scope.getScopeId(), onInit(value, scope) { value.toUpperCase(); scope.getScopeId(); } });
const oldData = c.transient<string, { name: string } | undefined>(S, (scope, data) => data?.name ?? scope.getScopeId());
type OldDataHandle = Assert<Equal<typeof oldData, BindingHandle<string, { name: string } | undefined>>>;
c.singleton(Concrete); c.transientMulti(Concrete, { deps: [S] });
// @ts-expect-error Abstract token still needs an implementation.
c.singleton(Abstract);
const noData: ServiceFactory<string> = scope => scope.getScopeId();
const explicitUndefined: ServiceFactory<string, undefined> = scope => scope.getScopeId();
const voidUndefined: ServiceFactory<string, void | undefined> = scope => scope.getScopeId();
const requiredUnion: ServiceFactory<string, { name: string } | undefined> = (_scope, data) => data?.name ?? '';
type NoDataArgs = Assert<Equal<Parameters<typeof noData>, [IContainer]>>;
type UndefinedArgs = Assert<Equal<Parameters<typeof explicitUndefined>, [IContainer]>>;
type VoidUndefinedArgs = Assert<Equal<Parameters<typeof voidUndefined>, [IContainer]>>;
type UnionArgs = Assert<Equal<Parameters<typeof requiredUnion>, [IContainer, { name: string } | undefined]>>;
noData(c); explicitUndefined(c); voidUndefined(c); requiredUnion(c, undefined);
// @ts-expect-error No-data form has no extra undefined argument slot.
noData(c, undefined);
// @ts-expect-error Required data union remains required.
requiredUnion(c);
// Derive the existing explicit caller-any branch without introducing any syntax.
type CallerAny = ReturnType<typeof JSON.parse>;
const explicitCallerAny: ServiceFactory<string, CallerAny> = (_scope, data) => String(data);
const callerAnyLifecycle: ServiceLifecycle<string, CallerAny> = { factory: (_scope, data) => String(data) };
type AnyArity = Assert<Equal<Parameters<typeof explicitCallerAny>['length'], 2>>;
explicitCallerAny(c, undefined); callerAnyLifecycle.factory(c, 1);
// @ts-expect-error Explicit caller-any payload remains required (G04).
explicitCallerAny(c);
// @ts-expect-error Lifecycle explicit caller-any payload remains required (G04).
callerAnyLifecycle.factory(c);
const oldAnyHandle = c.singleton<string, CallerAny>(S, explicitCallerAny);
type AnyHandle = Assert<Equal<typeof oldAnyHandle, BindingHandle<string, CallerAny>>>;
const callerAnyIgnoring: ServiceFactory<string, CallerAny> = () => 'ignoring';
// @ts-expect-error Ignoring payload does not remove the required caller-any call slot.
callerAnyIgnoring(c);
const undefinedHandle = c.transient<string, undefined>(S, explicitUndefined);
type UndefinedHandle = Assert<Equal<typeof undefinedHandle, BindingHandle<string, undefined>>>;
const oldTypedLifecycle: ServiceLifecycle<string, { name: string } | undefined> = { factory: (_scope, data) => data?.name ?? '', onDispose(value) { value.toUpperCase(); } };
c.request<string, { name: string } | undefined>(S, oldTypedLifecycle);
// @ts-expect-error Explicit G04 data shape cannot change through the new overload.
c.transient<string, { name: string }>(S, (_scope, data: { name: number }) => String(data.name));
const unqualified = c.singleton(S, { inject: ['untyped', Symbol('plain')] as const, factory: (a, b) => {
  type UntypedValues = Assert<Equal<[typeof a, typeof b], [unknown, unknown]>>;
  return String(a) + String(b);
} });

// Every method retains each legacy overload family.
c.singleton(Concrete);
c.singleton(S, scope => scope.getScopeId());
c.singleton<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.transient(Concrete);
c.transient(S, scope => scope.getScopeId());
c.transient<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.scoped(Concrete);
c.scoped(S, scope => scope.getScopeId());
c.scoped<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.tenant(Concrete);
c.tenant(S, scope => scope.getScopeId());
c.tenant<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.request(Concrete);
c.request(S, scope => scope.getScopeId());
c.request<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.singletonMulti(Concrete);
c.singletonMulti(S, scope => scope.getScopeId());
c.singletonMulti<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.transientMulti(Concrete);
c.transientMulti(S, scope => scope.getScopeId());
c.transientMulti<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.scopedMulti(Concrete);
c.scopedMulti(S, scope => scope.getScopeId());
c.scopedMulti<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.tenantMulti(Concrete);
c.tenantMulti(S, scope => scope.getScopeId());
c.tenantMulti<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());
c.requestMulti(Concrete);
c.requestMulti(S, scope => scope.getScopeId());
c.requestMulti<string, { name: string }>(S, (scope, data) => data.name + scope.getScopeId());

c.singleton(S, { inject: [N] as const, factory: n => n.toFixed(), onHealthCheck(value) { return { healthy: value.length > 0 }; } });
// @ts-expect-error Lifecycle onInit container remains IContainer.
c.singleton(S, { inject: [N] as const, factory: n => n.toFixed(), onInit(value, scope: number) {} });
