import { ActionTimeoutError } from '@coldmind/wire/src/core/action.errors.ts';

/** A value or a promise of it. */
export type ActionAsync<T> = T | PromiseLike<T>;
/** Releases something; may be async. */
export type ActionCleanup = () => ActionAsync<void>;

/** @throws {RangeError} When `value` is not a safe integer in 1..`maximum`. */
export function positiveLimit(value: number, name: string, maximum = 2_147_483_647): number {
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum) throw new RangeError(name + ' is out of range');
  return value;
}

/** Rejects immediately on cancellation; observes ignored adapters' later rejection. */
export function abortable<T>(value: PromiseLike<T>, signal: AbortSignal): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    const finish = (work: () => void) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      work();
    };
    const abort = () => finish(() => reject(signal.reason));
    signal.addEventListener('abort', abort, { once: true });
    void Promise.resolve(value).then(result => finish(() => resolve(result)), error => finish(() => reject(error)));
    if (signal.aborted) abort();
  });
}

/** One unit of async work with a deadline, cancelled by any of the given signals. Call `close()` when done. */
export class ActionOperation {
  readonly #controller = new AbortController();
  readonly #releases: Array<() => void> = [];
  readonly #timer: ReturnType<typeof setTimeout>;
  readonly #deadline: number;
  readonly #timeoutMs: number;

  constructor(timeoutMs: number, signals: readonly (AbortSignal | undefined)[]) {
    positiveLimit(timeoutMs, 'timeoutMs');
    this.#timeoutMs = timeoutMs;
    this.#deadline = performance.now() + timeoutMs;
    this.#timer = setTimeout(() => this.#controller.abort(new ActionTimeoutError(timeoutMs)), timeoutMs);
    for (const signal of new Set(signals)) {
      if (!signal) continue;
      const abort = () => this.#controller.abort(signal.reason);
      if (signal.aborted) { abort(); break; }
      signal.addEventListener('abort', abort, { once: true });
      this.#releases.push(() => signal.removeEventListener('abort', abort));
    }
  }

  /** Aborts on the deadline or when any of the given signals aborts. */
  get signal(): AbortSignal { return this.#controller.signal; }

  /** @throws The abort reason, `ActionTimeoutError` once the deadline passed. */
  check(): void {
    if (performance.now() >= this.#deadline) this.#controller.abort(new ActionTimeoutError(this.#timeoutMs));
    this.signal.throwIfAborted();
  }

  /** Awaits `value`, rejecting at once when the operation is aborted or times out. */
  async wait<T>(value: ActionAsync<T>): Promise<T> {
    // The producer may already have synchronously aborted before returning.
    const pending = Promise.resolve(value);
    void pending.catch(() => {});
    this.check();
    const result = await abortable(pending, this.signal);
    this.check();
    return result;
  }

  /** Clears the timer and detaches from the given signals. */
  close(): void { clearTimeout(this.#timer); for (const release of this.#releases) release(); this.#releases.length = 0; }
}
