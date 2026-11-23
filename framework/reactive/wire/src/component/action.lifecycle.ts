import { ActionDisposedError, ActionTimeoutError } from '@coldmind/wire/src/core/action.errors.ts';
import { abortable, positiveLimit } from '@coldmind/wire/src/core/action.operation.ts';
import type { ActionAsync, ActionCleanup } from '@coldmind/wire/src/core/action.operation.ts';
import { actionErrorAttributes } from '@coldmind/wire/src/telemetry/action.telemetry.ts';
import type { ActionTelemetry, ActionTraceContext } from '@coldmind/wire/src/telemetry/action.telemetry.ts';

/** created → starting → started → destroying → destroyed. */
export type ActionLifecycleState = 'created' | 'starting' | 'started' | 'destroying' | 'destroyed';
/** Telemetry, the cleanup deadline and lifecycle callbacks; callback failures are reported, never thrown. */
export interface ActionLifecycleOptions {
  readonly telemetry?: ActionTelemetry;
  /** Time all cleanups together get at `destroy()`. Default 5000 ms. */
  readonly cleanupTimeoutMs?: number;
  readonly onCreate?: (name: string) => unknown;
  readonly onStart?: (name: string) => unknown;
  readonly onError?: (error: unknown, name: string) => unknown;
  readonly onDestroy?: (name: string) => unknown;
}

/** Shared lifetime ownership; abort happens before reverse-order cleanup. */
export abstract class ActionLifecycle {
  readonly name: string;
  readonly telemetry: ActionTelemetry | undefined;
  readonly #options: ActionLifecycleOptions;
  readonly #controller = new AbortController();
  readonly #cleanups: ActionCleanup[] = [];
  readonly #cleanupTimeoutMs: number;
  #state: ActionLifecycleState = 'created';
  #start: Promise<this> | undefined;
  #destroy: Promise<void> | undefined;

  protected constructor(name: string, options: ActionLifecycleOptions = {}) {
    if (!name.trim() || name.length > 200) throw new TypeError('Invalid lifecycle name');
    this.name = name;
    this.telemetry = options.telemetry;
    this.#cleanupTimeoutMs = positiveLimit(options.cleanupTimeoutMs ?? 5000, 'cleanupTimeoutMs');
    this.#options = Object.freeze({ ...options });
    this.#notify('onCreate');
  }

  get state(): ActionLifecycleState { return this.#state; }
  /** Aborted when `destroy()` begins, before any cleanup runs. */
  get signal(): AbortSignal { return this.#controller.signal; }

  #notify(event: 'onCreate' | 'onStart' | 'onDestroy'): void {
    this.telemetry?.log('debug', event, { component: this.name });
    try { void Promise.resolve(this.#options[event]?.(this.name)).catch(error => this.reportError(error)); }
    catch (error) { this.reportError(error); }
  }

  /** Logs to telemetry and passes the error to `onError`; never throws. */
  protected reportError(error: unknown, context?: ActionTraceContext): void {
    this.telemetry?.log('error', 'ActionWire error', { component: this.name, ...actionErrorAttributes(error) }, context);
    try {
      void Promise.resolve(this.#options.onError?.(error, this.name)).catch(() => {});
    } catch {}
  }

  /** @throws {ActionDisposedError} Once `destroy()` has begun. */
  protected assertAlive(): void { if (this.signal.aborted) throw new ActionDisposedError(); }

  /** Registers a cleanup for `destroy()`; the returned function runs it early, once. After destroy it runs at once. */
  protected own(cleanup: ActionCleanup): ActionCleanup {
    if (typeof cleanup !== 'function') throw new TypeError('Cleanup must be a function');
    let active = true;
    const release = async () => {
      if (!active) return;
      active = false;
      const index = this.#cleanups.indexOf(release);
      if (index !== -1) this.#cleanups.splice(index, 1);
      await cleanup();
    };
    if (this.signal.aborted) void release().catch(error => this.reportError(error));
    else this.#cleanups.push(release);
    return release;
  }

  /** The instance's work; a returned cleanup is owned. `signal` aborts when the instance is destroyed. */
  protected abstract setup(signal: AbortSignal): ActionAsync<void | ActionCleanup>;

  /** Runs `setup` once; later calls return the same promise. A failing setup destroys the instance and rejects. */
  start(): Promise<this> {
    if (this.signal.aborted) return Promise.reject(new ActionDisposedError());
    if (this.#start) return this.#start;
    this.#state = 'starting';
    this.#start = Promise.resolve().then(async () => {
      this.assertAlive();
      const acquisition = Promise.resolve(this.setup(this.signal)).then(cleanup => {
        if (cleanup !== undefined) this.own(cleanup);
      });
      await abortable(acquisition, this.signal);
      this.assertAlive();
      this.#state = 'started';
      this.#notify('onStart');
      this.assertAlive();
      return this;
    }).catch(async error => {
      if (!this.signal.aborted) this.reportError(error);
      try { await this.destroy(); } catch (cleanupError) { this.reportError(cleanupError); }
      throw error;
    });
    return this.#start;
  }

  /**
   * Aborts `signal`, then runs the cleanups in reverse order within `cleanupTimeoutMs`. Idempotent.
   * @throws {AggregateError} (rejects) When a cleanup failed or ran out of time; the others still ran.
   */
  destroy(): Promise<void> {
    if (this.#destroy) return this.#destroy;
    this.#state = 'destroying';
    // Assign the promise before invoking user abort listeners.
    this.#destroy = Promise.resolve().then(async () => {
      const failures: unknown[] = [];
      const deadline = performance.now() + this.#cleanupTimeoutMs;
      for (const cleanup of [...this.#cleanups].reverse()) {
        const controller = new AbortController();
        const remaining = Math.max(0, deadline - performance.now());
        const timer = setTimeout(() => controller.abort(new ActionTimeoutError(this.#cleanupTimeoutMs)), remaining);
        try {
          await abortable(Promise.resolve(cleanup()), controller.signal);
          if (performance.now() >= deadline) throw new ActionTimeoutError(this.#cleanupTimeoutMs);
        } catch (error) { failures.push(error); this.reportError(error); }
        finally { clearTimeout(timer); }
      }
      this.#state = 'destroyed';
      this.#notify('onDestroy');
      if (failures.length) throw new AggregateError(failures, 'ActionWire cleanup failed');
    });
    this.#controller.abort(new ActionDisposedError());
    return this.#destroy;
  }
}
