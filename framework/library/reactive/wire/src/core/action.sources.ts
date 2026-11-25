import type { ActionCore } from '@coldmind/wire/src/core/action.core.ts';
import type { Scope } from '@coldmind/wire/src/core/action.scope.ts';
import type { CallbackSource } from '@coldmind/wire/src/core/action.types.ts';

interface PromiseReceiver<T> {
  target: ActionCore<T> | undefined;
  release: (() => void) | undefined;
}

// Keep promise reactions in an independent lexical scope. They must not capture
// the source's Scope (which retains its error reporter and therefore its core).
function observePromise<T>(promise: PromiseLike<T>, receiver: PromiseReceiver<T>, retain: boolean): void {
  void Promise.resolve(promise).then(
    value => {
      const target = receiver.target;
      receiver.release?.();
      if (!target || target.disposed) return;
      if (retain) target.retainValue(value);
      else target.publish(value);
    },
    error => {
      const target = receiver.target;
      receiver.release?.();
      if (!target || target.disposed) return;
      if (retain) target.retainError(error);
      else target.error(error);
    },
  );
}

export function callbackSource<T>(core: ActionCore<T>, scope: Scope, register: CallbackSource<T>): void {
  const controller = new AbortController();
  scope.add(() => controller.abort());
  const cleanup = register(
    value => { if (!scope.closed) core.publish(value); },
    error => { if (!scope.closed && !core.disposed) core.error(error); },
    controller.signal,
  );
  if (cleanup !== undefined) {
    if (typeof cleanup !== 'function') throw new TypeError('Callback source must return a cleanup function or undefined');
    scope.add(cleanup);
  }
}

export function eventSource<T, E extends Event>(
  core: ActionCore<T>, scope: Scope, target: EventTarget, type: string,
  map: (event: E) => T, options: AddEventListenerOptions = {},
): void {
  if (options.signal?.aborted) { scope.close(); return; }
  const listener: EventListener = event => {
    if (scope.closed || core.disposed) return;
    // Remove before mapping, because the mapper may dispatch this event again.
    if (options.once) scope.close();
    try { core.publish(map(event as E)); }
    catch (error) { if (!core.disposed) core.error(error); }
  };
  const capture = options.capture ?? false;
  scope.add(() => target.removeEventListener(type, listener, capture));
  const signal = options.signal;
  if (signal) {
    const abort = () => scope.close();
    signal.addEventListener('abort', abort, { once: true });
    scope.add(() => signal.removeEventListener('abort', abort));
  }
  target.addEventListener(type, listener, { capture, passive: options.passive ?? false });
}

export function promiseSource<T>(
  core: ActionCore<T>, scope: Scope, promise: PromiseLike<T>, retain = false,
): void {
  const receiver: PromiseReceiver<T> = { target: core, release: () => scope.close() };
  scope.add(() => { receiver.target = undefined; receiver.release = undefined; });
  observePromise(promise, receiver, retain);
}
