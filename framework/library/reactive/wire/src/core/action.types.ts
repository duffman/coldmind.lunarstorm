/** Idempotently detach a subscription or release a source. */
export type Unsubscribe = () => void;

/** Returned promises are observed for rejection; delivery itself is synchronous. */
export type DataHandler<T> = (data: T) => unknown;
export type ErrorHandler = (error: unknown) => unknown;

export interface ActionOptions {
  readonly name?: string;
  readonly telemetry?: ActionTelemetry;
  /** Receives otherwise-unhandled errors and errors thrown by error handlers. */
  readonly onUnhandledError?: ErrorHandler;
}

export interface SubscriptionOptions {
  readonly signal?: AbortSignal;
}

export interface NextOptions extends SubscriptionOptions {
  /** Milliseconds, inclusive range 0–2147483647. Omit to wait indefinitely. */
  readonly timeout?: number;
}

/**
 * Registration is synchronous and may emit immediately. Return a synchronous
 * cleanup function; the supplied signal aborts before cleanup runs.
 */
export type CallbackSource<T> = (
  emit: (data: T) => void,
  error: (reason: unknown) => void,
  signal: AbortSignal,
) => void | Unsubscribe;

export type AsyncMapper<T, R> = (data: T, signal: AbortSignal) => PromiseLike<R>;

export type Equality<T> = (previous: T, current: T) => boolean;
import type { ActionTelemetry } from '@coldmind/wire/src/telemetry/action.telemetry.ts';

