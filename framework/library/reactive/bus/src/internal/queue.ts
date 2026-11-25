import {
  DynAckTimeoutError,
  DynCircuitBreakerOpenError,
  DynDestroyedError,
  DynError,
  DynHandlerError,
  DynHandlerTimeoutError,
  DynQueueClearedError,
  DynQueueFullError,
} from '@coldmind/bus/src/errors.ts'
import type {
  CircuitBreakerOptions,
  DeadLetterEntry,
  DeadLetterOptions,
  DeliveryContext,
  DispatchMode,
  DynMessage,
  DynObserver,
  DynReplyError,
  InternalSubscription,
  QueuedDelivery,
  ReplyOptions,
} from '@coldmind/bus/src/types.ts'
import { CircuitBreaker } from '@coldmind/bus/src/internal/breaker.ts'
import type { CircuitBreakerMetrics, CircuitBreakerState } from '@coldmind/bus/src/internal/breaker.ts'
import { PriorityHeap } from '@coldmind/bus/src/internal/heap.ts'

interface QueueOptions {
  dispatch: DispatchMode
  stopOnError: boolean
  ackTimeoutMs: number
  maxRetries: number
  retryDelayMs: number
  retryBackoffFactor: number
  requireExplicitAck: boolean
  defaultHandlerTimeoutMs: number
  maxQueueSize: number
  deadLetter: Required<DeadLetterOptions>
  circuitBreaker: Required<CircuitBreakerOptions>
}

interface Deferred<T> {
  promise: Promise<T>
  resolve: (v: T) => void
  reject: (e: unknown) => void
}

function deferred<T>(): Deferred<T> {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}

/**
 * The deliveries waiting for one subscription. In parallel dispatch every subscription
 * drains its own lane, so a slow handler holds back only its own subscription.
 */
interface Lane<TPayload, TMeta, TContext> {
  waiting: QueuedDelivery<TPayload, TMeta, TContext>[]
  head: number
  running: number
  concurrency: number
}

/** What one delivery has told the requester so far. Shared across its retry attempts. */
interface ReplyState {
  final: boolean
}

function toReplyError(error: DynReplyError | Error | unknown, fallbackCode: string): DynReplyError {
  if (error instanceof Error) {
    const code = (error as { code?: unknown }).code
    const data = (error as { data?: unknown }).data
    return {
      code: typeof code === 'string' ? code : fallbackCode,
      message: error.message,
      ...(data !== undefined ? { data } : {}),
    }
  }
  if (typeof error === 'object' && error !== null && typeof (error as DynReplyError).message === 'string') {
    const e = error as DynReplyError
    return {
      code: typeof e.code === 'string' ? e.code : fallbackCode,
      message: e.message,
      ...(e.data !== undefined ? { data: e.data } : {}),
    }
  }
  return { code: fallbackCode, message: String(error) }
}

export class QueueProcessor<TPayload, TMeta, TContext> {
  private queue: PriorityHeap<QueuedDelivery<TPayload, TMeta, TContext>>
  private readonly dlq: DeadLetterEntry<TPayload, TMeta>[] = []
  private processingLock: Promise<void> | null = null
  private destroyed = false
  private deliverySeq = 0
  private readonly circuitBreakers = new Map<string, CircuitBreaker>()
  private readonly lanes = new Map<string, Lane<TPayload, TMeta, TContext>>()
  private laneBacklog = 0
  private laneDispatchScheduled = false
  private readonly activeDeliveries = new Set<AbortController>()
  private readonly requestDeliveries = new Map<string, Set<AbortController>>()

  constructor(
    private readonly opts: QueueOptions,
    private observer?: DynObserver<TPayload, TMeta>,
    private readonly replyFn?: (
      topic: string,
      payload: unknown,
      correlationId: string,
      meta?: TMeta,
      flags?: { more?: boolean; error?: DynReplyError },
    ) => Promise<void>,
  ) {
    this.queue = new PriorityHeap<QueuedDelivery<TPayload, TMeta, TContext>>(
      (a, b) => a.priority - b.priority || (b.seq ?? 0) - (a.seq ?? 0),
    )
  }

  setObserver(obs: DynObserver<TPayload, TMeta>): void {
    this.observer = obs
  }

  get queueLength(): number {
    return this.queue.size() + this.laneBacklog
  }

  get dlqSize(): number {
    return this.dlq.length
  }

