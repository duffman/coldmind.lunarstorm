import { expectTypeOf, test } from 'vitest';
import { createToken, delay, type InjectionToken } from '@coldmind/inject/token';
import { lazy, provider, bindDeferredSlot, type Lazy, type Provider, type LazyToken, type ProviderToken, type DependencyValue, type DependencyValues, type DeferredScope } from '@coldmind/inject/lazy';

class Service { readonly service = true; }
const typed = createToken<Service>('service');
const lazyClass: LazyToken<Service> = lazy(Service);
const lazySymbol: LazyToken<Service> = lazy(typed);
const lazyDelayed: LazyToken<Service> = lazy(delay(() => Service));
const providerClass: ProviderToken<Service> = provider(Service);
const providerSymbol: ProviderToken<Service> = provider(typed);
const lazyString: LazyToken<unknown> = lazy('service');
const providerPlainSymbol: ProviderToken<unknown> = provider(Symbol('service'));
declare const scope: DeferredScope;
const access: Lazy<Service> = bindDeferredSlot(scope, lazySymbol);
const factory: Provider<Service> = bindDeferredSlot(scope, providerSymbol);
const pending: Promise<Service> = access.get();
const sync: Service = access.getSync();
const peek: Service | undefined = access.peek();
const call: Promise<Service> = factory();
type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends (<T>() => T extends B ? 1 : 2) ? true : false;
type Assert<T extends true> = T;
type PlainSymbolValue = Assert<Equal<DependencyValue<symbol>, unknown>>;
type InferredString = Assert<Equal<typeof lazyString, LazyToken<unknown>>>;
type InferredPlainSymbol = Assert<Equal<typeof providerPlainSymbol, ProviderToken<unknown>>>;
type ClassValue = Assert<Equal<DependencyValue<typeof Service>, Service>>;
type SymbolValue = Assert<Equal<DependencyValue<typeof typed>, Service>>;
type StringValue = Assert<Equal<DependencyValue<'service'>, unknown>>;
type TupleValues = Assert<Equal<DependencyValues<readonly [typeof Service, typeof lazySymbol, typeof providerSymbol]>, readonly [Service, Lazy<Service>, Provider<Service>]>>;
// @ts-expect-error A lazy marker is a dependency slot, not an ordinary service token.
const notToken: InjectionToken<Service> = lazyClass;
// @ts-expect-error A provider marker is not a service value.
const notService: Service = providerClass;
// @ts-expect-error Private branding prevents arbitrary structural markers.
const notMarker: LazyToken<Service> = { token: Service };
// @ts-expect-error Lazy factories preserve their token's service type.
const wrongService: LazyToken<number> = lazy(typed);
// @ts-expect-error Unqualified strings do not infer a service type.
const unknownService: Lazy<Service> = bindDeferredSlot(scope, lazyString);
// @ts-expect-error Deferred markers are not ordinary resolve inputs.
scope.resolve(lazyClass);
// @ts-expect-error Markers cannot wrap other markers.
lazy(providerSymbol);
// @ts-expect-error Tuple shape is exact.
const short: DependencyValues<readonly [typeof Service, typeof providerSymbol]> = [new Service()];
// @ts-expect-error Tuple elements preserve service and wrapper types.
const wrongElement: DependencyValues<readonly [typeof Service, typeof providerSymbol]> = [new Service(), async () => 1];
void [lazyDelayed, providerPlainSymbol, pending, sync, peek, call];

test('compiled positive and negative contracts: lazy-types.test-d.ts', () => { expectTypeOf(access).toEqualTypeOf<Lazy<Service>>(); });
