import type { TDynError } from '@coldmind/bus/src/errors.ts'

export type LogLevel = 'debug' | 'info' | 'warn' | 'error'

export interface Logger {
  error(message: string, data?: unknown): void
  warn(message: string, data?: unknown): void
  info(message: string, data?: unknown): void
  debug(message: string, data?: unknown): void
}

export type QoS = 0 | 1 | 2
export type DeliveryMode = 'queue' | 'immediate'
export type DispatchMode = 'parallel' | 'sequential'
export type AckStrategy = 'post-handler' | 'pre-queue'

/** A reply that reports a failure instead of a result. */
export interface DynReplyError {
  code: string
  message: string
  data?: unknown
}

export interface DynMessage<TPayload = unknown, TMeta = unknown> {
  id: string
  topic: string
  payload: TPayload
  qos: QoS
  retain: boolean
  timestamp: number
  meta?: TMeta
  correlationId?: string
  replyTo?: string
  /** On a reply: more replies follow for the same request (streamed response). */
  more?: boolean
  /** On a reply: the request failed. The requester's promise rejects with DynRequestError. */
  error?: DynReplyError
}

export interface PublishOptions<TMeta = unknown, TContext = unknown> {
  qos?: QoS
  retain?: boolean
  meta?: TMeta
  id?: string
  correlationId?: string
  replyTo?: string
  delivery?: DeliveryMode
  handlerTimeoutMs?: number
  context?: TContext
  /** Originating bus time. Defaults to the local clock. */
  timestamp?: number
  /** Marks a hand-published reply as one of several. See DynMessage.more. */
  more?: boolean
  /** Marks a hand-published reply as a failure. See DynMessage.error. */
  error?: DynReplyError
}

export interface SubscribeOptions {
  receiveRetained?: boolean
  priority?: number
  /**
   * How many deliveries this subscription may have in progress at once in parallel dispatch.
   * Default 1: the subscription receives its messages one at a time, in order.
   * Subscriptions never wait for each other.
   */
  concurrency?: number
  /** Requested delivery guarantee for subscriptions that cross a network. A local bus ignores it. */
  qos?: QoS
}

export interface ReplyOptions {
  /** More replies follow. The requester receives this one as a chunk and keeps waiting. */
  more?: boolean
}

export interface DeliveryContext<TPayload = unknown, TMeta = unknown, TContext = unknown> {
  subId: string
  filter: string
  attempt: number
  priority: number
  context?: TContext
  /**
   * Aborted when the request this delivery answers is cancelled or times out,
   * when the handler's own timeout fires, and when the bus is destroyed.
   */
  signal: AbortSignal
  ack: () => void
  nack: (error?: unknown) => void
  reply: <T>(payload: T, meta?: TMeta, options?: ReplyOptions) => Promise<void>
  /** Answers the request with a failure. The requester gets it at once instead of a timeout. */
  replyError: (error: DynReplyError | Error, meta?: TMeta) => Promise<void>
}

export type DynHandler<TPayload = unknown, TMeta = unknown, TContext = unknown> = (
  msg: DynMessage<TPayload, TMeta>,
  ctx: DeliveryContext<TPayload, TMeta, TContext>,
) => void | Promise<void>

export interface DynSubscription {
  readonly id: string
  readonly filter: string
  readonly priority: number
  unsubscribe(): void
}

export interface DeadLetterEntry<TPayload = unknown, TMeta = unknown> {
  enqueuedAt: number
  attempts: number
  subId: string
  filter: string
  message: DynMessage<TPayload, TMeta>
  error: unknown
}

export interface DeadLetterOptions {
  enabled?: boolean
  maxSize?: number
  overflow?: 'drop-oldest' | 'drop-newest'
}

export interface CircuitBreakerOptions {
  enabled?: boolean
  failureThreshold?: number
  successThreshold?: number
  timeout?: number
  halfOpenMaxAttempts?: number
}

export interface RateLimitOptions {
  maxPublishPerSecond?: number
  maxSubscribePerSecond?: number
  maxRequestPerSecond?: number
  burstCapacity?: number
  perTopic?: boolean
  /** With perTopic: how many topics are tracked at once before the least recently used is let go. Default 10000. */
  maxTopics?: number
}

export interface DynPubSubOptions {
  instanceId?: string
  dispatch?: DispatchMode
  stopOnError?: boolean
  validatePublishTopic?: boolean
  validateFilters?: boolean
  ackTimeoutMs?: number
  maxRetries?: number
  retryDelayMs?: number
  retryBackoffFactor?: number
  requireExplicitAck?: boolean
  defaultHandlerTimeoutMs?: number
  maxQueueSize?: number
  maxMessageSize?: number
  deadLetter?: DeadLetterOptions
  ackStrategy?: AckStrategy
  circuitBreaker?: CircuitBreakerOptions
  rateLimits?: RateLimitOptions
  logger?: Logger
  /** QoS 2: a message id published again inside this window is accepted once. Default 60000. */
  dedupeWindowMs?: number
  /** QoS 2: how many message ids the window remembers. Default 10000. */
  dedupeMaxEntries?: number
}

export interface DynErrorEvent {
  type: TDynError
  topic: string
  messageId: string
  error: unknown
  subId?: string
  filter?: string
  attempt?: number
  correlationId?: string
  meta?: Record<string, unknown>
}

