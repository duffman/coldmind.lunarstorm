import { LifecycleError } from '@coldmind/inject/errors';
import { tokenName, type InjectionToken } from '@coldmind/inject/token';

/** High-resolution timestamp in milliseconds, used for record and observation times. */
export const now = () => performance.now();
/** Shared reading of the clock for the current synchronous run. A property, not a `let`: hot paths read it without a binding check. */
const clock: { tick: number | undefined } = { tick: undefined };
/** Clears the shared clock reading at the end of the synchronous run. */
const resetClock = () => { clock.tick = undefined; };
/**
 * Timestamp for diagnostics (registration, build, edge and snapshot times): one clock read per synchronous run, so a
 * burst of registrations or builds pays for one. Durations use `now`.
 */
export function stamp(): number {
  const tick = clock.tick;
  if (tick !== undefined) return tick;
  queueMicrotask(resetClock);
  return clock.tick = performance.now();
}
/** Only the instance slot is needed to expose an already selected resolution result. */
interface InstanceValue { readonly instance: unknown; }

/** Applies `next` to a value now, or once it settles when it is a promise. Keeps synchronous work synchronous. */
export function chain<A, B>(value: A | Promise<A>, next: (value: A) => B | Promise<B>): B | Promise<B> {
  return value instanceof Promise ? value.then(next) : next(value);
}
/** Calls `run` for each item in order. Synchronous until a call returns a promise; the rest then wait for it. */
export function each<T>(items: readonly T[], run: (item: T) => unknown, from = 0): void | Promise<void> {
  for (let i = from; i < items.length; i++) {
    const result = run(items[i]!);
    if (result instanceof Promise) return result.then(() => each(items, run, i + 1));
  }
}
/** Runs a hook whose failure must not replace `error`: a hook error becomes the cause of `error` instead. */
export function guarded(error: unknown, hook: () => unknown): unknown {
  try {
    const result = hook();
    return result instanceof Promise ? result.catch(hookError => attachCause(error, hookError)) : result;
  } catch (hookError) { attachCause(error, hookError); return undefined; }
}
/**
 * Hides a thenable instance's `then` while a promise settles with it, so the promise does not adopt it.
 * The member is restored in a microtask, before the awaiting code resumes, which keeps the object identical.
 */
