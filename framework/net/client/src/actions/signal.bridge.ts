import { Action, ActionComponent, ActionDisposedError, isActionTraceContext } from '@coldmind/wire';
import type { ActionLifecycleOptions, ActionSpan, ActionTraceContext } from '@coldmind/wire';
import type { DeliveryContext, PublishOptions, SubscribeOptions } from '@coldmind/bus';
import { defineSignalTopic, sourceDefinition, traceFrom } from '@coldmind/net.client/src/actions/signal.topic.ts';
import type { SignalMessage, SignalMetadata, SignalPort, SignalTopic } from '@coldmind/net.client/src/actions/signal.topic.ts';
import { milliseconds, SignalOperation, withSignal } from '@coldmind/net.client/src/actions/signal.operation.ts';

/** Lifecycle options plus the bridge's name, the prefix of its span and action names. Default "signal". */
export interface SignalBridgeOptions extends ActionLifecycleOptions { readonly name?: string }
/** Bus publish options plus tracing; the trace context travels in `meta.trace`. */
export interface SignalPublishOptions extends PublishOptions<SignalMetadata> {
  /** Parent trace context for the publish span. */
  readonly trace?: ActionTraceContext;
  /** The message this publish answers to; its trace is continued when `trace` is not given. */
  readonly causedBy?: SignalMessage<unknown>;
  /** Cancels this wait; messages already dispatched cannot be recalled. */
  readonly signal?: AbortSignal;
}
/** Subscribe options plus the time one handler run may take. Default 30 s. */
export interface SignalConsumeOptions extends SubscribeOptions { readonly timeoutMs?: number }
/** What a consumer receives besides the message: delivery details, reply functions, a cancel signal and its span. */
export interface SignalConsumeContext extends Omit<DeliveryContext<unknown, unknown, unknown>, 'ack' | 'nack'> {
  /** Aborts on the handler timeout, on release of the consumer, or when the bridge is destroyed. */
  readonly signal: AbortSignal;
  readonly trace: ActionTraceContext | undefined;
  readonly span: ActionSpan | undefined;
}
/** An async message handler; resolving acknowledges, throwing makes the bus deliver again (QoS ≥ 1) or dead-letter. */
export type SignalConsumeHandler<T> = (message: SignalMessage<T>, context: SignalConsumeContext) => void | PromiseLike<void>;

/** Lifecycle-compatible wiring for a borrowed bus; destroys only its own subscriptions and Actions. */
export class SignalActionBridge extends ActionComponent<Record<never, never>, Record<never, never>> {
  readonly #bus: SignalPort;
  constructor(bus: SignalPort, options: SignalBridgeOptions = {}) {
    super({ name: options.name ?? 'signal', inputs: {}, outputs: {} }, options);
    if (!bus || typeof bus.subscribe !== 'function' || typeof bus.publish !== 'function') throw new TypeError('Invalid SignalBus port');
    this.#bus = bus;
  }