  enqueue(items: QueuedDelivery<TPayload, TMeta, TContext>[]): Promise<void> {
    if (this.queueLength + items.length > this.opts.maxQueueSize) {
      throw new DynQueueFullError(this.opts.maxQueueSize)
    }

    const completion = new Promise<void>((resolve, reject) => {
      let count = items.length
      let settled = false
      if (count === 0) return resolve()

      const onDone = () => {
        if (settled) return
        if (--count === 0) {
          settled = true
          resolve()
        }
      }

      const onError = (error: unknown) => {
        if (settled) return
        settled = true
        reject(error)
      }

      for (const item of items) {
        item.resolve = onDone
        item.reject = onError
      }
    })

    for (const item of items) item.seq = ++this.deliverySeq
    this.queue.pushMany(items)

    if (this.opts.dispatch === 'parallel') {
      // Parallel dispatch: hand the work to per-subscription lanes instead of one shared batch.
      if (!this.laneDispatchScheduled) {
        this.laneDispatchScheduled = true
        queueMicrotask(() => this.dispatchToLanes())
      }
      return completion
    }

    if (!this.processingLock) {
      let releaseLock!: () => void
      this.processingLock = new Promise<void>((resolve) => {
        releaseLock = resolve
      })
      queueMicrotask(() => this.processQueue(releaseLock))
    }

    return completion
  }

  async runImmediate(
    subs: InternalSubscription<TPayload, TMeta, TContext>[],
    msg: DynMessage<TPayload, TMeta>,
    handlerTimeoutMs?: number,
    context?: TContext,
  ): Promise<void> {
    const items: QueuedDelivery<TPayload, TMeta, TContext>[] = [...subs]
      .sort((a, b) => b.priority - a.priority)
      .map((sub) => ({
        sub,
        message: msg,
        priority: sub.priority,
        handlerTimeoutMs,
        context,
        fanout: subs.length,
      }))

    if (this.opts.dispatch === 'sequential') {
      for (const item of items) {
        try {
          await this.deliverWithRetry(item)
        } catch (error) {
          if (this.opts.stopOnError) throw error
        }
      }
      return
    }

    // Starting work in sorted order preserves priority without serializing
    // otherwise-independent handlers in parallel dispatch mode.
    const results = await Promise.allSettled(items.map((item) => this.deliverWithRetry(item)))
    if (this.opts.stopOnError) {
      const failure = results.find((result) => result.status === 'rejected')
      if (failure?.status === 'rejected') throw failure.reason
    }
  }

  peekDlq(count = 50): ReadonlyArray<DeadLetterEntry<TPayload, TMeta>> {
    return this.dlq.slice(0, Math.max(0, count))
  }

  drainDlq(count = 50): DeadLetterEntry<TPayload, TMeta>[] {
    return this.dlq.splice(0, Math.max(0, count))
  }

  clear(reason: unknown = new DynQueueClearedError()): void {
    const pending = this.queue.popMany(this.queue.size())
    for (const item of pending) item.reject?.(reason)
    for (const [subId, lane] of this.lanes) {
      for (let i = lane.head; i < lane.waiting.length; i++) lane.waiting[i]!.reject?.(reason)
      lane.waiting = []
      lane.head = 0
      if (lane.running === 0) this.lanes.delete(subId)
    }
    this.laneBacklog = 0
    this.dlq.length = 0
    for (const cb of this.circuitBreakers.values()) cb.destroy()
    this.circuitBreakers.clear()
  }

  destroy(): void {
    this.destroyed = true
    const reason = new DynDestroyedError()
    this.clear(reason)
    for (const controller of this.activeDeliveries) controller.abort(reason)
    this.activeDeliveries.clear()
    this.requestDeliveries.clear()
  }

  /** Aborts the signal of every delivery in progress that answers this request. */
  abortDeliveries(correlationId: string, reason: unknown): void {
    const controllers = this.requestDeliveries.get(correlationId)
    if (!controllers) return
    for (const controller of [...controllers]) controller.abort(reason)
  }

