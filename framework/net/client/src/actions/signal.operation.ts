import { ActionTimeoutError } from '@coldmind/wire';

/** @throws {RangeError} When `value` is not an integer in 1..2147483647. */
export function milliseconds(value: number): number {
  if (!Number.isSafeInteger(value) || value < 1 || value > 2_147_483_647) throw new RangeError('Invalid timeout');
  return value;
}

/** Observe late rejection even when cancellation has already won the race. */
export function withSignal<T>(value: T | PromiseLike<T>, signal: AbortSignal): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    const finish = (work: () => void) => {
      if (settled) return;
      settled = true; signal.removeEventListener('abort', abort); work();
    };
    const abort = () => finish(() => reject(signal.reason));
    signal.addEventListener('abort', abort, { once: true });
    void Promise.resolve(value).then(value => finish(() => resolve(value)), error => finish(() => reject(error)));
    if (signal.aborted) abort();
  });
}

/** One handler run with a deadline, cancelled by any of the given signals. Call `close()` when done. */
export class SignalOperation {
  readonly #controller = new AbortController();
  readonly #releases: Array<() => void> = [];
  readonly #timer: ReturnType<typeof setTimeout>;
  readonly #deadline: number;
  readonly #timeoutMs: number;
  constructor(timeoutMs: number, signals: readonly AbortSignal[]) {
    this.#timeoutMs = milliseconds(timeoutMs);
    this.#deadline = performance.now() + timeoutMs;
    this.#timer = setTimeout(() => this.#controller.abort(new ActionTimeoutError(timeoutMs)), timeoutMs);
    for (const signal of new Set(signals)) {
      const abort = () => this.#controller.abort(signal.reason);
      if (signal.aborted) { abort(); break; }
      signal.addEventListener('abort', abort, { once: true });
      this.#releases.push(() => signal.removeEventListener('abort', abort));
    }
  }
  get signal(): AbortSignal { return this.#controller.signal; }
  check(): void {
    if (performance.now() >= this.#deadline) this.#controller.abort(new ActionTimeoutError(this.#timeoutMs));
    this.signal.throwIfAborted();
  }
  async wait<T>(value: T | PromiseLike<T>): Promise<T> {
    const pending = Promise.resolve(value); void pending.catch(() => {});
    this.check(); const result = await withSignal(pending, this.signal); this.check(); return result;
  }
  close(): void { clearTimeout(this.#timer); for (const release of this.#releases) release(); this.#releases.length = 0; }
}
