import type { IContainer } from '@coldmind/inject/src/core/cminject.contracts.ts';
import type { InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';
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
declare const deferredKind: unique symbol;
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
export type DependencyValues<Slots extends readonly DependencySlot[]> = {
    [Index in keyof Slots]: DependencyValue<Slots[Index]>;
};
/** Declares deferred access to a token; does not resolve or register anything. */
export declare function lazy<T>(token: InjectionToken<T>): LazyToken<T>;
/** Declares per-call resolution of a token; does not resolve or register anything. */
export declare function provider<T>(token: InjectionToken<T>): ProviderToken<T>;
/** Whether this is a factory-issued lazy or provider declaration. @internal */
export declare function isDeferredToken(value: unknown): value is DeferredToken;
/** Whether this is a factory-issued lazy declaration. @internal */
export declare function isLazyToken(value: unknown): value is LazyToken<unknown>;
/** Whether this is a factory-issued provider declaration. @internal */
export declare function isProviderToken(value: unknown): value is ProviderToken<unknown>;
/** Obtains the ordinary token without resolving a dependency. @internal */
export declare function unwrapDependencySlot<T>(slot: DependencySlot<T>): InjectionToken<T>;
/** Slot metadata for future graph integration; no graph validation happens in this leaf. @internal */
export interface DependencySlotDescriptor<T = unknown> {
    /** Ordinary token whose availability and lifetime still require validation. */
    readonly token: InjectionToken<T>;
    /** Eager traversal classification; deferred edges are not absent dependencies. */
    readonly kind: 'eager' | 'lazy' | 'provider';
}
/** Describes an edge without registering, resolving, or claiming static cycle safety. @internal */
export declare function describeDependencySlot<T>(slot: DependencySlot<T>): DependencySlotDescriptor<T>;
/** Public resolution operations needed by a bound deferred dependency. @internal */
export type DeferredScope = Pick<IContainer, 'resolve' | 'resolveSync' | 'tryGet'>;
/** Binds lazy access to the provided resolving scope without substituting a root or copying private frame state. @internal */
export declare function bindDeferredSlot<T>(scope: DeferredScope, slot: LazyToken<T>): Lazy<T>;
/** Binds a provider slot to the actual resolving scope; each invocation uses ordinary resolution. @internal */
export declare function bindDeferredSlot<T>(scope: DeferredScope, slot: ProviderToken<T>): Provider<T>;
/** Supports callers with a narrowed deferred-slot union. @internal */
export declare function bindDeferredSlot<T>(scope: DeferredScope, slot: DeferredToken<T>): Lazy<T> | Provider<T>;
export {};