  // Moves everything staged in the heap to its subscription's lane and starts the lanes.
  // The heap hands items over by priority, then enqueue order, so handlers still start in that order.
  private dispatchToLanes(): void {
    this.laneDispatchScheduled = false
    if (this.destroyed) return

    const touched = new Map<string, Lane<TPayload, TMeta, TContext>>()
    for (const item of this.queue.popMany(this.queue.size())) {
      let lane = this.lanes.get(item.sub.id)
      if (!lane) {
        const limit = item.sub.concurrency
        lane = { waiting: [], head: 0, running: 0, concurrency: limit >= 1 ? limit : 1 }
        this.lanes.set(item.sub.id, lane)
      }
      lane.waiting.push(item)
      this.laneBacklog++
      touched.set(item.sub.id, lane)
    }

    for (const [subId, lane] of touched) this.pumpLane(subId, lane)
  }

  private pumpLane(subId: string, lane: Lane<TPayload, TMeta, TContext>): void {
    while (!this.destroyed && lane.running < lane.concurrency && lane.head < lane.waiting.length) {
      const item = lane.waiting[lane.head++]!
      this.laneBacklog--
      if (lane.head > 64 && lane.head * 2 > lane.waiting.length) {
        lane.waiting = lane.waiting.slice(lane.head)
        lane.head = 0
      }

      lane.running++
      // A failure with stopOnError has already been passed to item.reject by deliverWithRetry.
      this.deliverWithRetry(item)
        .catch(() => {})
        .finally(() => {
          lane.running--
          if (lane.running === 0 && lane.head >= lane.waiting.length) {
            if (this.lanes.get(subId) === lane) this.lanes.delete(subId)
          } else {
            this.pumpLane(subId, lane)
          }
        })
    }
  }

  removeSubscription(subId: string): void {
    const cb = this.circuitBreakers.get(subId)
    cb?.destroy()
    this.circuitBreakers.delete(subId)
  }

  getCircuitBreakerState(subId: string): CircuitBreakerState {
    const cb = this.circuitBreakers.get(subId)
    return cb ? cb.getState() : 'CLOSED'
  }

  getCircuitBreakerMetrics(subId: string): CircuitBreakerMetrics {
    const cb = this.getOrCreateCircuitBreaker(subId)
    return cb.getMetrics()
  }

  private getOrCreateCircuitBreaker(subId: string): CircuitBreaker {
    let cb = this.circuitBreakers.get(subId)
    if (!cb) {
      cb = new CircuitBreaker(subId, this.opts.circuitBreaker)
      this.circuitBreakers.set(subId, cb)
    }
    return cb
  }

  private async processQueue(releaseLock: () => void): Promise<void> {
    try {
      while (!this.queue.isEmpty() && !this.destroyed) {
        const batch = this.queue.popMany(100)

        if (this.opts.dispatch === 'sequential') {
          for (let index = 0; index < batch.length; index++) {
            const item = batch[index]
            try {
              await this.deliverWithRetry(item!)
            } catch (err) {
              if (this.opts.stopOnError) {
                for (const skipped of batch.slice(index + 1)) skipped.reject?.(err)
                break
              }
            }
          }
        } else {
          await Promise.allSettled(
            batch.map((item) =>
              this.deliverWithRetry(item).catch((err) => {
                if (this.opts.stopOnError) throw err
              }),
            ),
          )
        }
      }
    } finally {
      this.processingLock = null
      releaseLock()
    }
  }

