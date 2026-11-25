import { describe, expect, test } from 'vitest'
import {
  DynNoResponderError,
  DynQueueFullError,
  DynRequestCancelledError,
  DynRequestError,
  DynRequestTimeoutError,
  SignalBus,
} from '@coldmind/bus'
import { PerTopicRateLimiter } from '@coldmind/bus/src/internal/limiter.ts'

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))
const timed = async <T>(p: Promise<T>): Promise<[T | unknown, number]> => {
  const start = performance.now()
  try {
    return [await p, performance.now() - start]
  } catch (e) {
    return [e, performance.now() - start]
  }
}

describe('request failures reach the requester', () => {
  test('ctx.replyError rejects the request with code, message and data', async () => {
    const bus = new SignalBus()
    bus.subscribe('svc/pay', async (_msg, ctx) => {
      await ctx.replyError({ code: 'declined', message: 'Card declined', data: { retry: false } })
    })

    const [err, ms] = await timed(bus.req('svc/pay', {}, 1000))
    expect(err).toBeInstanceOf(DynRequestError)
    expect((err as DynRequestError).code).toBe('declined')
    expect((err as DynRequestError).message).toBe('Card declined')
    expect((err as DynRequestError).data).toEqual({ retry: false })
    expect(ms).toBeLessThan(200)
    bus.destroy()
  })

  test('a handler that throws answers the request with the error instead of a timeout', async () => {
    const bus = new SignalBus()
    bus.subscribe('svc/fail', () => {
      throw new Error('boom')
    })

    const [err, ms] = await timed(bus.req('svc/fail', {}, 1000))
    expect(err).toBeInstanceOf(DynRequestError)
    expect((err as DynRequestError).code).toBe('handlerError')
    expect((err as DynRequestError).message).toBe('boom')
    expect(ms).toBeLessThan(200)
    bus.destroy()
  })

  test('with several subscribers, one failing does not answer for the others', async () => {
    const bus = new SignalBus()
    bus.subscribe('svc/mixed', () => {
      throw new Error('observer broke')
    })
    bus.subscribe('svc/mixed', async (_msg, ctx) => {
      await sleep(20)
      await ctx.reply('answer')
    })

    expect(await bus.req<unknown, string>('svc/mixed', {}, 1000)).toBe('answer')
    bus.destroy()
  })

  test('a request nobody subscribes to fails at once', async () => {
    const bus = new SignalBus()
    const [err, ms] = await timed(bus.req('svc/none', {}, 1000))
    expect(err).toBeInstanceOf(DynNoResponderError)
    expect(ms).toBeLessThan(100)
    bus.destroy()
  })

  test('reply() answers a request from outside its handler', async () => {
    const bus = new SignalBus<string>()
    bus.subscribe('svc/later', (msg) => {
      setTimeout(() => void bus.reply(msg, `late ${msg.payload}`), 20)
    })
    expect(await bus.req<string, string>('svc/later', 'x', 1000)).toBe('late x')
    bus.destroy()
  })

  test('a reply to a reply topic someone else owns is delivered to its subscribers', async () => {
    const bus = new SignalBus<string>()
    const got: Array<[string, string | undefined]> = []
    bus.subscribe('inbox/me', (msg) => {
      got.push([msg.payload, msg.correlationId])
    })
    bus.subscribe('svc/echo', async (msg, ctx) => {
      await ctx.reply(msg.payload.toUpperCase())
    })
    await bus.publish('svc/echo', 'hi', { replyTo: 'inbox/me', correlationId: 'c-1', delivery: 'immediate' })
    expect(got).toEqual([['HI', 'c-1']])
    bus.destroy()
  })
})

