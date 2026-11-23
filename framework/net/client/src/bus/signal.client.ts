import {
  DynDestroyedError, DynRequestCancelledError, createStream, topicMatches,
  type DeadLetterEntry, type DeliveryContext, type DynHandler, type DynMessage, type DynObserver, type DynReplyError, type DynStream,
  type DynSubscription, type IPubsub, type Logger, type PublishOptions, type PublishResult, type QoS, type RateLimiterStats,
  type ReplyOptions, type RequestOptions, type SubscribeOptions,
} from "@coldmind/bus";
import type { Api, Client } from "@coldmind/net.client/src/rpc/rpc.client.ts";
import { RpcError } from "@coldmind/rpc";
import { busContract, type BusChunk, type BusMessage } from "@coldmind/protocol";
import { toBusError } from "@coldmind/net.client/src/bus/signal.client.errors.ts";

/** Identity, retries and the dead letter queue of a `RemoteBus`. */
export type RemoteBusOptions = {
  /** Stable id of this client. Guaranteed subscriptions are kept under it while the connection is down. Default: random. */
  clientId?: string;
  /** How often a failing handler is tried again for a message with QoS ≥ 1, and the delay before the first retry. Defaults 3 and 250 ms, doubling. */
  maxRetries?: number;
  retryDelayMs?: number;
  /** How many messages that failed for good are kept for `peekDlq`. Default 1000. */
  maxDlq?: number;
  /** What `getLogger()` returns. Default: silent. */
  logger?: Logger;
};

type Sub<P, M, C> = DynSubscription & {
  handler: DynHandler<P, M, C>;
  options: SubscribeOptions;
  qos: QoS;
  durable: boolean;
  abort: AbortController;
  stopped: boolean;
  waiting: BusMessage[];
  running: number;
  concurrency: number;
  /** Highest seq taken in, highest seq below which everything is handled, and what is handled beyond that. */
  lastSeq: number;
  doneSeq: number | undefined;
  done: Set<number>;
  ackSent: number;
  acking: boolean;
};

/** Marks a message as answerable. The real reply topic never leaves the server. */
const REMOTE_REPLY = "$remote";
const noop = () => {};
const silent: Logger = { error: noop, warn: noop, info: noop, debug: noop };
const sleep = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms));
const lost = (e: unknown) => e instanceof RpcError && (e.code === "CLOSED" || e.code === "TRANSPORT" || e.code === "TIMEOUT");

/**
 * The bus, reached over the network. Implements the same `IPubsub` as a local bus, so code written
 * against one runs against the other: subscribe, publish, request and reply all go through the
 * server's `bus` controller.
 *
 * Over a link that reconnects it recovers by itself: subscriptions are made again, a QoS ≥ 1
 * subscription continues where it stopped (the server kept what was not acknowledged), and a
 * QoS ≥ 1 publish that was in flight is sent again under the same message id.
 *
 * What stays local: `getRetained`/`retained` answer from retained messages this client has
 * received through its subscriptions, the dead letter queue holds this client's own failed
 * deliveries, and `delivery`, `handlerTimeoutMs` and `context` of the publish options are not sent.
 */
export class RemoteBus<TPayload = unknown, TMeta = unknown, TContext = unknown> implements IPubsub<TPayload, TMeta, TContext> {
  readonly instanceId: string;
  private readonly api: Api<typeof busContract.procs>;
  private readonly subs = new Set<Sub<TPayload, TMeta, TContext>>();
  private readonly retainedCache = new Map<string, DynMessage<TPayload, TMeta>>();
  private readonly dlq: DeadLetterEntry<TPayload, TMeta>[] = [];
  /** Requests this client is answering, and requests it has made: both cancellable by correlationId. */
  private readonly responding = new Map<string, AbortController>();
  private readonly requesting = new Map<string, AbortController>();
  private observer?: DynObserver<TPayload, TMeta>;
  private destroyed = false;
  private subSeq = 0;
  /** Publishes go out one at a time, so they reach the bus in the order they were made, as on a local bus. */
  private publishing: Promise<unknown> = Promise.resolve();