  /**
   * A topic (validated payloads) or a filter (unvalidated) as an action of messages. Subscribed while observed;
   * retained messages only with `receiveRetained: true`. A payload that fails validation reaches `onError`.
   */
  stream<T>(topic: SignalTopic<T>, options?: SubscribeOptions): Action<SignalMessage<T>>;
  stream(filter: string, options?: SubscribeOptions): Action<SignalMessage<unknown>>;
  stream<T>(source: SignalTopic<T> | string, options: SubscribeOptions = {}): Action<SignalMessage<T>> {
    this.assertAlive();
    const definition = sourceDefinition(source);
    const subscriptionOptions = { ...options, receiveRetained: options.receiveRetained ?? false };
    const action = Action.fromCallback<SignalMessage<T>>((emit, fail, signal) => {
      this.assertAlive();
      const subscription = this.#bus.subscribe(definition.filter, (message, context) => {
        if (signal.aborted || this.signal.aborted) { context.ack(); return; }
        const parent = traceFrom(message);
        const span = this.telemetry?.startSpan('signal.receive.' + definition.filter, {
          ...(parent ? { parent } : {}), attributes: { topic: message.topic, messageId: message.id },
        });
        try {
          const value = Object.freeze({ ...message, payload: definition.parse(message.payload) });
          if (signal.aborted || this.signal.aborted) { context.ack(); span?.end('cancelled'); return; }
          emit(value); context.ack(); span?.end();
        } catch (error) { span?.end('error'); fail(error); throw error; }
      }, subscriptionOptions);
      return () => subscription.unsubscribe();
    }, {
      name: this.name + '.' + definition.filter,
      ...(this.telemetry ? { telemetry: this.telemetry } : {}),
      onUnhandledError: error => this.reportError(error),
    });
    this.own(() => action.dispose());
    return action;
  }

  /**
   * Validates `payload` against the topic and publishes it, in a span whose context goes along in `meta.trace`.
   * Starts the bridge if needed. Throws what validation or the bus throws.
   */
  async publish<T>(topic: SignalTopic<T>, payload: NoInfer<T>, options: SignalPublishOptions = {}): Promise<{ id: string }> {
    this.assertAlive(); options.signal?.throwIfAborted();
    const contract = defineSignalTopic(topic.name, topic.parse);
    const value = contract.parse(payload);
    const parent = options.trace ?? (options.causedBy ? traceFrom(options.causedBy) : undefined);
    if (parent !== undefined && !isActionTraceContext(parent)) throw new TypeError('Invalid trace context');
    await this.start(); this.assertAlive(); options.signal?.throwIfAborted();
    const span = this.telemetry?.startSpan('signal.publish.' + contract.name, { ...(parent ? { parent } : {}) });
    const { trace: _trace, causedBy: _cause, signal, ...publication } = options;
    const trace = span?.context ?? parent;
    const meta = trace ? { ...publication.meta, trace } : publication.meta;
    try {
      const pending = this.#bus.publish(contract.name, value, { ...publication, ...(meta ? { meta } : {}) });
      const result = await (signal ? withSignal(pending, signal) : pending);
      span?.end(); return result;
    } catch (error) {
      span?.end(signal?.aborted ? 'cancelled' : 'error');
      if (!signal?.aborted) this.reportError(error, span?.context);
      throw error;
    }
  }

  /**
   * Runs `handler` for every message, each run in its own span and bounded by `timeoutMs`.
   * Returns the release function; the bridge's `destroy()` releases it too.
   */
  consume<T>(topic: SignalTopic<T>, handler: SignalConsumeHandler<T>, options?: SignalConsumeOptions): () => Promise<void>;
  consume(filter: string, handler: SignalConsumeHandler<unknown>, options?: SignalConsumeOptions): () => Promise<void>;
  consume<T>(source: SignalTopic<T> | string, handler: SignalConsumeHandler<T>, options: SignalConsumeOptions = {}): () => Promise<void> {
    this.assertAlive();
    if (typeof handler !== 'function') throw new TypeError('Expected an async consumer');
    const definition = sourceDefinition(source);
    const timeoutMs = milliseconds(options.timeoutMs ?? 30_000);
    const lifetime = new AbortController();
    const subscription = this.#bus.subscribe(definition.filter, async (message, context) => {
      if (lifetime.signal.aborted || this.signal.aborted) { context.ack(); return; }
      const operation = new SignalOperation(timeoutMs, [this.signal, lifetime.signal]);
      const parent = traceFrom(message);
      const span = this.telemetry?.startSpan('signal.consume.' + definition.filter, {
        ...(parent ? { parent } : {}), attributes: { topic: message.topic, messageId: message.id, attempt: context.attempt },
      });
      try {
        operation.check();
        const value = Object.freeze({ ...message, payload: definition.parse(message.payload) });
        await operation.wait(handler(value, Object.freeze({
          subId: context.subId, filter: context.filter, attempt: context.attempt,
          priority: context.priority, reply: context.reply, replyError: context.replyError,
          ...(context.context === undefined ? {} : { context: context.context }),
          signal: operation.signal, trace: span?.context ?? parent, span,
        })));
        context.ack(); span?.end();
      } catch (error) {
        span?.end(operation.signal.aborted ? 'cancelled' : 'error');
        if (lifetime.signal.aborted || this.signal.aborted) { context.ack(); return; }
        this.reportError(error, span?.context); throw error;
      } finally { operation.close(); }
    }, { receiveRetained: options.receiveRetained ?? false, ...(options.priority === undefined ? {} : { priority: options.priority }) });
    const release = this.own(() => { subscription.unsubscribe(); lifetime.abort(new ActionDisposedError()); });
    return async () => { await release(); };
  }
}