describe('subscriptions do not wait for each other', () => {
  test('a slow subscription does not delay a fast one', async () => {
    const bus = new SignalBus()
    bus.subscribe('svc/slow', async (_msg, ctx) => {
      await sleep(300)
      await ctx.reply('slow')
    })
    bus.subscribe('svc/fast', async (_msg, ctx) => {
      await ctx.reply('fast')
    })

    const slow = timed(bus.req('svc/slow', {}, 2000))
    const together = await timed(bus.req('svc/fast', {}, 2000))
    await sleep(50)
    const later = await timed(bus.req('svc/fast', {}, 2000))

    expect(together[0]).toBe('fast')
    expect(together[1]).toBeLessThan(100)
    expect(later[1]).toBeLessThan(100)
    expect((await slow)[0]).toBe('slow')
    bus.destroy()
  })

  test('one subscription receives its messages in order, one at a time', async () => {
    const bus = new SignalBus<number>()
    const finished: number[] = []
    let running = 0
    let maxRunning = 0
    bus.subscribe('jobs', async (msg) => {
      maxRunning = Math.max(maxRunning, ++running)
      await sleep((6 - msg.payload) * 5)
      finished.push(msg.payload)
      running--
    })

    for (const n of [1, 2, 3, 4, 5]) await bus.publish('jobs', n)
    expect(bus.queueLength).toBeGreaterThan(0)
    await sleep(150)

    expect(finished).toEqual([1, 2, 3, 4, 5])
    expect(maxRunning).toBe(1)
    expect(bus.queueLength).toBe(0)
    bus.destroy()
  })

  test('concurrency lets one subscription work on several messages at once', async () => {
    const bus = new SignalBus<number>()
    let running = 0
    let maxRunning = 0
    let done = 0
    bus.subscribe(
      'jobs',
      async () => {
        maxRunning = Math.max(maxRunning, ++running)
        await sleep(20)
        running--
        done++
      },
      { concurrency: 3 },
    )

    for (let n = 0; n < 9; n++) await bus.publish('jobs', n)
    await sleep(150)

    expect(maxRunning).toBe(3)
    expect(done).toBe(9)
    bus.destroy()
  })

  test('the queue limit counts what is waiting in the lanes', async () => {
    const bus = new SignalBus<number>({ maxQueueSize: 3 })
    bus.subscribe('jobs', () => sleep(100))

    await bus.publish('jobs', 0)
    await sleep(5)
    for (const n of [1, 2, 3]) await bus.publish('jobs', n)
    await expect(bus.publish('jobs', 4)).rejects.toThrow(DynQueueFullError)
    bus.destroy()
  })

  test('replies are not counted against the publish rate limit', async () => {
    const bus = new SignalBus<number>({ rateLimits: { maxPublishPerSecond: 6 } })
    bus.subscribe('svc/echo', async (msg, ctx) => {
      await ctx.reply(msg.payload)
    })

    const results = await Promise.all([1, 2, 3, 4, 5].map((n) => bus.req<number, number>('svc/echo', n, 500)))
    expect(results).toEqual([1, 2, 3, 4, 5])
    expect(bus.getRateLimitStats()?.publish?.allowed).toBe(5)
    bus.destroy()
  })
})

