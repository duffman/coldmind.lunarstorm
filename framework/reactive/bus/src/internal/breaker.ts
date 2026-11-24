export type CircuitBreakerState = 'CLOSED' | 'OPEN' | 'HALF_OPEN'

export interface CircuitBreakerMetrics {
  state: CircuitBreakerState
  failureCount: number
  successCount: number
  lastFailureTime?: number
  lastStateChange: number
}

export interface CircuitBreakerConfig {
  enabled: boolean
  failureThreshold: number
  successThreshold: number
  timeout: number
  halfOpenMaxAttempts: number
}

export class CircuitBreaker {
  private state: CircuitBreakerState = 'CLOSED'
  private failureCount = 0
  private successCount = 0
  private lastFailureTime?: number
  private lastStateChange = Date.now()
  private stateTransitionTimer?: ReturnType<typeof setTimeout>
  private halfOpenAttempts = 0

  constructor(
    private readonly subId: string,
    private readonly options: CircuitBreakerConfig,
  ) {}

  getState(): CircuitBreakerState {
    if (this.state === 'OPEN' && this.shouldTransitionToHalfOpen()) {
      this.transitionTo('HALF_OPEN')
    }
    return this.state
  }

  allowRequest(): boolean {
    if (!this.options.enabled) return true

    const currentState = this.getState()
    if (currentState === 'CLOSED') return true

    if (currentState === 'HALF_OPEN') {
      if (this.halfOpenAttempts < this.options.halfOpenMaxAttempts) {
        this.halfOpenAttempts++
        return true
      }
      return false
    }
    return false
  }

  recordSuccess(): void {
    if (!this.options.enabled) return

    if (this.state === 'CLOSED') {
      this.failureCount = 0
      this.successCount++
    } else if (this.state === 'HALF_OPEN') {
      this.successCount++
      if (this.successCount >= this.options.successThreshold) {
        this.transitionTo('CLOSED')
      }
    }
  }

  recordFailure(): void {
    if (!this.options.enabled) return

    this.lastFailureTime = Date.now()

    if (this.state === 'CLOSED') {
      this.failureCount++
      if (this.failureCount >= this.options.failureThreshold) {
        this.transitionTo('OPEN')
      }
    } else if (this.state === 'HALF_OPEN') {
      this.transitionTo('OPEN')
    }
  }

  getMetrics(): CircuitBreakerMetrics {
    return {
      state: this.getState(),
      failureCount: this.failureCount,
      successCount: this.successCount,
      lastFailureTime: this.lastFailureTime,
      lastStateChange: this.lastStateChange,
    }
  }

  reset(): void {
    this.clearTimer()
    this.transitionTo('CLOSED')
  }

  destroy(): void {
    this.clearTimer()
  }

  private shouldTransitionToHalfOpen(): boolean {
    if (!this.lastFailureTime) return false
    return Date.now() - this.lastFailureTime >= this.options.timeout
  }

  private transitionTo(newState: CircuitBreakerState): void {
    if (this.state === newState) return

    this.state = newState
    this.lastStateChange = Date.now()

    switch (newState) {
      case 'CLOSED':
        this.failureCount = 0
        this.successCount = 0
        this.halfOpenAttempts = 0
        this.clearTimer()
        break
      case 'OPEN':
        this.successCount = 0
        this.halfOpenAttempts = 0
        this.scheduleHalfOpenTransition()
        break
      case 'HALF_OPEN':
        this.successCount = 0
        this.halfOpenAttempts = 0
        this.clearTimer()
        break
    }
  }

  private scheduleHalfOpenTransition(): void {
    this.clearTimer()
    this.stateTransitionTimer = setTimeout(() => {
      if (this.state === 'OPEN') this.transitionTo('HALF_OPEN')
    }, this.options.timeout)
  }

  private clearTimer(): void {
    if (this.stateTransitionTimer) {
      clearTimeout(this.stateTransitionTimer)
      this.stateTransitionTimer = undefined
    }
  }
}
