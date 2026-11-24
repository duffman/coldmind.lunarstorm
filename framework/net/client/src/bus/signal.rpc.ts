import { DynRequestError, type IPubsub } from "@coldmind/bus";
import { RpcError, parseLocal, validateLocal } from "@coldmind/rpc";
import type { Model } from "@coldmind/rpc";
import type { ChunkOf, Contract, InputArgsOf, ProcDef, ProcMap, ResultOf } from "@coldmind/rpc";
import { parseOutFrame, type AuthSlot } from "@coldmind/protocol";

export type BusCallOptions<C = unknown> = { onChunk?: (chunk: C) => void; signal?: AbortSignal; timeoutMs?: number };
export type BusApi<P extends ProcMap> = {
  [K in keyof P]: (args: InputArgsOf<P[K]>, opts?: BusCallOptions<ChunkOf<P[K]>>) => Promise<ResultOf<P[K]>>;
};

/**
 * Typed calls to a server's procs from anywhere on the bus, the in-process counterpart of
 * `client.api(contract)`. Idempotent procs go out with QoS 1 (retried by the bus, dead-lettered if
 * they keep failing), all others with QoS 0. `session.sid` is kept up to date like a Client's.
 */
export function busApi<P extends ProcMap>(
  bus: IPubsub<any, any, any>,
  c: Contract<P>,
  opts: { prefix?: string; timeoutMs?: number; auth?: AuthSlot; session?: { sid: string | null } } = {},
): BusApi<P> {
  const prefix = opts.prefix ?? "rpc";
  const session = opts.session ?? { sid: null };
  const api: Record<string, (args: Model, callOpts?: BusCallOptions) => Promise<unknown>> = {};

  for (const [method, def] of Object.entries(c.procs) as [string, ProcDef][]) {
    api[method] = async (args, callOpts = {}) => {
      const parsed = parseLocal(def.args, args, "INVALID_ARGS");
      const sentSid = session.sid;
      let raw: unknown;
      try {
        raw = await bus.req(`${prefix}/${c.name}/${method}`, parsed, callOpts.timeoutMs ?? opts.timeoutMs ?? 30_000, {
          qos: def.idempotent ? 1 : 0,
          meta: { ...(sentSid !== null ? { sid: sentSid } : {}), ...(opts.auth ? { auth: opts.auth } : {}) },
          signal: callOpts.signal,
          onChunk: chunk => {
            const frame = parseOutFrame(chunk);
            if (frame?.kind !== "chunk" || !callOpts.onChunk) return;
            callOpts.onChunk(def.chunk ? validateLocal(def.chunk, frame.value) : frame.value);
          },
        });
      } catch (e) {
        // The bus answers for the server when its retries are used up.
        if (e instanceof DynRequestError) throw new RpcError(e.code, e.message, e.data);
        throw e;
      }
      const frame = parseOutFrame(raw);
      if (!frame || (frame.kind !== "result" && frame.kind !== "error")) throw new RpcError("TRANSPORT", "Reply was not a result or error frame");
      if (frame.sid !== undefined && sentSid === session.sid) session.sid = frame.sid;
      if (frame.kind === "error") throw new RpcError(frame.code, frame.message, frame.data);
      return def.result ? parseLocal(def.result, frame.value, "INVALID_RESULT") : frame.value;
    };
  }
  return api as BusApi<P>;
}
