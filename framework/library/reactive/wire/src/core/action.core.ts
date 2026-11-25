import { ActionCycleError, ActionDisposedError, defaultErrorReporter, observeReturn } from '@coldmind/wire/src/core/action.errors.ts';
import { Scope } from '@coldmind/wire/src/core/action.scope.ts';
import type { ActionSpan, ActionTraceContext } from '@coldmind/wire/src/telemetry/action.telemetry.ts';
import { actionErrorAttributes } from '@coldmind/wire/src/telemetry/action.telemetry.ts';
import type { ActionOptions, ErrorHandler, SubscriptionOptions, Unsubscribe } from '@coldmind/wire/src/core/action.types.ts';

export interface Observer<T> {
  readonly data?: (value: T) => unknown;
  /** True means this branch handled the error. */
  readonly error?: (error: unknown) => boolean;
  readonly end?: () => void;
  readonly once?: boolean;
}

interface Registration<T> {
  readonly observer: Observer<T>;
  active: boolean;
  release: Unsubscribe;
  abortCleanup?: Unsubscribe;
}

interface DependencyNode { readonly parents: Map<DependencyNode, number> }
type Settlement<T> = { readonly kind: 'data'; readonly value: T } | { readonly kind: 'error'; readonly error: unknown };
type Activation = (scope: Scope) => void;

interface ReturnedResult<T> {
  target: ActionCore<T> | undefined;
  release: Unsubscribe | undefined;
  readonly diagnostic: boolean;
}

function observeHandlerResult<T>(result: unknown, receiver: ReturnedResult<T>): void {
  void Promise.resolve(result).then(
    () => receiver.release?.(),
    error => {
      const target = receiver.target;
      receiver.release?.();
      if (!target) defaultErrorReporter(error);
      else if (receiver.diagnostic) target.diagnostic(error);
      else target.error(error);
    },
  );
}

export class ActionCore<T> {
  readonly options: ActionOptions;
  readonly #sink: ErrorHandler;
  readonly #observers = new Set<Registration<T>>();
  readonly #owned: Scope;
  readonly #graph: DependencyNode = { parents: new Map() };
  readonly #queue: Array<{ value: T; observers: Registration<T>[]; span: ActionSpan | undefined }> = [];
  #context: ActionTraceContext | undefined;
  #currentSpan: ActionSpan | undefined;
  #start: Activation | undefined;
  #active: Scope | undefined;
  #settlement: Settlement<T> | undefined;
  #disposed = false;
  #disposing = false;
  #emitting = false;
  #reporting = false;

  constructor(options: ActionOptions = {}, start?: Activation) {
    if (options.onUnhandledError !== undefined && typeof options.onUnhandledError !== 'function') {
      throw new TypeError('onUnhandledError must be a function');
    }
    this.options = Object.freeze({ ...options });
    this.#sink = options.onUnhandledError ?? defaultErrorReporter;
    this.#owned = new Scope(error => this.error(error));
    this.#start = start;
  }

