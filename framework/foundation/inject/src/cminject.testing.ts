import { Container } from '@coldmind/inject/container';
import { ContainerView } from '@coldmind/inject/internal/view';
import { MutationConflictError } from '@coldmind/inject/errors';
import { ServiceLifetime, type IContainer, type ServiceFactory, type ServiceLifecycle } from '@coldmind/inject/contracts';
import { lookupToken, type InjectionToken } from '@coldmind/inject/token';
import type { ModuleDefinition } from '@coldmind/inject/module';

/** An awaited override; restore is idempotent and nested overrides restore in LIFO order. */
export interface OverrideHandle { restore(): Promise<void>; }
/** Explicit factory form also supports services whose value itself is callable. */
export interface FactoryOverride<T> { readonly kind: 'factory'; readonly factory: ServiceFactory<T> | ServiceLifecycle<T>; }
/** Explicit value form disambiguates callable service values. */
export interface ValueOverride<T> { readonly kind: 'value'; readonly value: T; }
const stacks = new WeakMap<Container, Map<symbol, OverrideHandle[]>>();
/** Replaces one definition after saving it, never reuses a disposed factory-produced instance. */
export async function override<T>(container: IContainer, token: InjectionToken<T>, replacement: T | ServiceFactory<T> | FactoryOverride<T> | ValueOverride<T>): Promise<OverrideHandle> {
  const target = container instanceof ContainerView ? container.target : container;
  if (!(target instanceof Container)) throw new TypeError('Overrides require a cminject container');
  const old = target.bindingOf(token);
  if (old?.kindTag === 'registerInstance' && old.owner.ownership === 'owned') throw new MutationConflictError('Cannot override an owned supplied instance: its disposed value cannot be reconstructed');
  const snapshot = target.snapshotBinding(token);
  let factory: ServiceFactory<T> | ServiceLifecycle<T>;
  if (replacement && typeof replacement === 'object' && 'kind' in replacement && replacement.kind === 'value' && 'value' in replacement) factory = () => replacement.value as T;
  else if (replacement && typeof replacement === 'object' && 'kind' in replacement && replacement.kind === 'factory' && 'factory' in replacement) factory = replacement.factory as ServiceFactory<T> | ServiceLifecycle<T>;
  else if (typeof replacement === 'function') factory = replacement as ServiceFactory<T>;
  else factory = () => replacement as T;
  await target.replace(token, factory, old?.lifetime ?? ServiceLifetime.Singleton, { retainPrevious: false });
  const key = lookupToken(token)!;
  let byToken = stacks.get(target); if (!byToken) stacks.set(target, byToken = new Map());
  let stack = byToken.get(key); if (!stack) byToken.set(key, stack = []);
  let done = false; let restoring: Promise<void> | undefined;
  const handle: OverrideHandle = { restore() {
    if (done) return Promise.resolve();
    if (restoring) return restoring;
    if (stack!.at(-1) !== handle) return Promise.reject(new MutationConflictError('Overrides must restore in LIFO order'));
    restoring = target.restoreBinding(snapshot).then(() => { done = true; stack!.pop(); if (!stack!.length) byToken!.delete(key); }, error => { restoring = undefined; throw error; });
    return restoring;
  } };
  stack.push(handle); return handle;
}
/** Isolated root with bounded builds; module registration is awaited before the root is exposed. */
export async function createTestContainer(...modules: ModuleDefinition[]): Promise<Container> {
  const root = new Container(); root.buildTimeout = 2000;
  try { await root.load(...modules); return root; }
  catch (error) { await root.dispose(); throw error; }
}
