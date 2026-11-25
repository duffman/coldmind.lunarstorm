import {
  DynDuplicateCorrelationIdError,
  DynError,
  DynRequestCancelledError,
  DynRequestError,
  DynRequestTimeoutError,
} from '@coldmind/bus/src/errors.ts'
import type { DynMessage, DynObserver } from '@coldmind/bus/src/types.ts'

interface PendingRequest<TRes> {
  resolve: (value: TRes) => void
  reject: (error: Error) => void
  timeout: ReturnType<typeof setTimeout>
  topic: string
  timeoutMs: number
  onChunk?: (chunk: unknown) => void
}

export class RequestManager<TPayload, TMeta> {
  private readonly pending = new Map<string, PendingRequest<unknown>>()
  private destroyed = false
  private onAbandon?: (correlationId: string, reason: unknown) => void

  constructor(private observer?: DynObserver<TPayload, TMeta>) {}

  /** Called when a request is given up on (timeout, cancel), so its responder can be told to stop. */
  setAbandonHandler(fn: (correlationId: string, reason: unknown) => void): void {
    this.onAbandon = fn
  }

  setObserver(obs: DynObserver<TPayload, TMeta>): void {
    this.observer = obs
  }

  createRequest<TRes>(
    correlationId: string,
    topic: string,
    timeoutMs: number,
    onChunk?: (chunk: unknown) => void,
  ): Promise<TRes> {
    if (this.destroyed) throw new Error('RequestManager destroyed')
    if (this.pending.has(correlationId)) {
      throw new DynDuplicateCorrelationIdError(correlationId)
    }

    return new Promise<TRes>((resolve, reject) => {
      this.pending.set(correlationId, {
        resolve: (value: unknown) => resolve(value as TRes),
        reject,
        timeout: this.arm(correlationId, topic, timeoutMs),
        topic,
        timeoutMs,
        onChunk,
      })
    })
  }

  private arm(correlationId: string, topic: string, timeoutMs: number): ReturnType<typeof setTimeout> {
    return setTimeout(() => {
      const pending = this.pending.get(correlationId)
      if (!pending) return
      this.pending.delete(correlationId)

      const err = new DynRequestTimeoutError(timeoutMs, topic, correlationId)
      this.observer?.onError?.({
        type: DynError.RequestTimeout,
        topic,
        messageId: correlationId,
        error: err,
        correlationId,
        meta: { timeoutMs },
      })
      this.onAbandon?.(correlationId, err)
      pending.reject(err)
    }, timeoutMs)
  }

  handleReply(msg: DynMessage<TPayload, TMeta>): boolean {
    if (this.destroyed) return false
    if (!msg.correlationId) return false

    const pending = this.pending.get(msg.correlationId)
    if (!pending) return false

    clearTimeout(pending.timeout)

    // One of several replies: hand it over and keep waiting. Each chunk restarts the timeout.
    if (msg.more) {
      pending.timeout = this.arm(msg.correlationId, pending.topic, pending.timeoutMs)
      try {
        pending.onChunk?.(msg.payload)
      } catch (error) {
        this.observer?.onError?.({
          type: DynError.HandlerError,
          topic: pending.topic,
          messageId: msg.id,
          error,
          correlationId: msg.correlationId,
        })
      }
      return true
    }

    this.pending.delete(msg.correlationId)

    if (msg.error) {
      const err = new DynRequestError(
        msg.error.code,
        msg.error.message,
        msg.error.data,
        pending.topic,
        msg.correlationId,
      )
      this.observer?.onError?.({
        type: DynError.RequestFailed,
        topic: pending.topic,
        messageId: msg.id,
        error: err,
        correlationId: msg.correlationId,
      })
      pending.reject(err)
      return true
    }

    pending.resolve(msg.payload)
    return true
  }

  /** Gives up on a pending request. Returns false when there is none with that id. */
  cancel(correlationId: string): boolean {
    const pending = this.pending.get(correlationId)
    if (!pending) return false

    clearTimeout(pending.timeout)
    this.pending.delete(correlationId)

    const err = new DynRequestCancelledError(pending.topic, correlationId)
    this.observer?.onError?.({
      type: DynError.RequestCancelled,
      topic: pending.topic,
      messageId: correlationId,
      error: err,
      correlationId,
    })
    this.onAbandon?.(correlationId, err)
    pending.reject(err)
    return true
  }

  handlePublishError(correlationId: string, topic: string, err: unknown): void {
    const pending = this.pending.get(correlationId)
    if (!pending) return

    clearTimeout(pending.timeout)
    this.pending.delete(correlationId)

    const errorObj = err instanceof Error ? err : new Error(String(err))
    this.observer?.onError?.({
      type: DynError.RequestSendError,
      topic,
      messageId: correlationId,
      error: errorObj,
      correlationId,
    })
    pending.reject(errorObj)
  }

  clear(): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout)
      pending.reject(new Error('Router cleared'))
    }
    this.pending.clear()
  }

  destroy(): void {
    if (this.destroyed) return
    this.destroyed = true

    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout)
      pending.reject(new Error('RequestManager destroyed'))
    }
    this.pending.clear()
  }

  get pendingCount(): number {
    return this.pending.size
  }

  get isDestroyed(): boolean {
    return this.destroyed
  }
}