  private async deliverWithRetry(item: QueuedDelivery<TPayload, TMeta, TContext>): Promise<void> {
    const { sub, message, handlerTimeoutMs, context } = item
    const needsAck = message.qos > 0
    const maxAttempts = needsAck ? 1 + this.opts.maxRetries : 1
    const replyState: ReplyState = { final: false }

    try {
      const circuitBreaker = this.opts.circuitBreaker.enabled
        ? this.getOrCreateCircuitBreaker(sub.id)
        : undefined
      if (circuitBreaker && !circuitBreaker.allowRequest()) {
        const cbError = new DynCircuitBreakerOpenError(sub.id, sub.filter)

        this.observer?.onError?.({
          type: DynError.CircuitBreakerOpen,
          topic: message.topic,
          messageId: message.id,
          error: cbError,
          subId: sub.id,
          filter: sub.filter,
          attempt: 1,
        })

        if (this.opts.deadLetter.enabled) {
          this.enqueueDlq({
            enqueuedAt: Date.now(),
            attempts: 1,
            subId: sub.id,
            filter: sub.filter,
            message,
            error: cbError,
          })
        }

        await this.replyFailure(item, replyState, cbError, DynError.CircuitBreakerOpen)

        if (this.opts.stopOnError) {
          item.reject?.(cbError)
          throw cbError
        }
        return
      }

      let lastErr: unknown

      for (let attempt = 1; attempt <= maxAttempts; attempt++) {
        if (this.destroyed) return

        try {
          await this.runSingleDelivery(sub, message, handlerTimeoutMs, context, attempt, replyState)
          circuitBreaker?.recordSuccess()
          return
        } catch (err) {
          lastErr = err

          this.observer?.onError?.({
            type:
              err instanceof DynHandlerTimeoutError
                ? DynError.HandlerTimeout
                : DynError.HandlerError,
            topic: message.topic,
            messageId: message.id,
            error: err,
            subId: sub.id,
            filter: sub.filter,
            attempt,
          })

          if (attempt >= maxAttempts) {
            circuitBreaker?.recordFailure()

            await this.replyFailure(
              item,
              replyState,
              err instanceof DynHandlerError && err.cause !== undefined ? err.cause : err,
              err instanceof DynHandlerTimeoutError ? DynError.HandlerTimeout : DynError.HandlerError,
            )

            if (this.opts.deadLetter.enabled) {
              this.enqueueDlq({
                enqueuedAt: Date.now(),
                attempts: attempt,
                subId: sub.id,
                filter: sub.filter,
                message,
                error: err,
              })
            }
            if (this.opts.stopOnError) {
              item.reject?.(err)
              throw err
            }
            return
          }

          await sleep(this.computeRetryDelay(attempt))
        }
      }

      if (lastErr) throw lastErr
    } finally {
      item.resolve?.()
    }
  }

  // The handler failed for good and the requester has not been answered: tell it why, so it does
  // not have to wait for its timeout. Only when this subscription is the request's sole recipient;
  // with several, one of the others may still answer.
  private async replyFailure(
    item: QueuedDelivery<TPayload, TMeta, TContext>,
    replyState: ReplyState,
    error: unknown,
    code: string,
  ): Promise<void> {
    const msg = item.message
    if (replyState.final || !msg.replyTo || !msg.correlationId || !this.replyFn) return
    if (item.fanout !== undefined && item.fanout > 1) return
    replyState.final = true
    try {
      await this.replyFn(msg.replyTo, undefined, msg.correlationId, undefined, {
        error: toReplyError(error, code),
      })
    } catch {
      // The requester is gone or unreachable; it will see its own timeout.
    }
  }

  private async runSingleDelivery(
    sub: InternalSubscription<TPayload, TMeta, TContext>,
    msg: DynMessage<TPayload, TMeta>,
    handlerTimeoutMs?: number,
    context?: TContext,
    attempt = 1,
    replyState: ReplyState = { final: false },
  ): Promise<void> {
    const controller = new AbortController()
    const answersRequest = msg.replyTo !== undefined && msg.correlationId !== undefined
    this.activeDeliveries.add(controller)
    if (answersRequest) {
      const set = this.requestDeliveries.get(msg.correlationId!) ?? new Set<AbortController>()
      set.add(controller)
      this.requestDeliveries.set(msg.correlationId!, set)
    }

    try {
      await this.runDelivery(sub, msg, handlerTimeoutMs, context, attempt, replyState, controller)
    } finally {
      this.activeDeliveries.delete(controller)
      if (answersRequest) {
        const set = this.requestDeliveries.get(msg.correlationId!)
        set?.delete(controller)
        if (set?.size === 0) this.requestDeliveries.delete(msg.correlationId!)
      }
    }
  }