export function shield<T>(value: T): T {
  if (value !== null && (typeof value === 'object' || typeof value === 'function') && typeof Reflect.get(value, 'then') === 'function') {
    const object = value;
    const original = Object.getOwnPropertyDescriptor(object, 'then');
    if ((!original || original.configurable) && Object.isExtensible(object)) {
      Object.defineProperty(object, 'then', { value: undefined, configurable: true });
      queueMicrotask(() => {
        if (original) Object.defineProperty(object, 'then', original);
        else Reflect.deleteProperty(object, 'then');
      });
    } else {
      throw new LifecycleError('A service with a callable `then` that cannot be hidden (frozen, sealed or non-configurable) cannot be resolved through a promise; use resolveSync() or get()');
    }
  }
  return value;
}
/** The selected typed token establishes T before its erased stored instance is exposed. */
function valueOf<T>(record: InstanceValue): T { return shield(record.instance) as T; }
/** Runs a resolution and returns its value as a promise. A synchronous throw becomes a rejection. */
export function promised<T>(run: () => InstanceValue | Promise<InstanceValue>): Promise<T> {
  let out: InstanceValue | Promise<InstanceValue>;
  try { out = run(); return out instanceof Promise ? out.then(record => valueOf<T>(record)) : Promise.resolve(valueOf<T>(out)); }
  catch (error) { return Promise.reject(error); }
}
/** Returns a resolution's value synchronously, or throws `LifecycleError` when the resolution went asynchronous. */
export function synchronous<T>(out: InstanceValue | Promise<InstanceValue>, token: InjectionToken): T {
  if (out instanceof Promise) {
    out.catch(() => undefined); // The build carries on and is cached; its failure surfaces on the next resolve.
    throw new LifecycleError(`Service is asynchronous, use resolve(): ${tokenName(token)}`);
  }
  // Resolution selected the binding associated with the caller's InjectionToken<T>.
  return out.instance as T;
}
/** Runs a build or injection and returns its value as a promise. A synchronous throw becomes a rejection. */
export function settle<T>(run: () => T | Promise<T>): Promise<T> {
  let out: T | Promise<T>;
  try { out = run(); return out instanceof Promise ? out.then(value => shield(value)) : Promise.resolve(shield(out)); }
  catch (error) { return Promise.reject(error); }
}
/** The value of a build that must be synchronous, or a `LifecycleError` naming `what` when a dependency made it asynchronous. */
export function syncValue<T>(out: T | Promise<T>, what: string, alternative: string): T {
  if (out instanceof Promise) {
    out.catch(() => undefined); // The build carries on; its failure surfaces to whoever awaits it.
    throw new LifecycleError(`${what} is asynchronous because a dependency is; use ${alternative}()`);
  }
  return out;
}
/** The values of slots started one after another: the values when every one is synchronous, otherwise a promise of them. */
export function gather(count: number, start: (index: number) => unknown): unknown[] | Promise<unknown[]> {
  const items: unknown[] = [];
  try { for (let i = 0; i < count; i++) items.push(start(i)); }
  catch (error) {
    for (const item of items) if (item instanceof Promise) item.catch(() => undefined); // Already started: do not leave rejections unhandled.
    throw error;
  }
  return items.some(item => item instanceof Promise) ? Promise.all(items) : items;
}
/** Wraps a non-`Error` thrown value in an `Error`. */
export function errorOf(error: unknown): Error { return error instanceof Error ? error : new Error(String(error)); }
/** Sets `secondary` as the `cause` of `original` when it has none, so a cleanup failure is not lost. */
export function attachCause(original: unknown, secondary: unknown): void {
  if (original instanceof Error && original.cause === undefined) {
    Object.defineProperty(original, 'cause', { configurable: true, value: secondary });
  }
}


/** Keeps fallback controllers alive only while their exposed signal is live. */
const SIGNAL_OWNER = Symbol('cminject.signalOwner');
/** Removes fallback source listeners after a combined signal becomes unreachable. */
let signalFinalizers: FinalizationRegistry<() => void> | undefined;
/** Combines abort sources without accumulating strong per-build listeners on a long-lived owner. */
export function buildSignal(local: AbortSignal, owner: AbortSignal): AbortSignal {
  if (typeof AbortSignal.any === 'function') return AbortSignal.any([local, owner]);
  const controller = new AbortController();
  if (local.aborted || owner.aborted) { controller.abort(local.aborted ? local.reason : owner.reason); return controller.signal; }
  if (typeof WeakRef !== 'function' || typeof FinalizationRegistry !== 'function') throw new LifecycleError('Build signals require AbortSignal.any or weak-reference support');
  const reference = new WeakRef(controller);
  const token = {};
  const finalizers = signalFinalizers ??= new FinalizationRegistry(cleanup => { cleanup(); });
  const abortLocal = (): void => { reference.deref()?.abort(local.reason); };
  const abortOwner = (): void => { reference.deref()?.abort(owner.reason); };
  const cleanup = (): void => {
    local.removeEventListener('abort', abortLocal); owner.removeEventListener('abort', abortOwner); finalizers.unregister(token);
    const current = reference.deref(); if (current) Reflect.deleteProperty(current.signal, SIGNAL_OWNER);
  };
  Object.defineProperty(controller.signal, SIGNAL_OWNER, { configurable: true, value: controller });
  local.addEventListener('abort', abortLocal, { once: true }); owner.addEventListener('abort', abortOwner, { once: true });
  controller.signal.addEventListener('abort', cleanup, { once: true }); finalizers.register(controller.signal, cleanup, token);
  return controller.signal;
}

