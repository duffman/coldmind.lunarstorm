import { isActionTraceContext } from '@coldmind/wire';
import type { ActionTraceContext } from '@coldmind/wire';
import { topicMatches } from '@coldmind/bus';
import type { DynMessage, IPubsub } from '@coldmind/bus';
import { validate } from '@coldmind/rpc';
import type { OutputOf, Schema } from '@coldmind/rpc';

/** What the wire needs of a bus: a local `SignalBus` and a `RemoteBus` both are one. */
export type SignalPort = Pick<IPubsub<unknown, unknown, unknown>, 'subscribe' | 'publish'>;
/** A bus message with a validated payload. */
export type SignalMessage<T> = DynMessage<T, unknown>;
/** Message metadata; `trace` carries the publisher's trace context. */
export interface SignalMetadata { readonly trace?: ActionTraceContext; readonly [key: string]: unknown }
/** A literal topic name and the parser its payloads pass on publish and on receipt. */
export interface SignalTopic<T> { readonly name: string; readonly parse: (value: unknown) => T }

/**
 * Runtime validation belongs to the topic boundary; generic casts are not validators.
 * The payload is described by a parser function, or by a model, shape or field from `@coldmind/model`.
 */
export function defineSignalTopic<T>(name: string, parse: (value: unknown) => T): SignalTopic<T>;
export function defineSignalTopic<S extends Schema>(name: string, schema: S): SignalTopic<OutputOf<S>>;
export function defineSignalTopic(name: string, payload: ((value: unknown) => unknown) | Schema): SignalTopic<unknown> {
  validateTopic(name);
  if (typeof payload === 'function') return Object.freeze({ name, parse: payload });
  if (!payload || typeof payload !== 'object') throw new TypeError('A topic requires a payload parser');
  return Object.freeze({ name, parse: (value: unknown) => validate(payload, value) });
}

/** @throws {TypeError} Unless `name` is a nonempty literal topic (no `+` or `#`) of at most 200 characters. */
export function validateTopic(name: string): void {
  if (typeof name !== 'string' || !name.trim() || name.length > 200 || /[+#]/.test(name)) {
    throw new TypeError('Expected a nonempty literal topic of at most 200 characters');
  }
}

/** The filter and parser for a topic, or for a filter string (payloads passed through unvalidated). */
export function sourceDefinition<T>(source: SignalTopic<T> | string): { filter: string; parse: (value: unknown) => T } {
  if (typeof source === 'string') {
    if (!source.trim() || source.length > 200) throw new TypeError('Invalid topic filter');
    topicMatches(source, 'probe');
    return { filter: source, parse: value => value as T };
  }
  const topic = defineSignalTopic(source.name, source.parse);
  return { filter: topic.name, parse: topic.parse };
}

/** The well-formed trace context in `meta.trace`, or undefined. */
export function traceFrom(message: SignalMessage<unknown>): ActionTraceContext | undefined {
  const meta = message.meta;
  if (!meta || typeof meta !== 'object') return undefined;
  const trace = (meta as { trace?: unknown }).trace;
  return isActionTraceContext(trace) ? Object.freeze({ ...trace }) : undefined;
}