export interface DynObserver<TPayload = unknown, TMeta = unknown> {
  onAccepted?: (data: { topic: string; count: number; messageId: string; qos: QoS }) => void
  onQueued?: (data: { topic: string; count: number; messageId: string }) => void
  onDelivered?: (data: { topic: string; latency: number; messageId: string; subId: string }) => void
  onError?: (err: DynErrorEvent) => void
  onDeadLetter?: (entry: DeadLetterEntry<TPayload, TMeta>) => void
}

export interface RequestOptions<TMeta = unknown, TContext = unknown> {
  qos?: QoS
  meta?: TMeta
  delivery?: DeliveryMode
  handlerTimeoutMs?: number
  context?: TContext
  /** Cancels the request. The responder's DeliveryContext.signal is aborted. */
  signal?: AbortSignal
  /** Receives each streamed reply (`more: true`). Every chunk restarts the timeout. */
  onChunk?: (chunk: unknown) => void
}

/** A streamed response: iterate the chunks, await `result` for the final reply. */
export interface DynStream<TChunk = unknown, TRes = unknown> extends AsyncIterable<TChunk> {
  readonly result: Promise<TRes>
  cancel(): void
}

export interface PublishResult {
  id: string
  /** How many subscriptions the message was routed to. */
  subscribers: number
  /** QoS 2: this id was already accepted inside the dedupe window, so nothing was delivered again. */
  duplicate?: boolean
}

export type Token = string | '+' | '#'

export interface InternalSubscription<TPayload, TMeta, TContext> extends DynSubscription {
  readonly tokens: Token[]
  readonly handler: DynHandler<TPayload, TMeta, TContext>
  readonly concurrency: number
  unsubscribe(): void
}

export interface QueuedDelivery<TPayload, TMeta, TContext> {
  sub: InternalSubscription<TPayload, TMeta, TContext>
  message: DynMessage<TPayload, TMeta>
  priority: number
  /** Enqueue order. Equal priorities leave the heap in this order. */
  seq?: number
  handlerTimeoutMs?: number
  context?: TContext
  /** How many subscriptions this message was routed to. */
  fanout?: number
  resolve?: () => void
  reject?: (error: unknown) => void
}

export interface RateLimiterStats {
  allowed: number
  rejected: number
  currentTokens: number
}

/**
 * The bus contract. `SignalBus` is the in-process one and `RemoteBus` the same bus reached over a link, so code
 * written against `IPubsub` (components, `SignalActionBridge`, `bridge`, `busApi`) runs unchanged on either side.
 * Topics are `/`-separated levels; filters use `+` for one level and `#` for the rest.
 * QoS 0 is delivered at most once, 1 is retried until handled and dead-lettered after `maxRetries`, 2 is 1 plus
 * a dedupe window on the message id.
 */
export interface IPubsub<TPayload = unknown, TMeta = unknown, TContext = unknown> {
  readonly instanceId: string
  getLogger(): Logger
  setObserver(obs: DynObserver<TPayload, TMeta>): void
  subscribe(
    filter: string,
    handler: DynHandler<TPayload, TMeta, TContext>,
    options?: SubscribeOptions,
  ): DynSubscription
  publish(
    topic: string,
    payload: TPayload,
    options?: PublishOptions<TMeta, TContext>,
  ): Promise<PublishResult>
  req<TReq extends TPayload, TRes>(
    topic: string,
    payload: TReq,
    timeoutMs?: number,
    options?: RequestOptions<TMeta, TContext>,
  ): Promise<TRes>
  reqWithId<TReq extends TPayload, TRes>(
    correlationId: string,
    topic: string,
    payload: TReq,
    timeoutMs?: number,
    options?: RequestOptions<TMeta, TContext>,
  ): Promise<TRes>
  reqStream<TChunk = unknown, TRes = unknown>(
    topic: string,
    payload: TPayload,
    timeoutMs?: number,
    options?: RequestOptions<TMeta, TContext>,
  ): DynStream<TChunk, TRes>
  /** Answers a request from outside its handler, given the request message or its replyTo and correlationId. */
  reply(
    to: { replyTo?: string; correlationId?: string },
    payload: unknown,
    options?: { meta?: TMeta; more?: boolean; error?: DynReplyError },
  ): Promise<void>
  cancelRequest(correlationId: string): void
  /**
   * Called when a request made on this bus is given up on (timeout or cancel). For responders that
   * answer after their handler has returned and so have no DeliveryContext.signal to watch.
   * Returns the function that removes the listener.
   */
  onAbandon?(listener: (correlationId: string, reason: unknown) => void): () => void
  readonly queueLength: number
  getRetained<T = TPayload>(topic: string): T | undefined
  getRetainedMessage<T = TPayload>(topic: string): DynMessage<T, TMeta> | undefined
  retained(filter: string): ReadonlyArray<DynMessage<TPayload, TMeta>>
  dlqSize(): number
  peekDlq(count?: number): ReadonlyArray<DeadLetterEntry<TPayload, TMeta>>
  drainDlq(count?: number): DeadLetterEntry<TPayload, TMeta>[]
  clearRetained(): void
  clearAll(): void
  destroy(): void
  readonly isDestroyed: boolean
  getRateLimitStats():
    | {
        publish?: RateLimiterStats
        subscribe?: RateLimiterStats
        request?: RateLimiterStats
      }
    | undefined
}