  private async runDelivery(
    sub: InternalSubscription<TPayload, TMeta, TContext>,
    msg: DynMessage<TPayload, TMeta>,
    handlerTimeoutMs: number | undefined,
    context: TContext | undefined,
    attempt: number,
    replyState: ReplyState,
    controller: AbortController,
  ): Promise<void> {
    const start = Date.now()
    const needsAck = msg.qos > 0
    const requireExplicit = needsAck && this.opts.requireExplicitAck

    const ackDef = deferred<void>()
    let settled = false

    const ack = (): void => {
      if (settled) return
      settled = true
      ackDef.resolve(undefined)
    }

    const nack = (error?: unknown): void => {
      if (settled) return
      settled = true
      ackDef.reject(error ?? new DynHandlerError(undefined, 'nack() called'))
    }

    const send = async (
      payload: unknown,
      meta: TMeta | undefined,
      flags: { more?: boolean; error?: DynReplyError },
    ): Promise<void> => {
      if (!msg.replyTo || !msg.correlationId || !this.replyFn) return
      if (!flags.more) replyState.final = true
      try {
        await this.replyFn(msg.replyTo, payload, msg.correlationId, meta, flags)
      } catch (error) {
        this.observer?.onError?.({
          type: DynError.HandlerError,
          topic: msg.replyTo,
          messageId: msg.id,
          error,
          subId: sub.id,
          filter: sub.filter,
          attempt,
          correlationId: msg.correlationId,
          meta: meta ? (meta as Record<string, unknown>) : undefined,
        })
        throw error
      }
    }

    const reply = <T>(payload: T, meta?: TMeta, options?: ReplyOptions): Promise<void> =>
      send(payload, meta, options?.more ? { more: true } : {})

    const replyError = (error: DynReplyError | Error, meta?: TMeta): Promise<void> =>
      send(undefined, meta, { error: toReplyError(error, DynError.RequestFailed) })

    const ctx: DeliveryContext<TPayload, TMeta, TContext> = {
      subId: sub.id,
      filter: sub.filter,
      attempt,
      priority: sub.priority,
      context,
      signal: controller.signal,
      ack,
      nack,
      reply,
      replyError,
    }

    let handlerPromise: Promise<void>
    try {
      handlerPromise = Promise.resolve(sub.handler(msg, ctx))
    } catch (e) {
      handlerPromise = Promise.reject(e)
    }

    const effectiveTimeout = handlerTimeoutMs ?? this.opts.defaultHandlerTimeoutMs
    if (effectiveTimeout > 0) {
      let timeoutId: ReturnType<typeof setTimeout>
      const timeoutPromise = new Promise<never>((_, reject) => {
        timeoutId = setTimeout(() => {
          const timeoutError = new DynHandlerTimeoutError(effectiveTimeout, msg.topic)
          controller.abort(timeoutError)
          reject(timeoutError)
        }, effectiveTimeout)
      })

      handlerPromise = Promise.race([
        handlerPromise.finally(() => clearTimeout(timeoutId)),
        timeoutPromise,
      ])
    }

    if (!needsAck) {
      await handlerPromise
      this.observer?.onDelivered?.({
        topic: msg.topic,
        latency: Date.now() - start,
        messageId: msg.id,
        subId: sub.id,
      })
      return
    }

    void (async () => {
      try {
        await handlerPromise
        if (!requireExplicit) ack()
      } catch (e) {
        nack(new DynHandlerError(e))
      }
    })()

    let t: ReturnType<typeof setTimeout> | undefined
    const timeoutP = new Promise<void>((_, rej) => {
      t = setTimeout(
        () => rej(new DynAckTimeoutError(this.opts.ackTimeoutMs)),
        this.opts.ackTimeoutMs,
      )
    })

    try {
      await Promise.race([ackDef.promise, timeoutP])
      this.observer?.onDelivered?.({
        topic: msg.topic,
        latency: Date.now() - start,
        messageId: msg.id,
        subId: sub.id,
      })
    } finally {
      if (t) clearTimeout(t)
    }
  }

  private computeRetryDelay(attempt: number): number {
    const exp = Math.max(0, attempt - 1)
    const MAX_BACKOFF_MS = 3600000
    const maxSafeExp =
      Math.log(MAX_BACKOFF_MS / this.opts.retryDelayMs) / Math.log(this.opts.retryBackoffFactor)
    const cappedExp = Math.min(exp, maxSafeExp)

    const base = this.opts.retryDelayMs * Math.pow(this.opts.retryBackoffFactor, cappedExp)
    const jitter = base * 0.1 * (Math.random() * 2 - 1)
    const delayWithJitter = base + jitter

    return Math.max(0, Math.floor(Math.min(delayWithJitter, MAX_BACKOFF_MS)))
  }

  private enqueueDlq(entry: DeadLetterEntry<TPayload, TMeta>): void {
    const { maxSize, overflow } = this.opts.deadLetter

    if (this.dlq.length >= maxSize) {
      if (overflow === 'drop-newest') return
      this.dlq.shift()
    }

    this.dlq.push(entry)
    this.observer?.onDeadLetter?.(entry)
  }
}
