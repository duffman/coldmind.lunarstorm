import { RpcError, parseLocal, parseWithAsync } from "@coldmind/rpc/src/rpc.errors.ts";
import { needsAsync, type InputOf, type Model, type OutputOf, type Schema } from "@coldmind/rpc/src/rpc.model.ts";

/** Who may call a proc: anyone, any logged-in principal, or a principal with one of the roles. */
export type Access = "public" | "authenticated" | { roles: readonly string[] };

/** A function definition: its name, schemas and policy. What a `ProcObj2` routes. */
export type TProc = {
  name: string;
  /** Informational text for docs and introspection. First line is the summary, the rest the description. */
  doc?: string;
  /** A model or a plain shape of fields from `@coldmind/model`. */
  args: Schema;
  result?: Schema;
  /** Shape of the stream chunks this proc may send before its result. */
  chunk?: Schema;
  /** Read by the default access check. Default: "authenticated". */
  access?: Access;
  /**
   * Calling it twice has the same effect as calling it once. Over the bus such a proc is called
   * with QoS 1, so a failed attempt is retried and ends in the dead letter queue if it keeps failing.
   */
  idempotent?: boolean;
  /** A call that stays open on purpose (a subscription). Exempt from the server's and the client's call timeout. */
  noTimeout?: boolean;
};

/** The one callback every call goes through, with args already validated. */
export type OnCall = (proc: TProc, args: Model) => unknown | Promise<unknown>;

const indexes = new WeakMap<readonly TProc[], Map<string, TProc>>();

/** A list of function definitions plus the single callback every invocation is routed through. */
export class ProcObj2 {
  readonly procs: readonly TProc[];
  private readonly index: Map<string, TProc>;

  /**
   * `validation`: "full" (default) runs every validator, async ones included; this is the side that
   * owns the data. "local" is for a caller: schemas with async validators are left to the other side.
   */
  constructor(procs: readonly TProc[], public onCall: OnCall, readonly validation: "full" | "local" = "full") {
    let index = indexes.get(procs);
    if (!index) {
      procs = Object.freeze([...procs]);
      index = new Map();
      for (const p of procs) {
        if (index.has(p.name)) throw new Error(`Duplicate proc: ${p.name}`);
        index.set(p.name, p);
      }
      indexes.set(procs, index);
    }
    this.procs = procs;
    this.index = index;
  }

  /** The definition named `controller.method`. */
  get(name: string): TProc | undefined {
    return this.index.get(name);
  }

  /**
   * Validates `args` against the proc and hands them to `onCall`.
   * @throws {RpcError} UNKNOWN_PROC for an unknown name, INVALID_ARGS with `data.errors` when validation fails.
   */
  async call(name: string, args: unknown): Promise<unknown> {
    const proc = this.index.get(name);
    if (!proc) throw new RpcError("UNKNOWN_PROC", `Unknown proc: ${name}`);
    // Only schemas with async validators wait for them: a rejection must not take longer than a lookup failure.
    const parsed =
      this.validation === "full" && needsAsync(proc.args) ? await parseWithAsync(proc.args, args, "INVALID_ARGS") : parseLocal(proc.args, args, "INVALID_ARGS");
    return this.onCall(proc, parsed as Model);
  }

  /** Same definitions, different execution. Shares the lookup index, so it is cheap per call. */
  bind(onCall: OnCall): ProcObj2 {
    return new ProcObj2(this.procs, onCall, this.validation);
  }
}

// ---- contracts: a named group of definitions shared by client and server ----

/** A proc as a contract declares it: the name comes from its key. */
export type ProcDef = Omit<TProc, "name">;
/** Method name to definition. */
export type ProcMap = Record<string, ProcDef>;
/** A controller's name and its procs, imported by both sides. */
export type Contract<P extends ProcMap = ProcMap> = {
  name: string;
  procs: P;
  /** Informational text for the controller as a whole. */
  doc?: string;
};

export type ContractOptions = { doc?: string | undefined };

/** Args as the handler receives them: validated, defaults applied. */
export type ArgsOf<D> = D extends { args: infer S } ? OutputOf<S> : never;
/** Args as a caller passes them: fields with a default may be left out. */
export type InputArgsOf<D> = D extends { args: infer S } ? InputOf<S> : never;
/** The validated result type of a proc definition. */
export type ResultOf<D> = D extends { result: infer S extends Schema } ? OutputOf<S> : unknown;
/** The validated type of each streamed chunk of a proc definition. */
export type ChunkOf<D> = D extends { chunk: infer S extends Schema } ? OutputOf<S> : unknown;

/**
 * Names a controller and declares its procs; each key is a method, called as `name.key`.
 * @example
 * const orders = contract("orders", { get: { args: { id: str() }, result: Order, access: "public" } });
 */
export const contract =<P extends ProcMap>(name: string, procs: P, opts: ContractOptions = {}): Contract<P> =>
  opts.doc === undefined ? { name, procs } : { name, procs, doc: opts.doc };

/** Flattens a contract to the `TProc[]` a ProcObj2 takes; names become `controller.method`. */
export const procsOf = (c: Contract): TProc[] =>
  Object.entries(c.procs).map(([key, def]) => ({ ...def, name: `${c.name}.${key}` }));

// ---- events: server-initiated, shared by client and server like a contract ----

/** A server-initiated message with a schema; `server.publish` sends it, `client.on` receives it validated. */
export type EventDef<T = unknown> = {
  name: string;
  data: Schema;
  /** Informational text for docs and introspection. */
  doc?: string;
  /** Type carrier only. */ readonly $data?: T;
};
export type EventOptions = { doc?: string | undefined };
/** Defines an event named `name` carrying `data`. */
export const event = <S extends Schema>(name: string, data: S, opts: EventOptions = {}): EventDef<OutputOf<S>> =>
  opts.doc === undefined ? { name, data } : { name, data, doc: opts.doc };