describe('streamed replies', () => {
  const serve = (bus: SignalBus<number>, gapMs = 0) =>
    bus.subscribe('svc/count', async (msg, ctx) => {
      for (let n = 1; n <= msg.payload; n++) {
        if (gapMs) await sleep(gapMs)
        await ctx.reply(n, undefined, { more: true })
      }
      await ctx.reply('done')
    })

  test('req delivers chunks to onChunk and resolves with the final reply', async () => {
    const bus = new SignalBus<number>()
    serve(bus)
    const chunks: unknown[] = []
    const result = await bus.req<number, string>('svc/count', 3, 1000, { onChunk: (c) => chunks.push(c) })
    expect(chunks).toEqual([1, 2, 3])
    expect(result).toBe('done')
    bus.destroy()
  })

  test('reqStream is iterable and carries the final reply in result', async () => {
    const bus = new SignalBus<number>()
    serve(bus, 5)
    const stream = bus.reqStream<number, string>('svc/count', 4, 1000)
    const chunks: number[] = []
    for await (const chunk of stream) chunks.push(chunk)
    expect(chunks).toEqual([1, 2, 3, 4])
    expect(await stream.result).toBe('done')
    bus.destroy()
  })

  test('every chunk restarts the request timeout', async () => {
    const bus = new SignalBus<number>()
    serve(bus, 30)
    // 5 chunks at 30 ms apart take longer than the 80 ms timeout in total.
    expect(await bus.req<number, string>('svc/count', 5, 80, { onChunk: () => {} })).toBe('done')
    bus.destroy()
  })

  test('a failure in the middle of a stream ends it with the error', async () => {
    const bus = new SignalBus<number>()
    bus.subscribe('svc/broken', async (_msg, ctx) => {
      await ctx.reply(1, undefined, { more: true })
      throw new Error('ran out')
    })
    const stream = bus.reqStream<number>('svc/broken', 0, 1000)
    const chunks: number[] = []
    let caught: unknown
    try {
      for await (const chunk of stream) chunks.push(chunk)
    } catch (e) {
      caught = e
    }
    expect(chunks).toEqual([1])
    expect(caught).toBeInstanceOf(DynRequestError)
    expect((caught as DynRequestError).message).toBe('ran out')
    bus.destroy()
  })

  test('leaving the loop early cancels the request', async () => {
    const bus = new SignalBus<number>()
    let aborted = false
    bus.subscribe('svc/endless', async (_msg, ctx) => {
      ctx.signal.addEventListener('abort', () => (aborted = true))
      for (let n = 1; !ctx.signal.aborted; n++) {
        await ctx.reply(n, undefined, { more: true })
        await sleep(5)
      }
    })
    const stream = bus.reqStream<number>('svc/endless', 0, 1000)
    for await (const chunk of stream) if (chunk === 3) break
    await sleep(20)
    expect(aborted).toBe(true)
    await expect(stream.result).rejects.toThrow(DynRequestCancelledError)
    bus.destroy()
  })
})

describe('cancel reaches the handler', () => {
  const waitForAbort = (bus: SignalBus, seen: unknown[]) =>
    bus.subscribe('svc/wait', (_msg, ctx) => {
      ctx.signal.addEventListener('abort', () => seen.push(ctx.signal.reason))
      return new Promise<void>((resolve) => ctx.signal.addEventListener('abort', () => resolve()))
    })

  test('aborting the request signal rejects the request and aborts ctx.signal', async () => {
    const bus = new SignalBus()
    const seen: unknown[] = []
    waitForAbort(bus, seen)
    const controller = new AbortController()
    const pending = timed(bus.req('svc/wait', {}, 1000, { signal: controller.signal }))
    await sleep(20)
    controller.abort()
    const [err, ms] = await pending
    expect(err).toBeInstanceOf(DynRequestCancelledError)
    expect(ms).toBeLessThan(200)
    expect(seen.length).toBe(1)
    expect(seen[0]).toBeInstanceOf(DynRequestCancelledError)
    bus.destroy()
  })

  test('a request that times out tells its handler to stop', async () => {
    const bus = new SignalBus()
    const seen: unknown[] = []
    waitForAbort(bus, seen)
    await expect(bus.req('svc/wait', {}, 40)).rejects.toThrow(DynRequestTimeoutError)
    expect(seen.length).toBe(1)
    expect(seen[0]).toBeInstanceOf(DynRequestTimeoutError)
    bus.destroy()
  })

  test('a handler timeout and destroy() abort the signal too', async () => {
    const bus = new SignalBus({ defaultHandlerTimeoutMs: 30 })
    const reasons: string[] = []
    bus.subscribe('work', (_msg, ctx) => {
      ctx.signal.addEventListener('abort', () => reasons.push((ctx.signal.reason as Error).name))
      return new Promise<void>(() => {})
    })
    await bus.publish('work', 1)
    await sleep(60)
    expect(reasons).toEqual(['DynHandlerTimeoutError'])

    const other = new SignalBus()
    other.subscribe('work', (_msg, ctx) => {
      ctx.signal.addEventListener('abort', () => reasons.push((ctx.signal.reason as Error).name))
      return new Promise<void>(() => {})
    })
    await other.publish('work', 1)
    await sleep(5)
    other.destroy()
    expect(reasons).toEqual(['DynHandlerTimeoutError', 'DynDestroyedError'])
    bus.destroy()
  })
})

