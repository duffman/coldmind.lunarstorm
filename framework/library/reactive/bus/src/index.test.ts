import { describe, expect, test } from 'vitest'
import { SignalBus, DynDestroyedError, DynRequestTimeoutError } from '@coldmind/bus'

describe('SignalBus', () => {
  // 1. Basic publish/subscribe
  test('subscribe receives published messages', async () => {
    const bus = new SignalBus<string>()
    const received: string[] = []

    bus.subscribe('chat/room1', (msg) => {
      received.push(msg.payload)
    })

    await bus.publish('chat/room1', 'hello')
    await bus.publish('chat/room1', 'world')
    await new Promise((r) => setTimeout(r, 10))

    expect(received).toEqual(['hello', 'world'])
    bus.destroy()
  })

  // 2. Wildcard pattern matching (+ and #)
  test('wildcards match topics correctly', async () => {
    const bus = new SignalBus<string>()
    const singleLevel: string[] = []
    const multiLevel: string[] = []

    bus.subscribe('chat/+/messages', (msg) => {
      singleLevel.push(msg.payload)
    })

    bus.subscribe('events/#', (msg) => {
      multiLevel.push(msg.payload)
    })

    await bus.publish('chat/room1/messages', 'msg1')
    await bus.publish('chat/room2/messages', 'msg2')
    await bus.publish('chat/room1/typing', 'ignored')
    await bus.publish('events/user/login', 'login')
    await bus.publish('events/system/health/check', 'health')
    await new Promise((r) => setTimeout(r, 10))

    expect(singleLevel.sort()).toEqual(['msg1', 'msg2'])
    expect(multiLevel.sort()).toEqual(['health', 'login'])
    bus.destroy()
  })

  // 3. Unsubscribe stops receiving messages
  test('unsubscribe stops message delivery', async () => {
    const bus = new SignalBus<string>()
    const received: string[] = []

    const sub = bus.subscribe('topic', (msg) => {
      received.push(msg.payload)
    })

    await bus.publish('topic', 'before')
    sub.unsubscribe()
    await bus.publish('topic', 'after')

    expect(received).toEqual(['before'])
    bus.destroy()
  })

  // 4. Request/Response pattern
  test('req/reply returns response from handler', async () => {
    const bus = new SignalBus<{ n: number }>()

    bus.subscribe('math/double', async (msg, ctx) => {
      await ctx.reply({ n: msg.payload.n * 2 })
    })

    const result = await bus.req<{ n: number }, { n: number }>('math/double', { n: 5 }, 1000)

    expect(result.n).toBe(10)
    bus.destroy()
  })

  // 5. Retained messages delivered to new subscribers
  test('retained messages sent to new subscribers', async () => {
    const bus = new SignalBus<string>()
    const received: string[] = []

    await bus.publish('status/online', 'user1', { retain: true })

    bus.subscribe('status/online', (msg) => {
      received.push(msg.payload)
    })

    // Wait for queue processing
    await new Promise((r) => setTimeout(r, 10))

    expect(received).toEqual(['user1'])
    bus.destroy()
  })

  // 6. Multiple subscribers receive same message
  test('multiple subscribers all receive message', async () => {
    const bus = new SignalBus<string>()
    const sub1: string[] = []
    const sub2: string[] = []
    const sub3: string[] = []

    bus.subscribe('broadcast', (msg) => {
      sub1.push(msg.payload)
    })
    bus.subscribe('broadcast', (msg) => {
      sub2.push(msg.payload)
    })
    bus.subscribe('broadcast', (msg) => {
      sub3.push(msg.payload)
    })

    await bus.publish('broadcast', 'hello')
    await new Promise((r) => setTimeout(r, 10))

    expect(sub1).toEqual(['hello'])
    expect(sub2).toEqual(['hello'])
    expect(sub3).toEqual(['hello'])
    bus.destroy()
  })

  // 7. Destroy prevents further operations
  test('destroy prevents publish and subscribe', async () => {
    const bus = new SignalBus<string>()
    bus.destroy()

    expect(bus.isDestroyed).toBe(true)
    expect(() => bus.subscribe('topic', () => {})).toThrow(DynDestroyedError)
    await expect(bus.publish('topic', 'test')).rejects.toThrow(DynDestroyedError)
  })

  // 8. Message includes metadata
  test('message contains id, timestamp, and meta', async () => {
    const bus = new SignalBus<string, { source: string }>()
    let captured: any = null

    bus.subscribe('data', (msg) => {
      captured = msg
    })

    await bus.publish('data', 'payload', { meta: { source: 'test' }, id: 'custom-id' })
    await new Promise((r) => setTimeout(r, 10))

    expect(captured.id).toBe('custom-id')
    expect(captured.payload).toBe('payload')
    expect(captured.meta.source).toBe('test')
    expect(typeof captured.timestamp).toBe('number')
    bus.destroy()
  })

  // 9. Priority ordering - higher priority first
  test('higher priority subscribers execute first', async () => {
    const bus = new SignalBus<string>({ dispatch: 'sequential' })
    const order: number[] = []

    bus.subscribe(
      'ordered',
      () => {
        order.push(1)
      },
      { priority: 1 },
    )
    bus.subscribe(
      'ordered',
      () => {
        order.push(3)
      },
      { priority: 3 },
    )
    bus.subscribe(
      'ordered',
      () => {
        order.push(2)
      },
      { priority: 2 },
    )

    await bus.publish('ordered', 'test')
    await new Promise((r) => setTimeout(r, 10))

    expect(order).toEqual([3, 2, 1])
    bus.destroy()
  })

  // 10. Handler errors don't crash bus
  test('handler error does not stop other subscribers', async () => {
    const bus = new SignalBus<string>()
    const received: string[] = []

    bus.subscribe('topic', () => {
      throw new Error('fail')
    })
    bus.subscribe('topic', (msg) => {
      received.push(msg.payload)
    })

    await bus.publish('topic', 'test')
    await new Promise((r) => setTimeout(r, 10))

    expect(received).toEqual(['test'])
    bus.destroy()
  })

  // 11. Dead letter queue captures failed messages
  test('failed messages go to dead letter queue', async () => {
    const bus = new SignalBus<string>({
      deadLetter: { enabled: true },
      maxRetries: 0,
    })

    bus.subscribe('failing', () => {
      throw new Error('always fails')
    })

    await bus.publish('failing', 'doomed', { qos: 1 })
    await new Promise((r) => setTimeout(r, 50))

    expect(bus.dlqSize()).toBeGreaterThan(0)
    const dlq = bus.peekDlq(1)
    expect(dlq[0].message.payload).toBe('doomed')
    bus.destroy()
  })

  // 12. Request timeout throws error
  test('request times out when no response', async () => {
    const bus = new SignalBus<string>()

    bus.subscribe('slow', () => {
      // Never replies
    })

    await expect(bus.req('slow', 'ping', 50)).rejects.toThrow(DynRequestTimeoutError)
    bus.destroy()
  })

  // 13. Immediate delivery bypasses queue
  test('immediate delivery executes synchronously', async () => {
    const bus = new SignalBus<string>()
    const received: string[] = []

    bus.subscribe('immediate', (msg) => {
      received.push(msg.payload)
    })

    await bus.publish('immediate', 'fast', { delivery: 'immediate' })
    // No wait needed - immediate delivery is sync
    expect(received).toEqual(['fast'])
    bus.destroy()
  })

  // 14. Clear retained removes stored messages
  test('clearRetained removes retained messages', async () => {
    const bus = new SignalBus<string>()

    await bus.publish('persist', 'data', { retain: true })
    bus.clearRetained()

    const received: string[] = []
    bus.subscribe('persist', (msg) => {
      received.push(msg.payload)
    })
    await new Promise((r) => setTimeout(r, 10))

    expect(received).toEqual([])
    bus.destroy()
  })

  // 15. Context provides subscription info
  test('delivery context has subId and filter', async () => {
    const bus = new SignalBus<string>()
    let ctx: any = null

    const sub = bus.subscribe('ctx/test', (msg, c) => {
      ctx = c
    })

    await bus.publish('ctx/test', 'data')
    await new Promise((r) => setTimeout(r, 10))

    expect(ctx.subId).toBe(sub.id)
    expect(ctx.filter).toBe('ctx/test')
    expect(typeof ctx.ack).toBe('function')
    expect(typeof ctx.nack).toBe('function')
    bus.destroy()
  })

  test('equal priority leaves the queue in enqueue order', async () => {
    const bus = new SignalBus<number>({ dispatch: 'sequential' })
    const order: number[] = []
    bus.subscribe('q', (msg) => {
      order.push(msg.payload)
    })
    const publishes = [1, 2, 3, 4, 5].map((n) => bus.publish('q', n))
    await Promise.all(publishes)
    await new Promise((r) => setTimeout(r, 20))
    expect(order).toEqual([1, 2, 3, 4, 5])
    bus.destroy()
  })
})
