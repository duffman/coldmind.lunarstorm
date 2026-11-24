/**
 * A decorated controller as its caller sees it. The client builds from the class's shape (decorators and
 * their arguments; the compiler strips the bodies), so the same class declares both sides.
 */
import { contractOf, eventsOf } from "@coldmind/rpc";
import type { Action } from "@coldmind/wire/src/core/action.ts";
import type { CallOptions, Client } from "@coldmind/net.client/src/rpc/rpc.client.ts";
import { eventAction } from "@coldmind/net.client/src/actions/signal.actions.ts";

type Method = (...args: never[]) => unknown;
type Remoted<F> = F extends (args: infer A, ...rest: never[]) => infer R ? (args: A, opts?: CallOptions) => Promise<Awaited<R>> : never;

/**
 * What a client calls: each method of the controller as `(args, opts?) => Promise<result>`, each `Action`
 * field as the action of that event's messages. Undecorated methods appear too: only proc names are real.
 */
export type Remote<C> = {
  [K in keyof C as C[K] extends Method ? K : C[K] extends Action<infer _T> ? K : never]: C[K] extends Method ? Remoted<C[K]> : C[K];
};

type Proc = (args: unknown, opts?: CallOptions) => Promise<unknown>;

/**
 * The remote side of a class decorated with `@Controller`: its procs over `client`, its events as actions.
 *
 * @throws {TypeError} When `ctor` is not decorated with `@Controller`.
 * @example
 * const notes = remoteOf(client, NotesController);
 * await notes.add({ text: "hi" });
 * notes.added.onData(note => ...);
 */
export function remoteOf<C extends object>(client: Client, ctor: abstract new (...args: never[]) => C): Remote<C> {
  const contract = contractOf(ctor);
  const api = client.api(contract) as unknown as Record<string, Proc>;
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(contract.procs)) out[key] = (args: unknown, opts?: CallOptions) => api[key]!(args, opts);
  for (const [key, def] of Object.entries(eventsOf(ctor))) out[key] = eventAction(client, def);
  return out as Remote<C>;
}
