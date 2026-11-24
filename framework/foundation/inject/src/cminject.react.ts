import { createContext, createElement, Suspense, useContext, useEffect, useState, type ReactNode } from 'react';
import type { InjectionToken } from '@coldmind/inject/token';
import type { IContainer } from '@coldmind/inject/contracts';
import { LifecycleError } from '@coldmind/inject/errors';

/** The container given to `ContainerProvider`. */
const ContainerContext = createContext<IContainer | undefined>(undefined);
/** The innermost scope created by a `ScopeProvider`. */
const ScopeContext = createContext<IContainer | undefined>(undefined);

/** Props for `ContainerProvider`. */
export interface ContainerProviderProps {
  /** The container to provide. The provider does not dispose it. */
  container: IContainer;
  /** The subtree that can read the container. */
  children?: ReactNode;
}
/** Provides a root (or any) container to the tree. Does not own its lifetime. */
export function ContainerProvider({ container, children }: ContainerProviderProps): ReactNode {
  return createElement(ContainerContext.Provider, { value: container }, children);
}

/** Props for `ScopeProvider`. */
export interface ScopeProviderProps {
  /** Kind of child scope: a generic scope, or a request scope. Defaults to `scope`. */
  kind?: 'scope' | 'request';
  /** Shown while a child suspends on a service of this scope. Defaults to `null`. */
  fallback?: ReactNode;
  /** The subtree that resolves from the new scope. */
  children?: ReactNode;
}

/** A scope shared by the renders and effects of one `ScopeProvider` instance. */
interface Slot {
  /** The parent and kind this slot was created for. */
  readonly parent: IContainer;
  readonly kind: 'scope' | 'request';
  /** The child scope this provider owns. */
  scope?: IContainer;
  /** Number of mounted effects holding the scope. Disposal waits until it reaches zero. */
  refs: number;
}

/**
 * Creates a child scope for this subtree. Reference-counted: StrictMode mounts, unmounts and remounts
 * the effect, and disposal is deferred to a microtask that rechecks the count, so the scope survives.
 *
 * Children render inside this provider's own Suspense boundary so a suspended child does not discard
 * the provider's state. Slots belong to component state, never to a global React-id cache.
 */
export function ScopeProvider({ kind = 'scope', fallback = null, children }: ScopeProviderProps): ReactNode {
  const enclosingScope = useContext(ScopeContext);
  const container = useContext(ContainerContext);
  const parent = requireContainer(enclosingScope ?? container);
  // Discarded StrictMode renders allocate only an empty slot; ScopeContents creates the retained slot's scope.
  let [slot, setSlot] = useState<Slot>(() => ({ parent, kind, refs: 0 }));
  if (slot.parent !== parent || slot.kind !== kind) {
    slot = { parent, kind, refs: 0 };
    setSlot(slot);
  }
  /** Holds the scope while mounted. On unmount, disposal is deferred to a microtask that rechecks the count. */
  useEffect(() => {
    slot.refs++;
    return () => {
      slot.refs--;
      queueMicrotask(() => {
        const scope = slot.scope;
        if (slot.refs > 0 || !scope || scope.isDisposed) return;
        scope.dispose().catch(error => console.error('cminject: scope disposal failed', error));
      });
    };
  }, [slot]);
  return createElement(ScopeContents, { slot, fallback, children });
}

/** Only the retained provider render reaches this child, including under React 18 StrictMode. */
function ScopeContents({ slot, fallback, children }: { slot: Slot; fallback: ReactNode; children?: ReactNode }): ReactNode {
  const scope = slot.scope ??= slot.kind === 'request' ? slot.parent.createRequestScope() : slot.parent.createScope();
  return createElement(ScopeContext.Provider, { value: scope }, createElement(Suspense, { fallback }, children));
}

/** Returns the container, or throws `LifecycleError` when no `ContainerProvider` is above. */
function requireContainer(container: IContainer | undefined): IContainer {
  if (!container) throw new LifecycleError('No cminject container: wrap the tree in <ContainerProvider>.');
  return container;
}

/** The innermost scope, or the provided container when no ScopeProvider is above. */
export function useScope(): IContainer {
  const scope = useContext(ScopeContext);
  const container = useContext(ContainerContext);
  return requireContainer(scope ?? container);
}

/** A suspended read retains its rejection until explicitly reset. */
interface ServiceRead {
  /** Settles when the build succeeds or its rejection is recorded. */
  promise: Promise<void>;
  /** Distinguishes a pending read from any rejection value. */
  status: 'pending' | 'rejected';
  /** The original rejection, including undefined. */
  error?: unknown;
}
/** Shares suspended reads and retained failures within each scope. */
const inFlight = new WeakMap<IContainer, Map<InjectionToken, ServiceRead>>();

/** Clears a retained failure without cancelling a pending build or evicting a ready instance. */
export function resetService(scope: IContainer, token: InjectionToken): void {
  const reads = inFlight.get(scope);
  if (reads?.get(token)?.status === 'rejected') reads.delete(token);
}

/**
 * Synchronous read of a service. A service that is not built yet is built during render when its whole build is
 * synchronous. When any step is async, the render suspends: it throws a promise (React 18 and 19 Suspense) that
 * settles when the build does.
 */
export function useService<T>(token: InjectionToken<T>): T {
  const scope = useScope();
  const read = inFlight.get(scope)?.get(token);
  if (read?.status === 'rejected') throw read.error;
  if (read) throw read.promise;
  try {
    return scope.get(token);
  } catch (error) {
    if (!(error instanceof LifecycleError)) throw error;
    let byToken = inFlight.get(scope);
    if (!byToken) { byToken = new Map(); inFlight.set(scope, byToken); }
    // Not built yet: build it now when every step is synchronous, otherwise suspend on the async build.
    try { return scope.resolveSync(token); }
    catch (syncError) {
      if (!(syncError instanceof LifecycleError)) {
        byToken.set(token, { promise: Promise.resolve(), status: 'rejected', error: syncError });
        throw syncError;
      }
    }
    const pending: ServiceRead = { promise: Promise.resolve(), status: 'pending' };
    pending.promise = scope.resolve(token).then(() => {
      if (byToken.get(token) === pending) byToken.delete(token);
    }, error => {
      pending.error = error;
      pending.status = 'rejected';
    });
    byToken.set(token, pending);
    throw pending.promise;
  }
}

/** A request scope under `parent`, for one server render. The caller disposes it. */
export function createRequestScope(parent: IContainer, requestId?: string): IContainer {
  return parent.createRequestScope(requestId);
}
