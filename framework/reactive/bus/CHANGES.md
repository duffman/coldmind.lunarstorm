# Changes to @coldmind/bus

Targeted edits to the supplied sources; `signal.patch` in the archive root is the exact diff.
The original 16 tests pass unchanged. `src/features.test.ts` covers everything below.

## Requests

- **Failures reach the requester.** `ctx.replyError({ code, message, data })` rejects the request with
  `DynRequestError`. A handler that throws for good does the same on its own (code `handlerError`,
  `handlerTimeout` or `circuitBreakerOpen`) when it is the request's only recipient.
- **No responder.** `req` rejects with `DynNoResponderError` at once when no subscription matches the topic.
- **Streamed replies.** `ctx.reply(part, meta, { more: true })` sends one part; `req(..., { onChunk })` receives
  the parts, `reqStream()` returns them as an async iterable with the final reply in `.result`. Every part
  restarts the request timeout.
- **Cancel.** `req(..., { signal })` and `cancelRequest(correlationId)`. Every delivery has `ctx.signal`,
  aborted when its request is cancelled or times out, when the handler's own timeout fires, and on `destroy()`.
  `onAbandon(listener)` reports given-up requests to responders that answer after their handler returned.
- **`reply(msg, payload, options)`** answers a request from outside its handler.

## Delivery

- **A lane per subscription** in parallel dispatch, instead of one shared batch of 100. Subscriptions no longer
  wait for each other. A subscription receives its messages one at a time and in order by default;
  `subscribe(..., { concurrency: n })` lets it work on several at once. Sequential dispatch is as before.
- **Replies take their own path**: straight to the waiting request, past the delivery queue and the publish
  rate limit. A reply is never held up or dropped by other traffic.
- **QoS 2 is accepted once.** A message id published again inside `dedupeWindowMs` (default 60 s, at most
  `dedupeMaxEntries` ids) is not delivered again; the result carries `duplicate: true`. A publish that was not
  accepted can be tried again under the same id.
- **`publish` reports `subscribers`**, the number of subscriptions the message was routed to.

## Rate limiting

- **Per-topic limiters are bounded.** A limiter whose bucket has refilled is dropped (it behaves like a new
  one, so nothing changes for the topic); beyond `rateLimits.maxTopics` (default 10000) the least recently
  used goes. Totals in `getRateLimitStats()` still count every topic ever seen.

## Behaviour that differs from before

- In parallel dispatch, two messages for the same subscription used to run at the same time when they
  landed in one batch. They now run in order unless the subscription sets `concurrency`.
- A request to a topic nobody subscribes to fails at once instead of after its timeout.
- A request whose sole handler throws fails with the handler's error instead of timing out.
- `IPubsub` gained `reqStream`, `reply`, `cancelRequest` and the optional `onAbandon`; `publish` resolves
  with `PublishResult`; `DeliveryContext` gained `signal` and `replyError`; `SubscribeOptions` gained
  `concurrency` and `qos` (the latter is for subscriptions that cross a network; a local bus ignores it).
