import { AsyncLocalStorage } from 'node:async_hooks';
import type { IContainer } from '@coldmind/inject/contracts';
import { LifecycleError } from '@coldmind/inject/errors';

/** Server-only ambient scope. Core never reads it: resolution passes its context explicitly. */
const ambient = new AsyncLocalStorage<IContainer>();

/** Runs `fn` with `container` as the ambient scope for the current async context. Throws when the container is disposed. */
export function runInContext<T>(container: IContainer, fn: () => T): T {
  if (container.isDisposed) throw new LifecycleError('Cannot perform operation on a disposed container');
  return ambient.run(container, fn);
}

/** The ambient scope set by `runInContext`, or `undefined` outside one. */
export function currentContext(): IContainer | undefined { return ambient.getStore(); }