  get disposed(): boolean { return this.#disposed; }
  get context(): ActionTraceContext | undefined { return this.#context; }

  span(operation: string, parent?: ActionTraceContext): ActionSpan | undefined {
    return this.options.telemetry?.startSpan((this.options.name ?? 'action') + '.' + operation, {
      ...(parent ? { parent } : {}),
    });
  }

  assertActive(): void {
    if (this.#disposed) throw new ActionDisposedError();
  }

  diagnostic(error: unknown): void {
    try { observeReturn(this.#sink(error), defaultErrorReporter); }
    catch (failure) { defaultErrorReporter(failure); }
  }

  callError(handler: ErrorHandler, error: unknown): void {
    try { this.trackResult(handler(error), true); }
    catch (failure) { this.diagnostic(failure); }
  }

  trackResult(result: unknown, diagnostic = false): void {
    if (result === null || (typeof result !== 'object' && typeof result !== 'function')) return;
    const scope = this.#owned.child();
    const receiver: ReturnedResult<T> = { target: this, release: () => scope.close(), diagnostic };
    scope.add(() => { receiver.target = undefined; receiver.release = undefined; });
    observeHandlerResult(result, receiver);
  }

  /** Internal observation is atomic, including synchronous replay/source setup. */
  observe(observer: Observer<T>, options: SubscriptionOptions = {}): Unsubscribe {
    if (this.#disposed) {
      try { observer.end?.(); } catch (error) { this.diagnostic(error); }
      return () => {};
    }
    if (options.signal?.aborted) return () => {};

    const registration: Registration<T> = {
      observer,
      active: true,
      release: () => {},
    };
    const release = () => {
      if (!registration.active) return;
      registration.active = false;
      registration.abortCleanup?.();
      this.#observers.delete(registration);
      if (this.#observers.size === 0) this.#deactivate();
    };
    registration.release = release;
    this.#observers.add(registration);

    const signal = options.signal;
    if (signal) {
      signal.addEventListener('abort', release, { once: true });
      registration.abortCleanup = () => signal.removeEventListener('abort', release);
    }

    const settlement = this.#settlement;
    if (settlement?.kind === 'data') this.#deliver(registration, settlement.value);
    else if (settlement?.kind === 'error' && observer.error) this.#deliverError(registration, settlement.error);

    if (registration.active && !this.#active && this.#start) {
      const scope = this.#owned.child();
      this.#active = scope;
      try { this.#start(scope); }
      catch (error) { this.error(error); scope.close(); }
    }
    return release;
  }

  #deactivate(): void {
    const active = this.#active;
    this.#active = undefined;
    active?.close();
  }

  #deliver(registration: Registration<T>, value: T): void {
    if (!registration.active || this.#disposed || !registration.observer.data) return;
    if (registration.observer.once) registration.release();
    try {
      this.trackResult(registration.observer.data(value));
    } catch (error) { this.error(error); }
  }

  publish(value: T, parent?: ActionTraceContext): void {
    if (this.#disposed) return;
    this.#queue.push({ value, observers: [...this.#observers], span: this.span('emit', parent ?? this.#context) });
    if (this.#emitting) return;
    this.#emitting = true;
    try {
      for (let index = 0; index < this.#queue.length && !this.#disposed; index++) {
        const item = this.#queue[index]!;
        const previous = this.#context;
        const previousSpan = this.#currentSpan;
        this.#context = item.span?.context;
        this.#currentSpan = item.span;
        try { for (const registration of item.observers) this.#deliver(registration, item.value); }
        finally { this.#context = previous; this.#currentSpan = previousSpan; item.span?.end(); }
      }
    } finally {
      this.#queue.length = 0;
      this.#emitting = false;
    }
  }

  #deliverError(registration: Registration<T>, error: unknown): boolean {
    if (!registration.active || !registration.observer.error) return false;
    if (registration.observer.once) registration.release();
    try { return registration.observer.error(error); }
    catch (failure) { this.diagnostic(failure); return true; }
  }

  /** Propagate without diagnosing, so a branching graph diagnoses only at its origin. */
  forwardError(error: unknown): boolean {
    if (this.#disposed && !this.#disposing) return false;
    if (this.#reporting) return false;
    this.#reporting = true;
    let handled = false;
    try {
      for (const registration of [...this.#observers]) {
        handled = this.#deliverError(registration, error) || handled;
      }
    } finally { this.#reporting = false; }
    return handled;
  }

  error(error: unknown): void {
    if (this.#currentSpan) this.#currentSpan.end('error');
    else this.span('error')?.end('error');
    this.options.telemetry?.log('error', 'Action error', { action: this.options.name ?? 'action', ...actionErrorAttributes(error) }, this.#context);
    if (!this.forwardError(error)) this.diagnostic(error);
  }

  retainValue(value: T): void {
    if (this.#disposed) return;
    this.#settlement = { kind: 'data', value };
    this.publish(value);
  }

  retainError(error: unknown): void {
    if (this.#disposed) return;
    this.#settlement = { kind: 'error', error };
    // Report like any other error when no handler has been attached yet; the settlement stays
    // retained, so a later onError or next() still receives it.
    this.error(error);
  }

  attach(register: Activation): void {
    this.assertActive();
    const scope = this.#owned.child();
    try { register(scope); }
    catch (error) { this.error(error); scope.close(); }
  }

  own(cleanup: Unsubscribe): Unsubscribe { return this.#owned.add(cleanup); }

  dependency<S>(source: ActionCore<S>): Unsubscribe {
    const pending = [source.#graph];
    const visited = new Set<DependencyNode>();
    while (pending.length) {
      const node = pending.pop()!;
      if (node === this.#graph) throw new ActionCycleError();
      if (visited.has(node)) continue;
      visited.add(node);
      pending.push(...node.parents.keys());
    }
    const node = source.#graph;
    this.#graph.parents.set(node, (this.#graph.parents.get(node) ?? 0) + 1);
    let attached = true;
    return () => {
      if (!attached) return;
      attached = false;
      const count = this.#graph.parents.get(node) ?? 0;
      if (count <= 1) this.#graph.parents.delete(node);
      else this.#graph.parents.set(node, count - 1);
    };
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#disposing = true;
    for (const item of this.#queue) item.span?.end('cancelled');
    this.#queue.length = 0;
    this.#start = undefined;
    this.#settlement = undefined;
    this.#deactivate();
    this.#owned.close();
    const registrations = [...this.#observers];
    for (const registration of registrations) registration.release();
    this.#graph.parents.clear();
    for (const { observer } of registrations) {
      try { observer.end?.(); } catch (error) { this.diagnostic(error); }
    }
    this.#disposing = false;
  }
}
