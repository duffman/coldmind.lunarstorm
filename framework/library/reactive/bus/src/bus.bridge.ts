import { DynNoResponderError, type DynHandler, type DynSubscription, type IPubsub, type QoS } from "@coldmind/bus";
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

type Bus = IPubsub<any, any, any>;

export type BridgeOptions = {
  /** Name of this bridge, unique among the nodes it connects. Carried in forwarded messages to stop loops. */
  id: string;
  /** Topic filters forwarded from `a` to `b`, and from `b` to `a`. */
  aToB?: string[];
  bToA?: string[];
  /** A message that has crossed this many bridges is not forwarded again. Default 8. */
  maxHops?: number;
  /** Guarantee asked for on the subscriptions the bridge makes (matters when a side is a RemoteBus). Default 0. */
  qos?: QoS;
  /** Timeout for requests the bridge forwards. Default 5000. */
  requestTimeoutMs?: number;
};

const VIA = "$via";

/**
 * Joins two buses. Either side can be a local bus or a RemoteBus, so this is how nodes are linked:
 * give it the node's own bus and a RemoteBus connected to the other node.
 *
 * Messages keep their id, so QoS 2 stays "accepted once" across the bridge. A request is forwarded
 * as a request and its replies, chunks and errors included, travel back. A delivery's `context` is
 * passed on, so it reaches the other side when that is a local bus (a RemoteBus does not send it). Loops are stopped two ways:
 * the bridge never forwards what it published itself, and every forwarded message lists the bridges
 * it has passed (`meta.$via`), so rings of nodes stop at the bridge that would repeat.
 */
export function bridge(a: Bus, b: Bus, opts: BridgeOptions): { stop(): void } {
  const maxHops = opts.maxHops ?? 8;
  const timeout = opts.requestTimeoutMs ?? 5000;
  /** Ids of messages and requests this bridge has put on a bus itself, remembered briefly. */
  const own = new Map<string, number>();
  const remember = (id: string) => {
    const now = Date.now();
    for (const [key, at] of own) {
      if (now - at < 60_000 && own.size <= 10_000) break;
      own.delete(key);
    }
    own.set(id, now);
  };

  /** Requests being forwarded, by the correlationId they arrived with, so giving one up stops the forwarded one. */
  const forwarding = new Map<string, AbortController>();
  const giveUp = (correlationId: string) => forwarding.get(correlationId)?.abort();
  const offs = [a.onAbandon?.(giveUp), b.onAbandon?.(giveUp)];

  const forward = (to: Bus): DynHandler<any, any, any> => (msg, ctx) => {
    if (own.has(msg.id) || (msg.correlationId !== undefined && own.has(msg.correlationId))) return;
    const via: string[] = isObj(msg.meta) && Array.isArray(msg.meta[VIA]) ? (msg.meta[VIA] as string[]) : [];
    if (via.includes(opts.id) || via.length >= maxHops) return;
    // Loop information needs somewhere to live; a meta that is not an object is passed on as it is.
    const meta = msg.meta === undefined || isObj(msg.meta) ? { ...(msg.meta as object | undefined), [VIA]: [...via, opts.id] } : msg.meta;

    if (msg.replyTo && msg.correlationId) {
      // Forwarded as a request of its own; the lane is not held while the other side works on it.
      const correlationId = crypto.randomUUID();
      const original = msg.correlationId;
      const controller = new AbortController();
      remember(correlationId);
      forwarding.set(original, controller);
      void to
        .reqWithId(correlationId, msg.topic, msg.payload, timeout, {
          qos: msg.qos, meta,
          // In-process data of the request; a RemoteBus does not send it.
          context: ctx.context,
          signal: AbortSignal.any([ctx.signal, controller.signal]),
          onChunk: chunk => void ctx.reply(chunk, undefined, { more: true }).catch(() => {}),
        })
        .then(
          result => ctx.reply(result),
          // Nobody there to answer, or the requester gave up: there is nothing to report back.
          error =>
            error instanceof DynNoResponderError || controller.signal.aborted
              ? undefined
              : ctx.replyError(error instanceof Error ? error : new Error(String(error))),
        )
        .catch(() => {})
        .finally(() => {
          if (forwarding.get(original) === controller) forwarding.delete(original);
        });
      return;
    }

    remember(msg.id);
    return to.publish(msg.topic, msg.payload, { qos: msg.qos, retain: msg.retain, meta, id: msg.id, timestamp: msg.timestamp, context: ctx.context }).then(() => {});
  };

  const subs: DynSubscription[] = [
    ...(opts.aToB ?? []).map(filter => a.subscribe(filter, forward(b), { qos: opts.qos })),
    ...(opts.bToA ?? []).map(filter => b.subscribe(filter, forward(a), { qos: opts.qos })),
  ];
  return {
    stop() {
      for (const sub of subs.splice(0)) sub.unsubscribe();
      for (const off of offs) off?.();
      for (const controller of forwarding.values()) controller.abort();
    },
  };
}
