import { ActionCore } from '@coldmind/wire/src/core/action.core.ts';
import { ActionDisposedError, ActionTimeoutError, validateMilliseconds } from '@coldmind/wire/src/core/action.errors.ts';
import { Scope } from '@coldmind/wire/src/core/action.scope.ts';
import { callbackSource, eventSource, promiseSource } from '@coldmind/wire/src/core/action.sources.ts';
import { connectAction, debounceAction, distinctAction, filterAction, mapAction, mergeActions, switchMapAction, tapAction } from '@coldmind/wire/src/core/action.operators.ts';
import type { ActionOptions, AsyncMapper, CallbackSource, DataHandler, Equality, ErrorHandler, NextOptions, SubscriptionOptions, Unsubscribe } from '@coldmind/wire/src/core/action.types.ts';

/**
 * A typed, synchronous channel with owned sources and nonterminal errors.
 * Plain actions are hot without replay; derived actions activate lazily and
 * share one upstream subscription until their last observer detaches.
 * Dispose at the owning component's lifetime boundary.
 *
 * @example
 * const input = new Action<string>();
 * const lengths = input.map(value => value.trim().length);
 * const next = lengths.next({ timeout: 1000 });
 * input.emit(' hello ');
 * console.log(await next); // 5
 * input.dispose();
 */
export class Action<T> {
  #core: ActionCore<T>;

  constructor(options: ActionOptions = {}) {
    this.#core = new ActionCore<T>(options);
  }

