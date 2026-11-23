import { any, bool, guard, num, obj, str, union } from "@coldmind/model";
import { contract, type OutputOf } from "@coldmind/rpc";

const qos = guard(any<0 | 1 | 2>(), v => v === 0 || v === 1 || v === 2, "Must be 0, 1 or 2", "ONE_OF");
/** Anything, including nothing: payloads and metadata belong to the application. */
const anything = () => any().optional().nullable();

/** A bus message as a network subscriber receives it. */
export const busMessage = obj({
  /** Position in a guaranteed subscription's stream, for `bus.ack`. 0 on subscriptions without a guarantee. */
  seq: num(),
  id: str(),
  topic: str(),
  payload: anything(),
  qos,
  retain: bool(),
  timestamp: num(),
  meta: anything(),
  /** Present together with `replyable` when the message is a request this subscriber may answer with `bus.reply`. */
  correlationId: str().optional(),
  replyable: bool().optional(),
  /** Sent before: the server is delivering it again after a reconnect because it was never acknowledged. */
  dup: bool().optional(),
});
export type BusMessage = OutputOf<typeof busMessage>;

/** First chunk of every subscription: the server has it in place. */
const ready = obj({ ready: guard(any<true>(), v => v === true, "Must be true", "ONE_OF") });
/** The request with this correlationId was given up on; stop working on it. */
const cancel = obj({ cancel: str() });

/** What a `bus.subscribe` call streams: `ready` first, then messages and request cancellations. */
export const busChunk = union(ready, cancel, busMessage);
export type BusChunk = OutputOf<typeof busChunk>;

/**
 * The bus as a controller. A subscription is a call that stays open: every message is a chunk,
 * and cancelling the call ends it. Shared by server and client like any contract.
 */
export const busContract = contract("bus", {
  subscribe: {
    args: {
      filter: str(),
      qos: qos.optional(),
      receiveRetained: bool().optional(),
      /** Stable id of the subscribing client. With qos ≥ 1 it makes the subscription survive a lost connection. */
      client: str().optional(),
    },
    chunk: busChunk,
    noTimeout: true,
  },
  publish: {
    args: {
      topic: str(),
      payload: anything(),
      qos: qos.optional(),
      retain: bool().optional(),
      meta: anything(),
      id: str().optional(),
    },
    result: { id: str(), subscribers: num(), duplicate: bool().optional() },
  },
  request: {
    args: {
      topic: str(),
      payload: anything(),
      timeoutMs: num().optional(),
      qos: qos.optional(),
      meta: anything(),
      correlationId: str().optional(),
    },
    chunk: anything(),
    result: { payload: anything() },
  },
  reply: {
    args: {
      correlationId: str(),
      payload: anything(),
      more: bool().optional(),
      error: obj({ code: str(), message: str(), data: anything() }).optional(),
      meta: anything(),
      client: str().optional(),
    },
    result: { delivered: bool() },
  },
  ack: { args: { client: str(), filter: str(), seq: num() } },
  unsubscribe: { args: { client: str(), filter: str() } },
});
