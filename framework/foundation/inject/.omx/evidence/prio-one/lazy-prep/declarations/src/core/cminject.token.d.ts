/** A class constructor, usable as a token. */
export type Constructor<T = unknown> = new (...args: any[]) => T;
/** A class constructor, abstract or not. Usable as a token; not as an implementation. */
export type AbstractConstructor<T = unknown> = abstract new (...args: any[]) => T;
/** Defers a class token until first use, to break import cycles. */
export declare class DelayedConstructor<T> {
    private readonly cb;
    /** Wraps the getter that yields the class. */
    constructor(cb: () => Constructor<T>);
    /** The class this token refers to. */
    get(): Constructor<T>;
}
/** A typed token from createToken: a symbol that carries its service type. Runtime-identical to a plain symbol. */
export type Token<T> = symbol & {
    readonly __service?: T;
};
/** Anything the container accepts as a token: a class, a string, a symbol, a typed token, or a delayed class. */
export type InjectionToken<T = unknown> = AbstractConstructor<T> | string | symbol | Token<T> | DelayedConstructor<T>;
/** A unique token for a service of type T. Prefer this to string tokens: their type is unknown. */
export declare function createToken<T>(description: string): Token<T>;
/** Finds an existing string key without interning misses; class keys remain weakly interned. @internal */
export declare function lookupToken(token: InjectionToken): symbol | undefined;
/** Interns a registration or declared dependency without permanently pinning its key. @internal */
export declare function internToken(token: InjectionToken): symbol;
/** Retains a string key for one binding slot and reports whether it needs release. @internal */
export declare function retainTokenKey(key: symbol): boolean;
/** Releases a binding's string key and collects it when no live or external reference remains. @internal */
export declare function releaseTokenKey(key: symbol): void;
/** Preserves an existing key issued by a public API for arbitrary caller retention. @internal */
export declare function pinTokenKey(key: symbol): symbol;
/** Maps a token to its stable public key; issued string keys are permanently pinned. */
export declare function normalizeToken(token: InjectionToken): symbol;
/** The permanently pinned shared key for a string name. */
export declare function getGlobalToken(name: string): symbol;
/** Wraps a class getter so the class is read on first use. */
export declare function delay<T>(cb: () => Constructor<T>): DelayedConstructor<T>;
/** Whether a value is a string or symbol token. */
export declare function isNormalToken(t: unknown): t is string | symbol;
/** Whether a value is a class token or a delayed class token. */
export declare function isConstructorToken(t: unknown): t is Constructor | DelayedConstructor<unknown>;
/** Number of live or externally pinned string keys. */
export declare function getTokenCacheSize(): number;
/** Collects unpinned keys with no live binding retention; publicly issued keys remain stable. */
export declare function clearTokenCaches(): void;
/** Human-readable name of a token for messages. */
export declare function tokenName(token: InjectionToken): string;
/** Reserves the next number of the shared id sequence, for an id that is formatted later. */
export declare function nextSequence(): number;
/** A new unique instance id. */
export declare function nextResourceInstanceId(): string;
/** A new unique scope id, prefixed with `kind`. */
export declare function nextScopeId(kind: string): string;