  constructor(private readonly client: Client, private readonly opts: RemoteBusOptions = {}) {
    this.instanceId = opts.clientId ?? crypto.randomUUID();
    this.api = client.api(busContract);
  }

  getLogger(): Logger {
    return this.opts.logger ?? silent;
  }

  /** Receives this client's deliveries, handler errors and dead letters. */
  setObserver(obs: DynObserver<TPayload, TMeta>): void {
    this.observer = obs;
  }

  // ---- subscribe ----

  /**
   * Subscribes through the server's `bus.subscribe`. QoS ≥ 1 makes it guaranteed under `clientId`: the server keeps
   * unacknowledged messages while the connection is down. A refused subscription ends and is reported to the observer.
   */
  subscribe(filter: string, handler: DynHandler<TPayload, TMeta, TContext>, options: SubscribeOptions = {}): DynSubscription {
    this.ensureAlive();
    const qos = options.qos ?? 0;
    const sub: Sub<TPayload, TMeta, TContext> = {
      id: `sub_${this.instanceId}_${++this.subSeq}`,
      filter,
      priority: options.priority ?? 0,
      handler, options, qos,
      durable: qos > 0,
      abort: new AbortController(),
      stopped: false,
      waiting: [], running: 0,
      concurrency: options.concurrency !== undefined && options.concurrency >= 1 ? options.concurrency : 1,
      lastSeq: 0, doneSeq: undefined, done: new Set(), ackSent: -1, acking: false,
      unsubscribe: () => this.unsubscribe(sub),
    };
    this.subs.add(sub);
    void this.run(sub);
    return sub;
  }

  private unsubscribe(sub: Sub<TPayload, TMeta, TContext>): void {
    if (sub.stopped) return;
    sub.stopped = true;
    this.subs.delete(sub);
    sub.abort.abort();
    sub.waiting = [];
    // A guaranteed subscription lives on the server until it is told to end.
    if (sub.durable && this.client.state !== "closed")
      this.api.unsubscribe({ client: this.instanceId, filter: sub.filter }).catch(noop);
  }

  /** Keeps the subscription's stream open for as long as the subscription exists, across lost connections. */
  private async run(sub: Sub<TPayload, TMeta, TContext>): Promise<void> {
    let failures = 0;
    while (!sub.stopped) {
      const started = Date.now();
      try {
        await this.api.subscribe(
          { filter: sub.filter, qos: sub.qos, receiveRetained: sub.options.receiveRetained, ...(sub.durable ? { client: this.instanceId } : {}) },
          { signal: sub.abort.signal, timeoutMs: 0, onChunk: chunk => this.receive(sub, chunk) },
        );
        return; // the server ended it
      } catch (e) {
        if (sub.stopped) return;
        if (!lost(e)) {
          // Refused, replaced from another connection, or ended by the server: not something to retry.
          this.observer?.onError?.({ type: "handlerError", topic: sub.filter, messageId: "N/A", error: e, subId: sub.id, filter: sub.filter });
          sub.stopped = true;
          this.subs.delete(sub);
          return;
        }
        failures = Date.now() - started > 1000 ? 0 : failures + 1;
        try {
          await this.client.whenUp();
        } catch {
          return; // closed for good
        }
        if (failures > 1) await sleep(Math.min(5000, 100 * 2 ** failures));
      }
    }
  }

