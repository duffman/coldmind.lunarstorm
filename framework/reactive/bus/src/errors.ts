export const DynError = {
  QueueFull: 'queueFull',
  HandlerError: 'handlerError',
  HandlerTimeout: 'handlerTimeout',
  AckTimeout: 'ackTimeout',
  RequestTimeout: 'requestTimeout',
  RequestSendError: 'requestSendError',
  CircuitBreakerOpen: 'circuitBreakerOpen',
  MessageTooLarge: 'messageTooLarge',
  RateLimitExceeded: 'rateLimitExceeded',
  NoResponder: 'noResponder',
  RequestFailed: 'requestFailed',
  RequestCancelled: 'requestCancelled',
} as const

export type TDynError = (typeof DynError)[keyof typeof DynError]

export class DynInvalidTopicError extends Error {
  constructor(
    public readonly topic: string,
    message = 'Invalid publish topic',
  ) {
    super(`${message}: "${topic}"`)
    this.name = 'DynInvalidTopicError'
  }
}

export class DynInvalidFilterError extends Error {
  constructor(
    public readonly filter: string,
    message = 'Invalid subscription filter',
  ) {
    super(`${message}: "${filter}"`)
    this.name = 'DynInvalidFilterError'
  }
}

export class DynHandlerError extends Error {
  constructor(
    public override readonly cause: unknown,
    message = 'Handler failed',
  ) {
    super(message)
    this.name = 'DynHandlerError'
  }
}

export class DynAckTimeoutError extends Error {
  constructor(public readonly timeoutMs: number) {
    super(`Ack timeout reached (${timeoutMs}ms)`)
    this.name = 'DynAckTimeoutError'
  }
}

export class DynHandlerTimeoutError extends Error {
  constructor(
    public readonly timeoutMs: number,
    public readonly topic: string,
  ) {
    super(`Handler timeout (${timeoutMs}ms) for topic: ${topic}`)
    this.name = 'DynHandlerTimeoutError'
  }
}

export class DynRequestTimeoutError extends Error {
  constructor(
    public readonly timeoutMs: number,
    public readonly topic: string,
    public readonly correlationId: string,
  ) {
    super(`Request timeout (${timeoutMs}ms) for topic: ${topic}`)
    this.name = 'DynRequestTimeoutError'
  }
}

/** No subscription matches the request topic, so nobody could answer. */
export class DynNoResponderError extends Error {
  constructor(public readonly topic: string) {
    super(`No responder for topic: ${topic}`)
    this.name = 'DynNoResponderError'
  }
}

/** The responder answered the request with a failure (ctx.replyError, or a handler that threw). */
export class DynRequestError extends Error {
  constructor(
    public readonly code: string,
    message: string,
    public readonly data: unknown,
    public readonly topic: string,
    public readonly correlationId: string,
  ) {
    super(message)
    this.name = 'DynRequestError'
  }
}

export class DynRequestCancelledError extends Error {
  constructor(
    public readonly topic: string,
    public readonly correlationId: string,
  ) {
    super(`Request cancelled for topic: ${topic}`)
    this.name = 'DynRequestCancelledError'
  }
}

export class DynQueueFullError extends Error {
  constructor(public readonly maxSize: number) {
    super(`Queue full: ${maxSize}`)
    this.name = 'DynQueueFullError'
  }
}

export class DynQueueClearedError extends Error {
  constructor(message = 'SignalBus delivery queue was cleared') {
    super(message)
    this.name = 'DynQueueClearedError'
  }
}

export class DynDuplicateCorrelationIdError extends Error {
  constructor(public readonly correlationId: string) {
    super(`A request with correlation ID "${correlationId}" is already pending`)
    this.name = 'DynDuplicateCorrelationIdError'
  }
}

export class DynDestroyedError extends Error {
  constructor() {
    super('SignalBus is destroyed')
    this.name = 'DynDestroyedError'
  }
}

export class DynMessageTooLargeError extends Error {
  constructor(
    public readonly size: number,
    public readonly maxSize: number,
  ) {
    super(`Message size (${size} bytes) exceeds limit (${maxSize} bytes)`)
    this.name = 'DynMessageTooLargeError'
  }
}

export class DynRateLimitExceededError extends Error {
  constructor(public readonly operation: string) {
    super(`Rate limit exceeded for ${operation}`)
    this.name = 'DynRateLimitExceededError'
  }
}

export class DynCircuitBreakerOpenError extends Error {
  constructor(
    public readonly subId: string,
    public readonly filter: string,
  ) {
    super(`Circuit breaker is OPEN for subscription ${subId} (${filter})`)
    this.name = 'DynCircuitBreakerOpenError'
  }
}
