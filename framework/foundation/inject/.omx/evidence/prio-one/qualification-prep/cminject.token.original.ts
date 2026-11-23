/** A class constructor, usable as a token. */
export type Constructor<T = unknown> = new (...args: never[]) => T;
/** A class constructor, abstract or not. Usable as a token; not as an implementation. */
export type AbstractConstructor<T = unknown> = abstract new (...args: never[]) => T;
/** Defers a class token until first use, to break import cycles. */
export class DelayedConstructor<T> {
  /** Wraps the getter that yields the class. */
  constructor(private readonly cb: () => Constructor<T>) {}
  /** The class this token refers to. */
  get(): Constructor<T> { return this.cb(); }
}
/** A typed token from createToken: a symbol that carries its service type. Runtime-identical to a plain symbol. */
export type Token<T> = symbol & { readonly __service?: T };
/** Anything the container accepts as a token: a class, a string, a symbol, a typed token, or a delayed class. */
export type InjectionToken<T = unknown> = AbstractConstructor<T> | string | symbol | Token<T> | DelayedConstructor<T>;
/** A unique token for a service of type T. Prefer this to string tokens: their type is unknown. */
export function createToken<T>(description: string): Token<T> { return Symbol(description); }

/** An interned string key and the live bindings or external callers that retain it. */
interface StringKey {
  /** The original string. */
  name: string;
  /** Its shared registry key. */
  key: symbol;
  /** Number of live binding retention slots. */
  refs: number;
  /** Whether a public API issued the key to a caller. */
  pinned: boolean;
}
/** Live or externally pinned string-token keys. */
const strings = new Map<string, StringKey>();
/** String-key retention metadata, indexed without interpreting symbol descriptions. */
const stringKeys = new Map<symbol, StringKey>();
/** Keys of class tokens. Weak, so a class can be collected. */
const constructors = new WeakMap<Function, symbol>();

/** Finds an existing string key without interning misses; class keys remain weakly interned. @internal */
export function lookupToken(token: InjectionToken): symbol | undefined {
  if (typeof token === 'symbol') return token;
  if (typeof token === 'string') return strings.get(token)?.key;
  if (token instanceof DelayedConstructor) return lookupToken(token.get());
  if (typeof token === 'function') {
    let key = constructors.get(token);
    if (!key) constructors.set(token, key = Symbol(token.name));
    return key;
  }
  throw new Error(`Invalid token type: ${typeof token}`);
}
/** Interns a registration or declared dependency without permanently pinning its key. @internal */
export function internToken(token: InjectionToken): symbol {
  if (token instanceof DelayedConstructor) return internToken(token.get());
  const known = lookupToken(token);
  if (known) return known;
  if (typeof token !== 'string') throw new Error(`Invalid token type: ${typeof token}`);
  const entry = { name: token, key: Symbol(token), refs: 0, pinned: false };
  strings.set(entry.name, entry); stringKeys.set(entry.key, entry);
  return entry.key;
}
/** Retains a string key for one binding slot and reports whether it needs release. @internal */
export function retainTokenKey(key: symbol): boolean {
  const entry = stringKeys.get(key);
  if (!entry) return false;
  entry.refs++;
  return true;
}
/** Releases a binding's string key and collects it when no live or external reference remains. @internal */
export function releaseTokenKey(key: symbol): void {
  const entry = stringKeys.get(key);
  if (!entry) return;
  if (entry.refs > 0) entry.refs--;
  if (!entry.refs && !entry.pinned) { strings.delete(entry.name); stringKeys.delete(key); }
}
/** Preserves an existing key issued by a public API for arbitrary caller retention. @internal */
export function pinTokenKey(key: symbol): symbol {
  const entry = stringKeys.get(key);
  if (entry) entry.pinned = true;
  return key;
}
/** Maps a token to its stable public key; issued string keys are permanently pinned. */
export function normalizeToken(token: InjectionToken): symbol { return pinTokenKey(internToken(token)); }
/** The permanently pinned shared key for a string name. */
export function getGlobalToken(name: string): symbol { return normalizeToken(name); }
/** Wraps a class getter so the class is read on first use. */
export function delay<T>(cb: () => Constructor<T>): DelayedConstructor<T> { return new DelayedConstructor(cb); }
/** Whether a value is a string or symbol token. */
export function isNormalToken(t: unknown): t is string | symbol { return typeof t === 'string' || typeof t === 'symbol'; }
/** Whether a value is a class token or a delayed class token. */
export function isConstructorToken(t: unknown): t is Constructor | DelayedConstructor<unknown> {
  return typeof t === 'function' || t instanceof DelayedConstructor;
}
/** Number of live or externally pinned string keys. */
export function getTokenCacheSize(): number { return strings.size; }
/** Collects unpinned keys with no live binding retention; publicly issued keys remain stable. */
export function clearTokenCaches(): void {
  for (const entry of strings.values()) if (!entry.refs && !entry.pinned) { strings.delete(entry.name); stringKeys.delete(entry.key); }
}
/** Human-readable name of a token for messages. */
export function tokenName(token: InjectionToken): string {
  if (token instanceof DelayedConstructor) return `delayed(${token.get().name})`;
  if (typeof token === 'function') return `class ${token.name}`;
  if (typeof token === 'string') return `"${token}"`;
  if (typeof token === 'symbol') return token.description ?? '';
  return String(token);
}

/** Counter behind every generated id. Ids only need to be unique within the process. A property, not a `let`: hot paths read it without a binding check. */
const sequence = { value: 0 };
/** Reserves the next number of the shared id sequence, for an id that is formatted later. */
export function nextSequence(): number { return ++sequence.value; }
/** A new unique instance id. */
export function nextResourceInstanceId(): string { return 'instance-' + ++sequence.value; }
/** A new unique scope id, prefixed with `kind`. */
export function nextScopeId(kind: string): string { return kind + '-' + ++sequence.value; }