  private receive(sub: Sub<TPayload, TMeta, TContext>, chunk: BusChunk): void {
    if (sub.stopped) return;
    if ("ready" in chunk) {
      // Attached (again): whatever was acknowledged before may not have arrived, so say it once more.
      sub.ackSent = -1;
      this.sendAck(sub);
      return;
    }
    if ("cancel" in chunk) {
      this.responding.get(chunk.cancel)?.abort(new DynRequestCancelledError(sub.filter, chunk.cancel));
      return;
    }
    if (chunk.seq > 0) {
      // Delivered again after a reconnect: already here, only the acknowledgement was missing.
      if (chunk.seq <= sub.lastSeq) return this.sendAck(sub);
      sub.lastSeq = chunk.seq;
      sub.doneSeq ??= chunk.seq - 1;
    }
    sub.waiting.push(chunk);
    this.pump(sub);
  }

  private pump(sub: Sub<TPayload, TMeta, TContext>): void {
    while (!sub.stopped && sub.running < sub.concurrency && sub.waiting.length > 0) {
      const next = sub.waiting.shift()!;
      sub.running++;
      void this.handle(sub, next).finally(() => {
        sub.running--;
        this.pump(sub);
      });
    }
  }

  private async handle(sub: Sub<TPayload, TMeta, TContext>, m: BusMessage): Promise<void> {
    const request = m.replyable && m.correlationId !== undefined ? m.correlationId : undefined;
    const msg: DynMessage<TPayload, TMeta> = {
      id: m.id,
      topic: m.topic,
      payload: m.payload as TPayload,
      qos: m.qos,
      retain: m.retain,
      timestamp: m.timestamp,
      meta: m.meta as TMeta | undefined,
      ...(m.correlationId !== undefined ? { correlationId: m.correlationId } : {}),
      ...(request !== undefined ? { replyTo: REMOTE_REPLY } : {}),
    };
    if (m.retain) {
      if (m.payload === undefined || m.payload === null) this.retainedCache.delete(m.topic);
      else this.retainedCache.set(m.topic, msg);
    }

    const controller = new AbortController();
    const onStop = () => controller.abort(sub.abort.signal.reason);
    sub.abort.signal.addEventListener("abort", onStop, { once: true });
    if (request !== undefined) this.responding.set(request, controller);

    let answered = false;
    const send = (payload: unknown, meta: TMeta | undefined, flags: { more?: boolean; error?: DynReplyError }): Promise<void> => {
      if (request === undefined) return Promise.resolve();
      if (!flags.more) {
        answered = true;
        if (this.responding.get(request) === controller) this.responding.delete(request);
      }
      return this.api.reply({ correlationId: request, payload, meta, client: this.instanceId, ...flags }).then(noop);
    };

    const maxAttempts = m.qos > 0 ? 1 + (this.opts.maxRetries ?? 3) : 1;
    const started = Date.now();
    for (let attempt = 1; ; attempt++) {
      let nacked: { error: unknown } | undefined;
      const ctx: DeliveryContext<TPayload, TMeta, TContext> = {
        subId: sub.id,
        filter: sub.filter,
        attempt,
        priority: sub.priority,
        signal: controller.signal,
        ack: noop,
        nack: error => void (nacked ??= { error: error ?? new Error("nack() called") }),
        reply: (payload, meta, options?: ReplyOptions) => send(payload, meta, options?.more ? { more: true } : {}),
        replyError: (error, meta) =>
          send(undefined, meta, {
            error: error instanceof Error ? { code: (error as { code?: string }).code ?? "requestFailed", message: error.message } : error,
          }),
      };
      try {
        await sub.handler(msg, ctx);
        if (nacked) throw nacked.error;
        this.observer?.onDelivered?.({ topic: m.topic, latency: Date.now() - started, messageId: m.id, subId: sub.id });
        break;
      } catch (error) {
        this.observer?.onError?.({ type: "handlerError", topic: m.topic, messageId: m.id, error, subId: sub.id, filter: sub.filter, attempt });
        if (attempt >= maxAttempts || sub.stopped) {
          // Same as a local bus: the requester hears about the failure, and a guaranteed message ends in the dead letter queue.
          if (!answered && request !== undefined)
            await send(undefined, undefined, { error: { code: "handlerError", message: error instanceof Error ? error.message : String(error) } }).catch(noop);
          if (m.qos > 0) this.deadLetter({ enqueuedAt: Date.now(), attempts: attempt, subId: sub.id, filter: sub.filter, message: msg, error });
          break;
        }
        await sleep((this.opts.retryDelayMs ?? 250) * 2 ** (attempt - 1));
      }
    }

    sub.abort.signal.removeEventListener("abort", onStop);
    // Not answered yet: the handler may still reply later through `reply()`, so its signal stays
    // reachable for a cancel until it does, or until the server stops accepting the reply.
    if (request !== undefined && this.responding.get(request) === controller) {
      const timer = setTimeout(() => {
        if (this.responding.get(request) === controller) this.responding.delete(request);
      }, 60_000);
      (timer as { unref?: () => void }).unref?.();
    }
    if (m.seq > 0) {
      sub.done.add(m.seq);
      while (sub.doneSeq !== undefined && sub.done.delete(sub.doneSeq + 1)) sub.doneSeq++;
      this.sendAck(sub);
    }
  }