  static #wrap<R>(core: ActionCore<R>): Action<R> {
    const action = new Action<R>();
    action.#core = core;
    return action;
  }

  /** Same as `new Action<T>(options)`. */
  static create<T>(options?: ActionOptions): Action<T> { return new Action<T>(options); }

  /** One action carrying the values of all sources; it ends when every source has ended. */
  static merge<Values extends readonly unknown[]>(...sources: { [K in keyof Values]: Action<Values[K]> }): Action<Values[number]> {
    return Action.#wrap(mergeActions(sources.map(source => source.#core)));
  }

  /** Attach immediately and retain this promise's settlement for late observers until disposal. */
  static fromPromise<T>(promise: PromiseLike<T>, options?: ActionOptions): Action<T> {
    const action = new Action<T>(options);
    action.#core.attach(scope => promiseSource(action.#core, scope, promise, true));
    return action;
  }

  /** Register on first observation; release on last unsubscribe and register afresh on reactivation. */
  static fromCallback<T>(register: CallbackSource<T>, options?: ActionOptions): Action<T> {
    const core: ActionCore<T> = new ActionCore(options, scope => callbackSource(core, scope, register));
    return Action.#wrap(core);
  }

  /** DOM events as an action; the listener is added on first observation and removed on the last unsubscribe. */
  static fromEvent<K extends keyof WindowEventMap>(target: Window, type: K, options?: AddEventListenerOptions): Action<WindowEventMap[K]>;
  static fromEvent<K extends keyof DocumentEventMap>(target: Document, type: K, options?: AddEventListenerOptions): Action<DocumentEventMap[K]>;
  static fromEvent<K extends keyof HTMLElementEventMap>(target: HTMLElement, type: K, options?: AddEventListenerOptions): Action<HTMLElementEventMap[K]>;
  static fromEvent<E extends Event = Event>(target: EventTarget, type: string, options?: AddEventListenerOptions): Action<E>;
  static fromEvent<E extends Event>(target: EventTarget, type: string, options?: AddEventListenerOptions): Action<E> {
    const core: ActionCore<E> = new ActionCore({}, scope => eventSource(core, scope, target, type, event => event as E, options));
    return Action.#wrap(core);
  }

  /** True after `dispose()`, or after the source of a derived action ended. */
  get isDisposed(): boolean { return this.#core.disposed; }

  /** Synchronous, FIFO multicast. No replay on a plain Action. */
  emit(data: T): this {
    this.#core.assertActive();
    this.#core.publish(data);
    return this;
  }

  /** Observe future values; returned unsubscribe is idempotent. Async handlers do not delay delivery. */
  onData(handler: DataHandler<T>, options?: SubscriptionOptions): Unsubscribe {
    this.#core.assertActive();
    return this.#core.observe({ data: handler }, options);
  }

  /** Observe errors; an observed error does not end the action. Unobserved errors go to `onUnhandledError`. */
  onError(handler: ErrorHandler, options?: SubscriptionOptions): Unsubscribe {
    this.#core.assertActive();
    return this.#core.observe({ error: error => { this.#core.callError(handler, error); return true; } }, options);
  }

  /** Observe the next value only. */
  once(handler: DataHandler<T>, options?: SubscriptionOptions): Unsubscribe {
    this.#core.assertActive();
    return this.#core.observe({ data: handler, once: true }, options);
  }

  /**
   * Atomically observe one value, error, or disposal, then release all listeners.
   * Subscribe before triggering a synchronous producer. Rejects with the source
   * error, signal reason, ActionTimeoutError, or ActionDisposedError.
   */
  next(options: NextOptions = {}): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      this.#core.assertActive();
      if (options.timeout !== undefined) validateMilliseconds(options.timeout);
      const signal = options.signal;
      if (signal?.aborted) { reject(signal.reason); return; }
      const scope = new Scope(error => this.#core.diagnostic(error));
      const fail = (error: unknown) => { if (!scope.closed) { scope.close(); reject(error); } };
      if (signal) {
        const abort = () => fail(signal.reason);
        signal.addEventListener('abort', abort, { once: true });
        scope.add(() => signal.removeEventListener('abort', abort));
      }
      if (options.timeout !== undefined) {
        const timeout = options.timeout;
        const timer = setTimeout(() => fail(new ActionTimeoutError(timeout)), timeout);
        scope.add(() => clearTimeout(timer));
      }
      scope.add(this.#core.observe({
        data: value => { if (!scope.closed) { scope.close(); resolve(value); } },
        error: error => { fail(error); return true; },
        end: () => fail(new ActionDisposedError()),
        once: true,
      }));
    });
  }

  /** Attach a hot one-shot input without replay; disposal ignores its later settlement. */
  fromPromise(promise: PromiseLike<T>): this {
    this.#core.attach(scope => promiseSource(this.#core, scope, promise));
    return this;
  }

  /** Register immediately, with cleanup owned until disposal. Subscribe first if registration can emit. */
  fromCallback(register: CallbackSource<T>): this {
    this.#core.attach(scope => callbackSource(this.#core, scope, register));
    return this;
  }

  /** Feed mapped DOM events into this action until it is disposed or `options.signal` aborts. */
  fromEvent<K extends keyof WindowEventMap>(target: Window, type: K, map: (event: WindowEventMap[K]) => T, options?: AddEventListenerOptions): this;
  fromEvent<K extends keyof DocumentEventMap>(target: Document, type: K, map: (event: DocumentEventMap[K]) => T, options?: AddEventListenerOptions): this;
  fromEvent<K extends keyof HTMLElementEventMap>(target: HTMLElement, type: K, map: (event: HTMLElementEventMap[K]) => T, options?: AddEventListenerOptions): this;
  fromEvent<E extends Event = Event>(target: EventTarget, type: string, map: (event: E) => T, options?: AddEventListenerOptions): this;
  fromEvent<E extends Event>(target: EventTarget, type: string, map: (event: E) => T, options?: AddEventListenerOptions): this {
    this.#core.attach(scope => eventSource(this.#core, scope, target, type, map, options));
    return this;
  }

  /** Own this connection, while leaving the source's lifetime independent. */
  connect(source: Action<T>): this {
    connectAction(this.#core, source.#core);
    return this;
  }

  /** Each value mapped. Derived actions are lazy and share one upstream subscription. */
  map<R>(map: (data: T) => R): Action<R> {
    return Action.#wrap(mapAction(this.#core, map));
  }

  /** Only the values that pass `predicate`. */
  filter<S extends T>(predicate: (data: T) => data is S): Action<S>;
  filter(predicate: (data: T) => boolean): Action<T>;
  filter(predicate: (data: T) => boolean): Action<T> {
    return Action.#wrap(filterAction(this.#core, predicate));
  }

  /** Runs `effect` for each value and passes the value on; a rejected effect reaches `onError`. */
  tap(effect: DataHandler<T>): Action<T> {
    return Action.#wrap(tapAction(this.#core, effect));
  }

  /** The last value, once `ms` passed without a newer one. */
  debounce(ms: number): Action<T> {
    return Action.#wrap(debounceAction(this.#core, ms));
  }

  /** Drops a value equal to the one before it (`Object.is` unless `equals` is given). */
  distinct(equals?: Equality<T>): Action<T> {
    return Action.#wrap(distinctAction(this.#core, equals));
  }

  /**
   * Deliver only the latest async result. New input or last unsubscribe aborts
   * the previous signal; stale results/errors are ignored even without cooperation.
   * Current-operation failures reach onError and leave the action usable.
   */
  switchMap<R>(map: AsyncMapper<T, R>): Action<R> {
    return Action.#wrap(switchMapAction(this.#core, map));
  }

  /** This action's values together with those of `sources`. */
  merge<Values extends readonly unknown[]>(...sources: { [K in keyof Values]: Action<Values[K]> }): Action<T | Values[number]> {
    this.#core.assertActive();
    return Action.merge(this, ...sources);
  }

  /**
   * Idempotently end this action, release inputs/timers, reject pending waits,
   * and end active descendants. Disposing a child never disposes its parent.
   */
  dispose(): void { this.#core.dispose(); }
}
