import type { DynMessage, IPubsub, SubscribeOptions } from "@coldmind/bus";
import type { EventDef } from "@coldmind/rpc";
import { Action } from "@coldmind/wire";
import type { Client, ClientState } from "@coldmind/net.client/src/rpc/rpc.client.ts";

/**
 * Everything that streams out of a client, as an `Action`: the wire's operators (`map`, `filter`,
 * `debounce`, `distinct`, `switchMap`, `merge`) then work on it directly. All four are lazy and
 * shared: the underlying subscription exists while the action has observers.
 */

/** A bus subscription as an action of messages. Works on a local bus and on a `RemoteBus`. */
export function topicAction<T = unknown>(bus: IPubsub<any, any, any>, filter: string, options?: SubscribeOptions): Action<DynMessage<T>> {
  return Action.fromCallback<DynMessage<T>>(emit => {
    const sub = bus.subscribe(filter, msg => void emit(msg as DynMessage<T>), options);
    return () => sub.unsubscribe();
  });
}

/** A server event as an action of its data. */
export function eventAction<T>(client: Client, def: EventDef<T>): Action<T> {
  return Action.fromCallback<T>(emit => client.on(def, emit));
}

/** The client's connection state ("up", "down", "closed") as an action. */
export function stateAction(client: Client): Action<ClientState> {
  return Action.fromCallback<ClientState>(emit => client.onState(emit));
}

/**
 * The chunks of a streamed call or request as an action. `run` starts it and passes chunks to
 * `onChunk`; the call is cancelled through `signal` when the last observer leaves. A failure goes to `onError`.
 *
 *   streamAction<Row>((onChunk, signal) => api.export({}, { onChunk, signal }))
 */
export function streamAction<C>(run: (onChunk: (chunk: C) => void, signal: AbortSignal) => PromiseLike<unknown>): Action<C> {
  return Action.fromCallback<C>((emit, error, signal) => {
    void Promise.resolve(run(emit, signal)).catch(reason => {
      if (!signal.aborted) error(reason);
    });
  });
}
