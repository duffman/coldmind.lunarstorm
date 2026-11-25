/** An Action or component was used after disposal, or a pending `next()` saw it end. */
export class ActionDisposedError extends Error {
  constructor() {
    super('Action has been disposed');
    this.name = 'ActionDisposedError';
  }
}

/** `next({ timeout })` saw no value in time, or a cleanup or operation passed its deadline. */
export class ActionTimeoutError extends Error {
  readonly timeout: number;

  constructor(timeout: number) {
    super(`No Action value received within ${timeout} ms`);
    this.name = 'ActionTimeoutError';
    this.timeout = timeout;
  }
}

/** Connecting or deriving would make an Action depend on itself. */
export class ActionCycleError extends Error {
  constructor() {
    super('Connecting this source would create an Action cycle');
    this.name = 'ActionCycleError';
  }
}

/** Avoid the host timer overflow/negative-delay coercions. */
export function validateMilliseconds(value: number): void {
  if (!Number.isInteger(value) || value < 0 || value > 2_147_483_647) {
    throw new RangeError('Milliseconds must be an integer between 0 and 2147483647');
  }
}

/** Promise assimilation also safely handles a throwing `then` getter. */
export function observeReturn(result: unknown, onError: (error: unknown) => void): void {
  if (result !== null && (typeof result === 'object' || typeof result === 'function')) {
    void Promise.resolve(result).then(undefined, onError);
  }
}

/** Standalone fallback must never close over an Action or its options. */
export function defaultErrorReporter(error: unknown): void {
  console.error('[Action] Unhandled error:', error);
}
