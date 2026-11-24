import {
  DynDestroyedError,
  DynError,
  DynMessageTooLargeError,
  DynNoResponderError,
  DynRateLimitExceededError,
  DynRequestCancelledError,
} from '@coldmind/bus/src/errors.ts'
import { DedupeWindow } from '@coldmind/bus/src/internal/dedupe.ts'
import { PerTopicRateLimiter, RateLimiter } from '@coldmind/bus/src/internal/limiter.ts'
import { createDefaultLogger } from '@coldmind/bus/src/internal/logger.ts'
import { PatternMatcher, parseFilter, validatePublishTopic } from '@coldmind/bus/src/internal/matcher.ts'
import { QueueProcessor } from '@coldmind/bus/src/internal/queue.ts'
import { RequestManager } from '@coldmind/bus/src/internal/request.ts'
import { RetainedStore } from '@coldmind/bus/src/internal/store.ts'
import { createStream } from '@coldmind/bus/src/internal/stream.ts'
import {
  InternalSub,
  calculateMessageSize,
  createMessageId,
  createSubscriptionId,
} from '@coldmind/bus/src/internal/subscription.ts'
import type {
  DeadLetterEntry,
  DynHandler,
  DynMessage,
  DynObserver,
  DynPubSubOptions,
  DynReplyError,
  DynStream,
  DynSubscription,
  IPubsub,
  Logger,
  PublishOptions,
  PublishResult,
  QoS,
  QueuedDelivery,
  RateLimiterStats,
  RequestOptions,
  SubscribeOptions,
} from '@coldmind/bus/src/types.ts'

// Re-export public API
export * from '@coldmind/bus/src/types.ts'
export * from '@coldmind/bus/src/errors.ts'
export { createStream } from '@coldmind/bus/src/internal/stream.ts'

/**
 * Whether `topic` matches `filter` (`+` one level, `#` the rest).
 * @throws {DynInvalidFilterError} When `validate` is on and the filter is malformed.
 */
export function topicMatches(filter: string, topic: string, validate = true): boolean {
  const tokens = parseFilter(filter, validate)
  const levels = topic.split('/')

  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i]
    if (token === '#') return true
    if (i >= levels.length) return false
    if (token === '+') continue
    if (token !== levels[i]) return false
  }

  return tokens.length === levels.length
}

export function topicMatchesAny(
  filters: readonly string[],
  topic: string,
  validate = true,
): boolean {
  return filters.some((filter) => topicMatches(filter, topic, validate))
}

/**
 * The core SignalBus orchestrator.
 * Combines pub/sub, request/response, and resiliency patterns into a single unit.
 *
 * @template TPayload The default payload type for messages
 * @template TMeta Optional metadata type for messages
 * @template TContext Optional contextual data passed to handlers
 */