  /** Acknowledges everything handled so far in one go. One acknowledgement is in flight at a time. */
  private sendAck(sub: Sub<TPayload, TMeta, TContext>): void {
    if (!sub.durable || sub.stopped || sub.acking || sub.doneSeq === undefined || sub.doneSeq <= sub.ackSent) return;
    const seq = sub.doneSeq;
    sub.acking = true;
    this.api.ack({ client: this.instanceId, filter: sub.filter, seq }).then(
      () => {
        sub.acking = false;
        sub.ackSent = Math.max(sub.ackSent, seq);
        this.sendAck(sub);
      },
      () => {
        // Sent again when the next message is handled, or when the subscription is attached again.
        sub.acking = false;
      },
    );
  }

  private deadLetter(entry: DeadLetterEntry<TPayload, TMeta>): void {
    if (this.dlq.length >= (this.opts.maxDlq ?? 1000)) this.dlq.shift();
    this.dlq.push(entry);
    this.observer?.onDeadLetter?.(entry);
  }

  // ---- publish, request, reply ----

  /** Publishes through `bus.publish`, in call order. QoS ≥ 1 is sent again under the same id after a lost connection. */
  publish(topic: string, payload: TPayload, options: PublishOptions<TMeta, TContext> = {}): Promise<PublishResult> {
    this.ensureAlive();
    const sent = this.publishing.then(() => this.send(topic, payload, options));
    this.publishing = sent.catch(noop);
    return sent;
  }

  private async send(topic: string, payload: TPayload, options: PublishOptions<TMeta, TContext>): Promise<PublishResult> {
    const qos = options.qos ?? 0;
    // A guaranteed publish keeps its id across attempts, so sending it again after a lost connection is safe with QoS 2
    // and at-least-once with QoS 1.
    const id = options.id ?? (qos > 0 ? crypto.randomUUID() : undefined);
    for (let attempt = 0; ; attempt++) {
      try {
        return await this.api.publish({ topic, payload, qos, retain: options.retain, meta: options.meta, id });
      } catch (e) {
        if (qos === 0 || !lost(e) || attempt >= (this.opts.maxRetries ?? 3)) throw e;
        await this.client.whenUp();
      }
    }
  }

  req<TReq extends TPayload, TRes>(topic: string, payload: TReq, timeoutMs = 5000, options: RequestOptions<TMeta, TContext> = {}): Promise<TRes> {
    return this.reqWithId(crypto.randomUUID(), topic, payload, timeoutMs, options);
  }

