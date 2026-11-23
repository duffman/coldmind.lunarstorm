# @coldmind/bus

In-process pub/sub with request/reply: topic filters with wildcards, retained messages, priorities,
retries with a dead letter queue, circuit breaker and rate limits.

    bun run test

What changed from the supplied sources is in [CHANGES.md](CHANGES.md). To put the bus on a network
(WebSocket, TCP, HTTP, MQTT) see [the network guide](../../net/README.md).

## Using the bus

`SignalBus` is in-process. Topics are levels separated by `/`; filters use `+` for one level and `#` for the rest.

```ts
import { SignalBus, DynRequestError } from "@coldmind/bus";

const bus = new SignalBus<any, any, any>();

bus.subscribe("shop/+/stock", msg => console.log(msg.topic, msg.payload));
await bus.publish("shop/A-1/stock", 12, { retain: true });              // new subscribers get the last value

// request / reply
bus.subscribe("svc/price", async (msg, ctx) => {
  if (msg.payload === "unknown") return ctx.replyError({ code: "no_such_sku", message: "Unknown SKU" });
  await ctx.reply({ sku: msg.payload, price: 99 });
});
const price = await bus.req<string, { price: number }>("svc/price", "A-1", 2000);

try {
  await bus.req("svc/price", "unknown");
} catch (e) {
  if (e instanceof DynRequestError) console.log(e.code);                // "no_such_sku", at once
}
```

Streamed replies and cancel:

```ts
bus.subscribe("svc/history", async (msg, ctx) => {
  for (let day = 1; day <= 30 && !ctx.signal.aborted; day++) await ctx.reply({ day }, undefined, { more: true });
  await ctx.reply({ done: true });
});

const history = bus.reqStream<{ day: number }, { done: boolean }>("svc/history", "A-1");
for await (const part of history) if (part.day === 7) break;            // leaving the loop cancels the request
```

What a request can end with:

| outcome | what the requester gets |
|---|---|
| `ctx.reply(value)` | the value |
| `ctx.replyError(...)`, or the only handler throws | `DynRequestError` with `code`, `message`, `data` |
| nobody subscribes to the topic | `DynNoResponderError`, at once |
| no answer in time | `DynRequestTimeoutError`; the handler's `ctx.signal` is aborted |
| `signal` aborted, or `cancelRequest(id)` | `DynRequestCancelledError`; the handler's `ctx.signal` is aborted |

Delivery:

- **Order.** A subscription receives its messages one at a time, in order. Subscriptions never wait for
  each other. `subscribe(filter, handler, { concurrency: 8 })` lets one subscription work on several at once.
- **QoS on publish.** `0`: one attempt. `1`: a failing handler is retried (`maxRetries`, backoff) and the
  message ends in the dead letter queue (`peekDlq`, `drainDlq`). `2`: as 1, and the same message `id`
  published again inside `dedupeWindowMs` is accepted once (`duplicate: true` in the result).
- **Replies** go straight to the waiting request. They pass neither the delivery queue nor the publish rate limit.

## Defaults

| setting | default | option |
|---|---|---|
| retries for QoS ≥ 1 | 3, from 250 ms, doubling | `maxRetries`, `retryDelayMs`, `retryBackoffFactor` |
| QoS 2 dedupe window | 60 s, 10000 ids | `dedupeWindowMs`, `dedupeMaxEntries` |
| queue | 10000 deliveries | `maxQueueSize` |
| request timeout | 5 s | third argument of `req` |
| per-topic rate limiters tracked | 10000 | `rateLimits.maxTopics` |
| dispatch | parallel, one lane per subscription | `dispatch`, per subscription `concurrency` |

## Files

    src/index.ts                SignalBus
    src/types.ts, errors.ts     public types and errors
    src/internal/queue.ts       delivery: lanes, retries, dead letter queue, cancel signals
    src/internal/request.ts     pending requests: replies, chunks, timeouts
    src/internal/matcher.ts     topic trie
    src/internal/store.ts       retained messages
    src/internal/limiter.ts, breaker.ts, dedupe.ts, stream.ts
    src/bus.bridge.ts       bridge(a, b): join two buses (local or RemoteBus); import from "@coldmind/bus/bridge"
