import type { RateLimiterStats } from '@coldmind/bus/src/types.ts'

export class RateLimiter {
  private tokens: number
  private lastRefill: number
  private allowed = 0
  private rejected = 0

  constructor(
    private readonly maxTokensPerSecond: number,
    private readonly burstCapacity: number = maxTokensPerSecond,
  ) {
    this.tokens = burstCapacity
    this.lastRefill = Date.now()
  }

  tryAcquire(): boolean {
    this.refillTokens()
    if (this.tokens >= 1) {
      this.tokens -= 1
      this.allowed++
      return true
    }
    this.rejected++
    return false
  }

  private refillTokens(): void {
    const now = Date.now()
    const timePassed = (now - this.lastRefill) / 1000
    const tokensToAdd = timePassed * this.maxTokensPerSecond
    if (tokensToAdd > 0) {
      this.tokens = Math.min(this.burstCapacity, this.tokens + tokensToAdd)
      this.lastRefill = now
    }
  }

  getStats(): RateLimiterStats {
    this.refillTokens()
    return {
      allowed: this.allowed,
      rejected: this.rejected,
      currentTokens: Math.floor(this.tokens),
    }
  }

  /** True once the bucket has refilled completely: the limiter then behaves exactly like a new one. */
  isFull(): boolean {
    this.refillTokens()
    return this.tokens >= this.burstCapacity
  }

  reset(): void {
    this.tokens = this.burstCapacity
    this.lastRefill = Date.now()
    this.allowed = 0
    this.rejected = 0
  }
}

const SWEEP_EVERY = 1024

export class PerTopicRateLimiter {
  // Kept in least-recently-used order: a topic moves to the end each time it is used.
  private limiters = new Map<string, RateLimiter>()
  private evictedAllowed = 0
  private evictedRejected = 0
  private sinceSweep = 0

  constructor(
    private readonly maxTokensPerSecond: number,
    private readonly burstCapacity: number = maxTokensPerSecond,
    private readonly maxTopics: number = 10_000,
  ) {}

  tryAcquire(topic?: string): boolean {
    let limiter = this.limiters.get(topic!)
    if (limiter) {
      this.limiters.delete(topic!)
    } else {
      limiter = new RateLimiter(this.maxTokensPerSecond, this.burstCapacity)
    }
    this.limiters.set(topic!, limiter)
    const allowed = limiter.tryAcquire()

    // Every topic in use and still over the cap: let go of the least recently used one.
    if (this.limiters.size > this.maxTopics) {
      const oldest = this.limiters.entries().next().value
      if (oldest) this.evict(oldest[0], oldest[1])
    }
    if (++this.sinceSweep >= SWEEP_EVERY) this.sweep()
    return allowed
  }

  /** Number of topics currently tracked. */
  get size(): number {
    return this.limiters.size
  }

  /** Totals across all topics, including those no longer tracked. */
  totals(): RateLimiterStats {
    let allowed = this.evictedAllowed
    let rejected = this.evictedRejected
    let currentTokens = 0
    for (const limiter of this.limiters.values()) {
      const s = limiter.getStats()
      allowed += s.allowed
      rejected += s.rejected
      currentTokens += s.currentTokens
    }
    return { allowed, rejected, currentTokens }
  }

  // A limiter whose bucket has refilled behaves like a new one, so dropping it changes nothing
  // for that topic. Topics that have gone quiet therefore cost no memory.
  private sweep(): void {
    this.sinceSweep = 0
    for (const [topic, limiter] of this.limiters) {
      if (limiter.isFull()) this.evict(topic, limiter)
    }
  }

  private evict(topic: string, limiter: RateLimiter): void {
    const s = limiter.getStats()
    this.evictedAllowed += s.allowed
    this.evictedRejected += s.rejected
    this.limiters.delete(topic)
  }

  getStats(topic?: string): Map<string, RateLimiterStats> | RateLimiterStats | undefined {
    if (topic) {
      return this.limiters.get(topic)?.getStats()
    }
    const stats = new Map<string, RateLimiterStats>()
    for (const [t, limiter] of this.limiters.entries()) {
      stats.set(t, limiter.getStats())
    }
    return stats
  }

  reset(topic?: string): void {
    if (topic) {
      this.limiters.get(topic)?.reset()
    } else {
      this.limiters.clear()
      this.evictedAllowed = 0
      this.evictedRejected = 0
    }
  }
}
