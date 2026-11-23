# The Coldmind protocol (version 1)

A server has one input: a frame of this protocol. A frame is structure, not bytes. Encoding (the codec)
and what carries it (WebSocket, TCP, HTTP, a function call) are outside the protocol. Other protocols
(JSON-RPC, MQTT, anything else) are translated to and from these frames by adapters.

RPC and the bus use the same six frames. The bus is not a second protocol: it is a controller (`bus`)
called with ordinary `call` frames, where a subscription is a call that stays open.

Every frame has `v: 1` and a `kind`.

## Frames a server is fed

| kind     | fields                                         | meaning |
|----------|------------------------------------------------|---------|
| `call`   | `id`, `name`, `args`, `sid?`, `auth?`, `meta?`, `timeoutMs?` | Call `controller.method`. `id` is chosen by the caller and unique among its calls in flight. |
| `cancel` | `id`                                           | Stop the call with that id. Never answered itself. |

- `id`: integer, or string of 1–128 characters.
- `sid`: session reference, as handed out by the server.
- `auth`: `{ method, credentials }`. Authenticates this call only; no session is created.
- `meta`: free-form object for adapters and hooks. Not validated against the proc.
  `meta.trace` = `{ traceId, spanId }` is the caller's trace context, by convention; `telemetryAdapter` continues it.
- `timeoutMs`: positive integer. The caller's deadline for the whole call, counted from when the server
  receives the frame. The server stops the call with `TIMEOUT` when it passes. It can only shorten the
  server's own limit, never extend it.

## Frames a server sends

| kind     | fields                                   | meaning |
|----------|------------------------------------------|---------|
| `result` | `id`, `value`, `sid?`                    | The call succeeded. Final frame for that id. |
| `error`  | `id`, `code`, `message`, `data?`, `sid?` | The call failed. Final frame for that id. `id` is null when the input could not be tied to a call. |
| `chunk`  | `id`, `value`                            | Stream item of a call in flight. Zero or more, in order, before the final frame. |
| `event`  | `name`, `data`                           | Server-initiated. Not tied to any call. |

`sid` on a final frame tells the caller its session reference changed: a string is the new reference,
`null` means the session is gone, absent means no change. A caller applies it only if the call was sent
under the reference it currently holds, so a late reply cannot overwrite a newer one.

## Rules

1. Every `call` gets exactly one final frame, `result` or `error`, unless the peer closes first.
2. `cancel` aborts cooperatively: the server answers the call with `error` / `CANCELLED` at once and signals
   the handler. A call that was cancelled, timed out or failed leaves the session unchanged.
3. A frame that is not valid is answered with `error` / `BAD_FRAME`; a wrong `v` with `UNSUPPORTED_VERSION`.
   The peer stays open.
4. Unknown fields are ignored. New frame kinds and fields get a new `v` only when an old peer could misread them.

## Error codes

| code                  | meaning |
|-----------------------|---------|
| `BAD_FRAME`           | Not a frame, a duplicate call id, or an unknown kind |
| `UNSUPPORTED_VERSION` | `v` is not 1 |
| `UNKNOWN_PROC`        | No such `controller.method` |
| `INVALID_ARGS`        | Args, credentials, a topic or a filter do not match what is expected |
| `AUTH_FAILED`         | Credentials refused, or no adapter for the method |
| `UNAUTHENTICATED`     | The proc needs a principal and the call has none |
| `FORBIDDEN`           | The principal may not call the proc, or not for this topic |
| `CANCELLED`           | Stopped by a `cancel` frame |
| `TIMEOUT`             | Stopped by a time limit (the server's for calls, the bus's for requests) |
| `BUSY`                | Too many calls in flight on this peer, or the bus is at a rate or queue limit |
| `NO_RESPONDER`        | `bus.request`: nobody subscribes to the topic |
| `SLOW_CONSUMER`       | `bus.subscribe`: ended because the reader fell too far behind |
| `REPLACED`            | `bus.subscribe`: the same guaranteed subscription was attached from another connection |
| `UNAVAILABLE`         | The bus is shut down |
| `INTERNAL`            | Server fault. Details never leave the server. |

