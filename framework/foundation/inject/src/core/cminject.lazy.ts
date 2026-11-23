import type { IContainer } from '@coldmind/inject/contracts';
import type { InjectionToken } from '@coldmind/inject/token';

/** Deferred access that preserves the token's normal lifetime; peek never constructs a service. */
export interface Lazy<T> {
  /** Resolves using the originating scope and the token's normal lifetime. */
  get(): Promise<T>;
  /** Resolves synchronously, throwing when the ordinary resolution needs asynchronous work. */
  getSync(): T;
  /** Reads an already available instance without constructing one. */
  peek(): T | undefined;
}
/** Resolves on each call; transient services therefore produce a new instance each time. */
export type Provider<T> = () => Promise<T>;
/** Private marker identity; markers describe dependency slots, not registered service tokens. */
const deferredKind: unique symbol = Symbol('inject.deferred');
/** Immutable lazy dependency declaration; only lazy() creates recognized declarations. */
export interface LazyToken<T> {
  /** Wrapped ordinary service token. */
  readonly token: InjectionToken<T>;
  /** Nominal lazy-slot discriminator. */
  readonly [deferredKind]: 'lazy';
}
/** Immutable provider dependency declaration; only provider() creates recognized declarations. */
export interface ProviderToken<T> {
  /** Wrapped ordinary service token. */
  readonly token: InjectionToken<T>;
  /** Nominal provider-slot discriminator. */
  readonly [deferredKind]: 'provider';
}
/** A deferred dependency declaration, never an ordinary InjectionToken. */
export type DeferredToken<T = unknown> = LazyToken<T> | ProviderToken<T>;
/** A dependency slot for future constructor/member/factory and graph integration. */
export type DependencySlot<T = unknown> = InjectionToken<T> | DeferredToken<T>;
/** The value supplied for one dependency slot. */
export type DependencyValue<Slot> = Slot extends LazyToken<infer T> ? Lazy<T> : Slot extends ProviderToken<infer T> ? Provider<T> : Slot extends InjectionToken<infer T> ? T : never;
/** Values of a dependency tuple, preserving its order and readonly modifier. */
export type DependencyValues<Slots extends readonly DependencySlot[]> = { [Index in keyof Slots]: DependencyValue<Slots[Index]> };
/** Factory-issued objects only; copied symbol properties cannot forge a recognized marker. */
const declarations = new WeakSet<object>();
/** Declares deferred access to a token; does not resolve or register anything. */
export function lazy<T>(token: InjectionToken<T>): LazyToken<T> {
  const marker: LazyToken<T> = Object.freeze({ token, [deferredKind]: 'lazy' as const });
  declarations.add(marker);
  return marker;
}
/** Declares per-call resolution of a token; does not resolve or register anything. */
export function provider<T>(token: InjectionToken<T>): ProviderToken<T> {
  const marker: ProviderToken<T> = Object.freeze({ token, [deferredKind]: 'provider' as const });
  declarations.add(marker);
  return marker;
}
/** Whether this is a factory-issued lazy or provider declaration. @internal */
export function isDeferredToken(value: unknown): value is DeferredToken {
  return typeof value === 'object' && value !== null && declarations.has(value);
}
/** Whether this is a factory-issued lazy declaration. @internal */
export function isLazyToken(value: unknown): value is LazyToken<unknown> {
  return isDeferredToken(value) && value[deferredKind] === 'lazy';
}
/** Whether this is a factory-issued provider declaration. @internal */
export function isProviderToken(value: unknown): value is ProviderToken<unknown> {
  return isDeferredToken(value) && value[deferredKind] === 'provider';
}
/** Obtains the ordinary token without resolving a dependency. @internal */
export function unwrapDependencySlot<T>(slot: DependencySlot<T>): InjectionToken<T> {
  return isDeferredToken(slot) ? slot.token : slot;
}
/** Slot metadata for future graph integration; no graph validation happens in this leaf. @internal */
export interface DependencySlotDescriptor<T = unknown> {
  /** Ordinary token whose availability and lifetime still require validation. */
  readonly token: InjectionToken<T>;
  /** Eager traversal classification; deferred edges are not absent dependencies. */
  readonly kind: 'eager' | 'lazy' | 'provider';
}
/** Describes an edge without registering, resolving, or claiming static cycle safety. @internal */
export function describeDependencySlot<T>(slot: DependencySlot<T>): DependencySlotDescriptor<T> {
  return { token: unwrapDependencySlot(slot), kind: isDeferredToken(slot) ? slot[deferredKind] : 'eager' };
}
/** Public resolution operations needed by a bound deferred dependency. @internal */
export type DeferredScope = Pick<IContainer, 'resolve' | 'resolveSync' | 'tryGet'>;
/** Binds lazy access to the provided resolving scope without substituting a root or copying private frame state. @internal */
export function bindDeferredSlot<T>(scope: DeferredScope, slot: LazyToken<T>): Lazy<T>;
/** Binds a provider slot to the actual resolving scope; each invocation uses ordinary resolution. @internal */
export function bindDeferredSlot<T>(scope: DeferredScope, slot: ProviderToken<T>): Provider<T>;
/** Supports callers with a narrowed deferred-slot union. @internal */
export function bindDeferredSlot<T>(scope: DeferredScope, slot: DeferredToken<T>): Lazy<T> | Provider<T>;
export function bindDeferredSlot<T>(scope: DeferredScope, slot: DeferredToken<T>): Lazy<T> | Provider<T> {
  if (!isDeferredToken(slot)) throw new TypeError('Expected a lazy or provider dependency declaration');
  const get = async (): Promise<T> => scope.resolve(slot.token);
  return isProviderToken(slot) ? get : Object.freeze({ get, getSync: (): T => scope.resolveSync(slot.token), peek: (): T | undefined => scope.tryGet(slot.token) });
}