  async reqWithId<TReq extends TPayload, TRes>(
    correlationId: string,
    topic: string,
    payload: TReq,
    timeoutMs = 5000,
    options: RequestOptions<TMeta, TContext> = {},
  ): Promise<TRes> {
    this.ensureAlive();
    const controller = new AbortController();
    const signal = options.signal ? AbortSignal.any([options.signal, controller.signal]) : controller.signal;
    this.requesting.set(correlationId, controller);
    try {
      const { payload: result } = await this.api.request(
        { topic, payload, timeoutMs, qos: options.qos, meta: options.meta, correlationId },
        // The bus enforces the request timeout; the call only needs to outlive it.
        { signal, onChunk: options.onChunk, timeoutMs: timeoutMs + 5000 },
      );
      return result as TRes;
    } catch (e) {
      throw toBusError(e, topic, correlationId, timeoutMs);
    } finally {
      if (this.requesting.get(correlationId) === controller) this.requesting.delete(correlationId);
    }
  }

  reqStream<TChunk = unknown, TRes = unknown>(
    topic: string,
    payload: TPayload,
    timeoutMs = 5000,
    options: RequestOptions<TMeta, TContext> = {},
  ): DynStream<TChunk, TRes> {
    return createStream<TChunk, TRes>((onChunk, signal) =>
      this.reqWithId<TPayload, TRes>(crypto.randomUUID(), topic, payload, timeoutMs, {
        ...options,
        signal: options.signal ? AbortSignal.any([options.signal, signal]) : signal,
        onChunk: chunk => {
          options.onChunk?.(chunk);
          onChunk(chunk);
        },
      }),
    );
  }

  async reply(
    to: { replyTo?: string; correlationId?: string },
    payload: unknown,
    options: { meta?: TMeta; more?: boolean; error?: DynReplyError } = {},
  ): Promise<void> {
    this.ensureAlive();
    if (!to.replyTo || !to.correlationId) return;
    if (!options.more) this.responding.delete(to.correlationId);
    await this.api.reply({ correlationId: to.correlationId, payload, meta: options.meta, more: options.more, error: options.error, client: this.instanceId });
  }

  cancelRequest(correlationId: string): void {
    this.requesting.get(correlationId)?.abort();
  }

  // ---- what a local bus answers from its own state ----

  get queueLength(): number {
    let n = 0;
    for (const sub of this.subs) n += sub.waiting.length;
    return n;
  }

  getRetained<T = TPayload>(topic: string): T | undefined {
    return this.retainedCache.get(topic)?.payload as unknown as T | undefined;
  }

  getRetainedMessage<T = TPayload>(topic: string): DynMessage<T, TMeta> | undefined {
    return this.retainedCache.get(topic) as unknown as DynMessage<T, TMeta> | undefined;
  }

  retained(filter: string): ReadonlyArray<DynMessage<TPayload, TMeta>> {
    return [...this.retainedCache.values()].filter(msg => topicMatches(filter, msg.topic));
  }

  dlqSize(): number {
    return this.dlq.length;
  }

  peekDlq(count = 50): ReadonlyArray<DeadLetterEntry<TPayload, TMeta>> {
    return this.dlq.slice(0, Math.max(0, count));
  }

  drainDlq(count = 50): DeadLetterEntry<TPayload, TMeta>[] {
    return this.dlq.splice(0, Math.max(0, count));
  }

  clearRetained(): void {
    this.retainedCache.clear();
  }

  clearAll(): void {
    for (const sub of [...this.subs]) this.unsubscribe(sub);
    this.retainedCache.clear();
    this.dlq.length = 0;
  }

  /** Ends this client's subscriptions and requests. The Client it was given is left open. */
  destroy(): void {
    if (this.destroyed) return;
    this.clearAll();
    for (const controller of this.requesting.values()) controller.abort();
    this.destroyed = true;
  }

  get isDestroyed(): boolean {
    return this.destroyed;
  }

  getRateLimitStats(): { publish?: RateLimiterStats; subscribe?: RateLimiterStats; request?: RateLimiterStats } | undefined {
    return undefined;
  }

  private ensureAlive(): void {
    if (this.destroyed) throw new DynDestroyedError();
  }
}