Handlers and bus responders may use codes of their own. A client adds `CLOSED`, `TRANSPORT` and
`INVALID_RESULT` for failures it detects itself.

## Controller `auth`

| proc          | args                      | result |
|---------------|---------------------------|--------|
| `auth.login`  | `{ method, credentials }` | `{ principal: { id, roles } }`. Binds the principal to the session and rotates `sid`. |
| `auth.logout` | `{}`                      | Ends the session; the reply carries `sid: null`. |
| `auth.whoami` | `{}`                      | `{ principal: { id, roles } \| null }` |

## Controller `bus`

Topics are levels separated by `/`. Filters may use `+` for one level and `#` for the rest.

| proc              | args | answer |
|-------------------|------|--------|
| `bus.subscribe`   | `{ filter, qos?, receiveRetained?, client? }` | Stays open. First chunk `{ ready: true }`, then one chunk per message. `cancel` ends it. |
| `bus.publish`     | `{ topic, payload, qos?, retain?, meta?, id? }` | `{ id, subscribers, duplicate? }` |
| `bus.request`     | `{ topic, payload, timeoutMs?, qos?, meta?, correlationId? }` | Chunks for a streamed reply, then `{ payload }` |
| `bus.reply`       | `{ correlationId, payload, more?, error?, meta?, client? }` | `{ delivered }`. Answers a request this subscriber was given. |
| `bus.ack`         | `{ client, filter, seq }` | Everything up to `seq` is handled. |
| `bus.unsubscribe` | `{ client, filter }` | Ends a guaranteed subscription for good. |

A message chunk is `{ seq, id, topic, payload, qos, retain, timestamp, meta?, correlationId?, replyable?, dup? }`.
A third chunk form, `{ cancel: correlationId }`, tells a subscriber that a request it was given has been given up on.

- **QoS on publish.** 0: no retry. 1: a failed delivery is retried by the bus and dead-lettered after the last
  attempt. 2: as 1, and a message `id` published again inside the bus's dedupe window is accepted once.
- **Guaranteed subscriptions.** `qos ≥ 1` with a `client` id: messages carry `seq` from 1. The server keeps every
  message until `bus.ack` covers it, also while the client is away (for `durableTtlMs`). Subscribing again with the
  same `client` and `filter` delivers what was not acknowledged, in order, marked `dup`. The subscription belongs
  to the principal that made it and to one connection at a time.
- **Without a guarantee** (`qos` 0 or no `client`): `seq` is 0, nothing is kept, the subscription ends with the call.
- **Requests.** A message with `replyable: true` is a request. The subscriber answers with `bus.reply` and the
  `correlationId`; `more: true` makes it one part of a streamed reply, `error` a failure. Reply topics never leave the server.
- **Reserved topics.** Peers can neither publish to nor receive from the server's own topics: `rpc/`, `reply/` and `$`.

### Procs on the bus

Inside the server's process every proc is also reachable as a bus request on `rpc/<controller>/<method>`:
payload = args, `meta` may carry `sid` and `auth`. Every reply payload is a frame: `chunk` frames with `more`,
then the `result` or `error` frame. An `idempotent` proc called with QoS 1 is retried by the bus on `INTERNAL`,
`TIMEOUT`, `BUSY`, `UNAVAILABLE` and `CLOSED`.

## Codecs and bindings

A codec turns a frame into bytes. Shipped: `json` and `msgpack`. Two ends agree on one by name, together with the
protocol version, as `subzero.v1+<codec>`.

- **WebSocket.** The client offers `subzero.v1+json`, `subzero.v1+msgpack` as subprotocols; the server answers
  with the first of its own it finds among them. No offer: the server's first codec. Only unknown offers: refused.
  One encoded frame per message, both directions.
- **TCP.** The client opens with the line `SUBZERO/1 <codec>[,<codec>...]`, the server answers `SUBZERO/1 <codec>`
  or `ERR ...` and closes. Then frames, each prefixed with its length as a 4-byte big-endian integer.
- **HTTP.** `POST` one encoded `call` frame. The response body is the answering frames, length-prefixed as on TCP,
  written as they are produced; a subscription is a response that stays open. Aborting the request cancels the call.
  No `event` frames.