export class SignalBus<TPayload = unknown, TMeta = unknown, TContext = unknown>
  implements IPubsub<TPayload, TMeta, TContext>
{
  readonly instanceId: string
  private readonly replyTopic: string
  private readonly validatePublish: boolean
  private readonly validateFilters: boolean
  private readonly ackStrategy: 'post-handler' | 'pre-queue'
  private readonly maxMessageSize: number

  private readonly matcher: PatternMatcher<TPayload, TMeta, TContext>
  private readonly queue: QueueProcessor<TPayload, TMeta, TContext>
  private readonly requests: RequestManager<TPayload, TMeta>
  private readonly retainedStore: RetainedStore<TPayload, TMeta>
  private readonly dedupe: DedupeWindow

  private readonly publishRateLimiter?: RateLimiter | PerTopicRateLimiter
  private readonly subscribeRateLimiter?: RateLimiter
  private readonly requestRateLimiter?: RateLimiter

  private destroyed = false
  private readonly abandonListeners = new Set<(correlationId: string, reason: unknown) => void>()
  private observer?: DynObserver<TPayload, TMeta>
  private logger: Logger

  constructor(options: DynPubSubOptions = {}) {
    this.instanceId = options.instanceId ?? createMessageId()
    this.replyTopic = `reply/${this.instanceId}`
    this.validatePublish = options.validatePublishTopic ?? true
    this.validateFilters = options.validateFilters ?? true
    this.ackStrategy = options.ackStrategy ?? 'post-handler'
    this.maxMessageSize = options.maxMessageSize ?? 0
    this.logger = options.logger ?? createDefaultLogger()

    this.matcher = new PatternMatcher()
    this.retainedStore = new RetainedStore()
    this.requests = new RequestManager()
    this.dedupe = new DedupeWindow(options.dedupeWindowMs ?? 60_000, options.dedupeMaxEntries ?? 10_000)

    if (options.rateLimits?.maxPublishPerSecond) {
      const burst = options.rateLimits.burstCapacity ?? options.rateLimits.maxPublishPerSecond
      if (options.rateLimits.perTopic) {
        this.publishRateLimiter = new PerTopicRateLimiter(
          options.rateLimits.maxPublishPerSecond,
          burst,
          options.rateLimits.maxTopics,
        )
      } else {
        this.publishRateLimiter = new RateLimiter(options.rateLimits.maxPublishPerSecond, burst)
      }
    }

    if (options.rateLimits?.maxSubscribePerSecond) {
      const burst = options.rateLimits.burstCapacity ?? options.rateLimits.maxSubscribePerSecond
      this.subscribeRateLimiter = new RateLimiter(options.rateLimits.maxSubscribePerSecond, burst)
    }

    if (options.rateLimits?.maxRequestPerSecond) {
      const burst = options.rateLimits.burstCapacity ?? options.rateLimits.maxRequestPerSecond
      this.requestRateLimiter = new RateLimiter(options.rateLimits.maxRequestPerSecond, burst)
    }

    const queueOpts = {
      dispatch: options.dispatch ?? 'parallel',
      stopOnError: options.stopOnError ?? false,
      ackTimeoutMs: options.ackTimeoutMs ?? 30_000,
      maxRetries: options.maxRetries ?? 3,
      retryDelayMs: options.retryDelayMs ?? 250,
      retryBackoffFactor: options.retryBackoffFactor ?? 2,
      requireExplicitAck: options.requireExplicitAck ?? false,
      defaultHandlerTimeoutMs: options.defaultHandlerTimeoutMs ?? 0,
      maxQueueSize: options.maxQueueSize ?? 10_000,
      deadLetter: {
        enabled: options.deadLetter?.enabled ?? true,
        maxSize: options.deadLetter?.maxSize ?? 1000,
        overflow: options.deadLetter?.overflow ?? 'drop-oldest',
      },
      circuitBreaker: {
        enabled: options.circuitBreaker?.enabled ?? false,
        failureThreshold: options.circuitBreaker?.failureThreshold ?? 5,
        successThreshold: options.circuitBreaker?.successThreshold ?? 2,
        timeout: options.circuitBreaker?.timeout ?? 60_000,
        halfOpenMaxAttempts: options.circuitBreaker?.halfOpenMaxAttempts ?? 3,
      },
    }

    this.queue = new QueueProcessor(
      queueOpts,
      undefined,
      async (topic, payload, correlationId, meta, flags) => {
        try {
          await this.sendReply(topic, payload, correlationId, meta, flags)
        } catch (error) {
          this.observer?.onError?.({
            type: DynError.HandlerError,
            topic,
            messageId: correlationId,
            error,
            correlationId,
            meta: meta ? (meta as Record<string, unknown>) : undefined,
          })
          throw error
        }
      },
    )

    // A request that is given up on (timeout, cancel) tells its responder to stop.
    this.requests.setAbandonHandler((correlationId, reason) => {
      this.queue.abortDeliveries(correlationId, reason)
      for (const listener of [...this.abandonListeners]) {
        try {
          listener(correlationId, reason)
        } catch (error) {
          this.observer?.onError?.({
            type: DynError.HandlerError,
            topic: '',
            messageId: correlationId,
            error,
            correlationId,
          })
        }
      }
    })

    this.installReplySubscription()
  }

  // Replies take their own path: straight to the waiting request when it was made on this bus,
  // otherwise to whoever subscribes to the reply topic. They do not pass the publish rate
  // limit or the delivery queue, so a reply is never held up or dropped by other traffic.
  private async sendReply(
    replyTo: string,
    payload: unknown,
    correlationId: string,
    meta?: TMeta,
    flags?: { more?: boolean; error?: DynReplyError },
  ): Promise<void> {
    const msg: DynMessage<TPayload, TMeta> = {
      id: createMessageId(),
      topic: replyTo,
      payload: payload as TPayload,
      qos: 0,
      retain: false,
      timestamp: Date.now(),
      meta,
      correlationId,
      ...(flags?.more ? { more: true } : {}),
      ...(flags?.error ? { error: flags.error } : {}),
    }

    if (replyTo === this.replyTopic) {
      this.requests.handleReply(msg)
      return
    }

    const subs = this.matcher.match(replyTo)
    if (subs.length > 0) await this.queue.runImmediate(subs, msg)
  }

  /**
   * Answers a request from outside its handler: pass the request message (or its replyTo and
   * correlationId). Does nothing when the message was not a request.
   */
  async reply(
    to: { replyTo?: string; correlationId?: string },
    payload: unknown,
    options: { meta?: TMeta; more?: boolean; error?: DynReplyError } = {},
  ): Promise<void> {
    this.ensureAlive()
    if (!to.replyTo || !to.correlationId) return
    await this.sendReply(to.replyTo, payload, to.correlationId, options.meta, {
      more: options.more,
      error: options.error,
    })
  }

  getLogger(): Logger {
    return this.logger
  }

  setObserver(obs: DynObserver<TPayload, TMeta>): void {
    this.observer = obs
    this.queue.setObserver(obs)
    this.requests.setObserver(obs)
  }

  /**
   * Subscribes to a topic pattern.
   * Supports wildcards: + (single level) and # (multi-level).
   *
   * @param filter The topic pattern to match (e.g. "sensors/+/temp")
   * @param handler Callback invoked when a message matches
   * @param options Subscription configuration (priority, receiveRetained, etc.)
   * @returns A subscription handle that can be used to unsubscribe
   */
  subscribe(
    filter: string,
    handler: DynHandler<TPayload, TMeta, TContext>,
    options: SubscribeOptions = {},
  ): DynSubscription {
    this.ensureAlive()

    if (this.subscribeRateLimiter && !this.subscribeRateLimiter.tryAcquire()) {
      const error = new DynRateLimitExceededError('subscribe')
      this.observer?.onError?.({
        type: DynError.RateLimitExceeded,
        topic: filter,
        messageId: 'N/A',
        error,
      })
      throw error
    }

    return this.addSubscription(filter, handler, options)
  }

  /**
   * Publishes a message to a specific topic.
   *
   * @param topic The exact topic to publish to
   * @param payload The message content
   * @param options Publish configuration (QoS, retain, delivery mode, etc.)
   * @returns The generated or provided message ID
   */
  async publish(
    topic: string,
    payload: TPayload,
    options: PublishOptions<TMeta, TContext> = {},
  ): Promise<PublishResult> {
    this.ensureAlive()
    if (this.validatePublish) validatePublishTopic(topic)

    const messageId = options.id ?? createMessageId()

    if (this.maxMessageSize > 0) {
      const size = calculateMessageSize(payload, options.meta)
      if (size > this.maxMessageSize) {
        const error = new DynMessageTooLargeError(size, this.maxMessageSize)
        this.observer?.onError?.({
          type: DynError.MessageTooLarge,
          topic,
          messageId,
          error,
          correlationId: options.correlationId,
          meta: options.meta ? (options.meta as Record<string, unknown>) : undefined,
        })
        throw error
      }
    }

    if (this.publishRateLimiter) {
      const allowed =
        this.publishRateLimiter instanceof PerTopicRateLimiter
          ? this.publishRateLimiter.tryAcquire(topic)
          : this.publishRateLimiter.tryAcquire()

      if (!allowed) {
        const error = new DynRateLimitExceededError('publish')
        this.observer?.onError?.({
          type: DynError.RateLimitExceeded,
          topic,
          messageId,
          error,
          correlationId: options.correlationId,
          meta: options.meta ? (options.meta as Record<string, unknown>) : undefined,
        })
        throw error
      }
    }

    const msg: DynMessage<TPayload, TMeta> = {
      id: messageId,
      topic,
      payload,
      qos: options.qos ?? 0,
      retain: options.retain ?? false,
      timestamp: options.timestamp ?? Date.now(),
      meta: options.meta,
      correlationId: options.correlationId,
      replyTo: options.replyTo,
      ...(options.more ? { more: true } : {}),
      ...(options.error ? { error: options.error } : {}),
    }

    // QoS 2: an id that was already accepted inside the window is not delivered again,
    // so a publisher that retries after a lost acknowledgement cannot cause a duplicate.
    const once = msg.qos === 2
    if (once && this.dedupe.has(msg.id)) return { id: msg.id, subscribers: 0, duplicate: true }

    if (msg.retain) {
      this.retainedStore.set(msg)
    }

    const subs = this.matcher.match(topic)
    if (once) this.dedupe.add(msg.id)
    if (subs.length === 0) return { id: msg.id, subscribers: 0 }

    try {
      return await this.route(msg, subs, options)
    } catch (error) {
      // Not accepted after all: the publisher may try the same id again.
      if (once) this.dedupe.delete(msg.id)
      throw error
    }
  }

  private async route(
    msg: DynMessage<TPayload, TMeta>,
    subs: ReturnType<PatternMatcher<TPayload, TMeta, TContext>['match']>,
    options: PublishOptions<TMeta, TContext>,
  ): Promise<PublishResult> {
    const topic = msg.topic
    const delivery = options.delivery ?? 'queue'
    const handlerTimeoutMs = options.handlerTimeoutMs
    const context = options.context

    if (delivery === 'immediate') {
      await this.queue.runImmediate(subs, msg, handlerTimeoutMs, context)
      return { id: msg.id, subscribers: subs.length }
    }

    if (this.ackStrategy === 'pre-queue' && msg.qos > 0) {
      this.observer?.onAccepted?.({
        topic,
        count: subs.length,
        messageId: msg.id,
        qos: msg.qos,
      })
    }

    const items: QueuedDelivery<TPayload, TMeta, TContext>[] = subs.map((sub) => ({
      sub,
      message: msg,
      priority: sub.priority,
      handlerTimeoutMs,
      context,
      fanout: subs.length,
    }))

    const result = this.queue.enqueue(items)
    this.observer?.onQueued?.({ topic, count: subs.length, messageId: msg.id })

    if (this.ackStrategy === 'post-handler' && msg.qos > 0) {
      await result
    } else {
      // QoS 0 and pre-queue acknowledgement intentionally return before
      // delivery completes, but cleared work must not become unhandled.
      void result.catch(() => {})
    }

    return { id: msg.id, subscribers: subs.length }
  }

  /**
   * Performs a request/response operation.
   *
   * @param topic The topic to send the request to
   * @param payload The request content
   * @param timeoutMs Maximum time to wait for a response (default 5000ms)
   * @param options Additional request configuration
   * @returns A promise that resolves with the response payload
   */
  async req<TReq extends TPayload, TRes>(
    topic: string,
    payload: TReq,
    timeoutMs = 5000,
    options: RequestOptions<TMeta, TContext> = {},
  ): Promise<TRes> {
    return this.reqWithId(createMessageId(), topic, payload, timeoutMs, options)
  }

  async reqWithId<TReq, TRes>(
    correlationId: string,
    topic: string,
    payload: TPayload,
    timeoutMs = 5000,
    options: RequestOptions<TMeta, TContext> = {},
  ): Promise<TRes> {
    this.ensureAlive()

    if (this.requestRateLimiter && !this.requestRateLimiter.tryAcquire()) {
      const error = new DynRateLimitExceededError('request')
      this.requests.handlePublishError(correlationId, topic, error)
      this.observer?.onError?.({
        type: DynError.RateLimitExceeded,
        topic,
        messageId: correlationId,
        error,
        correlationId,
      })
      throw error
    }

    // Nobody is subscribed: say so now instead of letting the request run into its timeout.
    if (this.matcher.match(topic).length === 0) {
      const error = new DynNoResponderError(topic)
      this.observer?.onError?.({
        type: DynError.NoResponder,
        topic,
        messageId: correlationId,
        error,
        correlationId,
      })
      throw error
    }

    const { signal, onChunk, ...publishOptions } = options
    if (signal?.aborted) throw new DynRequestCancelledError(topic, correlationId)

    const responsePromise = this.requests.createRequest<TRes>(correlationId, topic, timeoutMs, onChunk)
    // The answer can arrive (and fail) while the publish below is still in progress.
    responsePromise.catch(() => {})

    const onAbort = (): void => this.cancelRequest(correlationId)
    signal?.addEventListener('abort', onAbort, { once: true })

    try {
      await this.publish(topic, payload, {
        ...publishOptions,
        correlationId,
        replyTo: this.replyTopic,
      })
      return await responsePromise
    } catch (err) {
      this.requests.handlePublishError(correlationId, topic, err)
      throw err
    } finally {
      signal?.removeEventListener('abort', onAbort)
    }
  }

  /**
   * A request whose responder answers in several parts (`ctx.reply(part, meta, { more: true })`).
   * Iterate the parts; `result` resolves with the final reply.
   */
  reqStream<TChunk = unknown, TRes = unknown>(
    topic: string,
    payload: TPayload,
    timeoutMs = 5000,
    options: RequestOptions<TMeta, TContext> = {},
  ): DynStream<TChunk, TRes> {
    return createStream<TChunk, TRes>((onChunk, signal) => {
      const outer = options.signal
      const combined = outer ? AbortSignal.any([outer, signal]) : signal
      return this.reqWithId<TPayload, TRes>(createMessageId(), topic, payload, timeoutMs, {
        ...options,
        signal: combined,
        onChunk: (chunk) => {
          options.onChunk?.(chunk)
          onChunk(chunk)
        },
      })
    })
  }

  /** Gives up on a pending request and aborts the signal of the handler answering it. */
  cancelRequest(correlationId: string): void {
    this.requests.cancel(correlationId)
  }

  onAbandon(listener: (correlationId: string, reason: unknown) => void): () => void {
    this.abandonListeners.add(listener)
    return () => {
      this.abandonListeners.delete(listener)
    }
  }

  get queueLength(): number {
    return this.queue.queueLength
  }

  getRetained<T = TPayload>(topic: string): T | undefined {
    return this.retainedStore.get(topic)?.payload as unknown as T | undefined
  }

  getRetainedMessage<T = TPayload>(topic: string): DynMessage<T, TMeta> | undefined {
    return this.retainedStore.get(topic) as unknown as DynMessage<T, TMeta> | undefined
  }

  retained(filter: string): ReadonlyArray<DynMessage<TPayload, TMeta>> {
    return this.retainedStore.match(parseFilter(filter, this.validateFilters))
  }

  dlqSize(): number {
    return this.queue.dlqSize
  }

  peekDlq(count = 50): ReadonlyArray<DeadLetterEntry<TPayload, TMeta>> {
    return this.queue.peekDlq(count)
  }

  drainDlq(count = 50): DeadLetterEntry<TPayload, TMeta>[] {
    return this.queue.drainDlq(count)
  }

  clearRetained(): void {
    this.retainedStore.clear()
  }

  clearAll(): void {
    this.matcher.clear()
    this.retainedStore.clear()
    this.queue.clear()
    this.requests.clear()
    this.dedupe.clear()
    this.installReplySubscription()
  }

  destroy(): void {
    if (this.destroyed) return
    this.destroyed = true
    this.matcher.clear()
    this.retainedStore.clear()
    this.queue.destroy()
    this.requests.destroy()
  }

  get isDestroyed(): boolean {
    return this.destroyed
  }

  private removeSub(sub: InternalSub<TPayload, TMeta, TContext>): void {
    this.matcher.removeSubscription(sub)
    this.queue.removeSubscription(sub.id)
  }

  private installReplySubscription(): void {
    this.addSubscription(
      this.replyTopic,
      (msg) => {
        this.requests.handleReply(msg)
      },
      { receiveRetained: false },
    )
  }

  private addSubscription(
    filter: string,
    handler: DynHandler<TPayload, TMeta, TContext>,
    options: SubscribeOptions,
  ): DynSubscription {
    const tokens = parseFilter(filter, this.validateFilters)
    const priority = options.priority ?? 0
    const subId = createSubscriptionId(this.instanceId)
    const sub = new InternalSub(
      this,
      subId,
      filter,
      tokens,
      handler,
      priority,
      options.concurrency ?? 1,
    )

    this.matcher.addSubscription(sub)

    if (options.receiveRetained !== false && this.retainedStore.size > 0) {
      const msgs = this.retainedStore.match(tokens)
      if (msgs.length) {
        const items: QueuedDelivery<TPayload, TMeta, TContext>[] = msgs.map((message) => ({
          sub,
          message,
          priority: sub.priority,
        }))
        try {
          void this.queue.enqueue(items).catch(() => {})
        } catch (error) {
          this.matcher.removeSubscription(sub)
          throw error
        }
      }
    }

    return sub
  }

  getRateLimitStats():
    | {
        publish?: RateLimiterStats
        subscribe?: RateLimiterStats
        request?: RateLimiterStats
      }
    | undefined {
    if (!this.publishRateLimiter && !this.subscribeRateLimiter && !this.requestRateLimiter) {
      return undefined
    }

    const stats: any = {}

    if (this.publishRateLimiter) {
      if (this.publishRateLimiter instanceof PerTopicRateLimiter) {
        stats.publish = this.publishRateLimiter.totals()
      } else {
        stats.publish = this.publishRateLimiter.getStats()
      }
    }

    if (this.subscribeRateLimiter) {
      stats.subscribe = this.subscribeRateLimiter.getStats()
    }

    if (this.requestRateLimiter) {
      stats.request = this.requestRateLimiter.getStats()
    }

    return stats
  }

  private ensureAlive(): void {
    if (this.destroyed) throw new DynDestroyedError()
  }
}