describe('QoS 2 is accepted once', () => {
  test('the same message id inside the window is delivered once', async () => {
    const bus = new SignalBus<string>({ dedupeWindowMs: 60 })
    const got: string[] = []
    bus.subscribe('orders', (msg) => {
      got.push(msg.payload)
    })

    const first = await bus.publish('orders', 'a', { qos: 2, id: 'order-1' })
    const again = await bus.publish('orders', 'a', { qos: 2, id: 'order-1' })
    await bus.publish('orders', 'b', { qos: 2, id: 'order-2' })
    // QoS 1 makes no such promise.
    await bus.publish('orders', 'c', { qos: 1, id: 'order-3' })
    await bus.publish('orders', 'c', { qos: 1, id: 'order-3' })

    expect(first).toEqual({ id: 'order-1', subscribers: 1 })
    expect(again).toEqual({ id: 'order-1', subscribers: 0, duplicate: true })
    expect(got).toEqual(['a', 'b', 'c', 'c'])

    await sleep(90)
    await bus.publish('orders', 'a', { qos: 2, id: 'order-1' })
    expect(got).toEqual(['a', 'b', 'c', 'c', 'a'])
    bus.destroy()
  })

  test('a publish that was not accepted can be tried again with the same id', async () => {
    const bus = new SignalBus<string>({ stopOnError: true, maxRetries: 0 })
    let runs = 0
    bus.subscribe('orders', () => {
      if (++runs === 1) throw new Error('first try fails')
    })

    await expect(bus.publish('orders', 'a', { qos: 2, id: 'order-1' })).rejects.toThrow()
    expect((await bus.publish('orders', 'a', { qos: 2, id: 'order-1' })).duplicate).toBeUndefined()
    expect(runs).toBe(2)
    bus.destroy()
  })
})

describe('per-topic rate limiting stays bounded', () => {
  test('the bus tracks at most maxTopics topics and keeps the totals', async () => {
    const bus = new SignalBus<number>({ rateLimits: { maxPublishPerSecond: 1000, perTopic: true, maxTopics: 100 } })
    for (let i = 0; i < 1000; i++) await bus.publish(`events/session/${i}`, i)
    expect((bus as any).publishRateLimiter.size).toBeLessThanOrEqual(100)
    expect(bus.getRateLimitStats()?.publish?.allowed).toBe(1000)
    bus.destroy()
  })

  test('topics that have gone quiet are let go', async () => {
    // 20 tokens/s with a burst of 1: a used limiter is full again after 50 ms.
    const limiter = new PerTopicRateLimiter(20, 1)
    for (let i = 0; i < 500; i++) limiter.tryAcquire(`old/${i}`)
    await sleep(80)
    for (let i = 0; i < 524; i++) limiter.tryAcquire(`new/${i}`)
    expect(limiter.size).toBeLessThan(600)
    expect(limiter.totals().allowed).toBe(1024)
    // A topic still inside its window keeps its state.
    expect(limiter.tryAcquire('new/523')).toBe(false)
  })
})

test('publish reports how many subscriptions the message reached', async () => {
  const bus = new SignalBus<string>()
  bus.subscribe('a/+', () => {})
  bus.subscribe('a/#', () => {})
  expect((await bus.publish('a/b', 'x')).subscribers).toBe(2)
  expect((await bus.publish('z', 'x')).subscribers).toBe(0)
  bus.destroy()
})
