import { authContract } from "@coldmind/protocol";
import { RpcError, parseLocal, validateLocal } from "@coldmind/rpc";
import type { Model } from "@coldmind/rpc";
import { ProcObj2, procsOf, type ChunkOf, type Contract, type EventDef, type InputArgsOf, type ProcMap, type ResultOf, type TProc } from "@coldmind/rpc";
import { frames, parseOutFrame, type AuthSlot, type FrameId, type InFrame } from "@coldmind/protocol";
import type { ActionTelemetry, ActionTraceContext } from "@coldmind/wire";
import type { Link } from "@coldmind/net.client/src/link/rpc.link.ts";

export type ClientOptions = {
  /** Idle timeout per call: time allowed without a chunk or a result. Default 30 s. 0 turns it off. */
  timeoutMs?: number;
  /** Credentials attached to every call, for use without a session. */
  auth?: AuthSlot;
  /**
   * Starts a span per call and sends its trace context along in `meta.trace`, so a server with
   * `telemetryAdapter` continues the same trace.
   */
  telemetry?: ActionTelemetry;
};

export type CallOptions<C = unknown> = {
  onChunk?: (chunk: C) => void;
  signal?: AbortSignal;
  /** Idle timeout: time allowed without a chunk or a result. */
  timeoutMs?: number;
  /** Deadline for the whole call. Sent to the server, which stops the call when it passes. */
  deadlineMs?: number;
  auth?: AuthSlot;
  meta?: Model;
  /** Parent trace context for this call's span (with `ClientOptions.telemetry`). */
  trace?: ActionTraceContext;
};

export type Api<P extends ProcMap> = {
  [K in keyof P]: (args: InputArgsOf<P[K]>, opts?: CallOptions<ChunkOf<P[K]>>) => Promise<ResultOf<P[K]>>;
};

type Pending = {
  proc: TProc;
  /** Session id the call was sent with; a reply may only replace the id it was sent under. */
  sentSid: string | null;
  onChunk?: (chunk: any) => void;
  touch(): void;
  resolve(value: unknown): void;
  reject(error: unknown): void;
  abandon(error: RpcError, tellServer?: boolean): void;
};
type Listener = { def: EventDef<any>; cb: (data: any) => void };
export type ClientState = "up" | "down" | "closed";

/** Runs a user callback without letting its exception break frame delivery. */
const guarded = (fn: () => void) => {
  try {
    fn();
  } catch (e) {
    queueMicrotask(() => {
      throw e;
    });
  }
};

/**
 * The calling side of the protocol over any `Link`. Validates args before sending and results and chunks on
 * arrival (async validators are left to the server), keeps the session id, and dispatches server events.
 * With a `reconnectingLink`, calls in flight when the connection drops fail with CLOSED; later ones wait in the link.
 */
export class Client {
  /** Session id handed out by the server; sent with every call. Persist and restore it to resume a session. */
  sid: string | null = null;
  private seq = 0;
  private closed = false;
  private up = true;
  private readonly stateListeners = new Set<(state: ClientState) => void>();
  private upWaiters: { resolve(): void; reject(e: unknown): void }[] = [];
  private readonly pending = new Map<FrameId, Pending>();
  private readonly listeners = new Map<string, Set<Listener>>();
  private readonly contracts = new Map<string, Contract>([[authContract.name, authContract]]);
  private obj: ProcObj2 = new ProcObj2(procsOf(authContract), () => undefined, "local");

  constructor(private readonly link: Link, private readonly opts: ClientOptions = {}) {
    link.onFrame(raw => this.receive(raw));
    link.onClose(() => {
      this.closed = true;
      for (const p of [...this.pending.values()]) p.reject(new RpcError("CLOSED", "Connection closed"));
      for (const w of this.upWaiters.splice(0)) w.reject(new RpcError("CLOSED", "Connection closed"));
      this.notify();
    });
    // A link that reconnects: calls in flight when the connection drops are lost, later ones wait in the link.
    link.onState?.(state => {
      if (this.closed || this.up === (state === "up")) return;
      this.up = state === "up";
      if (this.up) for (const w of this.upWaiters.splice(0)) w.resolve();
      else for (const p of [...this.pending.values()]) p.reject(new RpcError("CLOSED", "Connection lost"));
      this.notify();
    });
  }

  /** "up": calls go through. "down": the link is reconnecting. "closed": the link is gone for good. */
  get state(): ClientState {
    return this.closed ? "closed" : this.up ? "up" : "down";
  }

  onState(cb: (state: ClientState) => void): () => void {
    this.stateListeners.add(cb);
    return () => void this.stateListeners.delete(cb);
  }

  /** Resolves when the client is up; rejects when it is closed for good. */
  whenUp(): Promise<void> {
    if (this.closed) return Promise.reject(new RpcError("CLOSED", "Connection closed"));
    if (this.up) return Promise.resolve();
    return new Promise((resolve, reject) => this.upWaiters.push({ resolve, reject }));
  }

  private notify(): void {
    for (const cb of [...this.stateListeners]) guarded(() => cb(this.state));
  }

  /** Adds a contract's definitions to this client. */
  register(c: Contract): this {
    const existing = this.contracts.get(c.name);
    if (existing === c) return this;
    if (existing) throw new Error(`Contract name already registered: ${c.name}`);
    this.contracts.set(c.name, c);
    this.obj = new ProcObj2([...this.contracts.values()].flatMap(procsOf), () => undefined, "local");
    return this;
  }

  /** Typed proxy for a contract: `client.api(users).get({ id })`. */
  api<P extends ProcMap>(c: Contract<P>): Api<P> {
    this.register(c);
    const api: Record<string, (args: Model, opts?: CallOptions) => Promise<unknown>> = {};
    for (const key of Object.keys(c.procs)) api[key] = (args, opts) => this.call(`${c.name}.${key}`, args, opts);
    return api as Api<P>;
  }

  /** By-name call against any registered contract. Args are validated before anything is sent. */
  call(name: string, args: unknown, opts: CallOptions = {}): Promise<unknown> {
    // The client's onCall: turn the call into a frame and await the frames that answer it.
    return this.obj.bind((proc, parsed) => this.invoke(proc, parsed, opts)).call(name, args);
  }

  /** Subscribes to a server event. Returns the unsubscribe function. */
  on<T>(def: EventDef<T>, cb: (data: T) => void): () => void {
    const listener: Listener = { def, cb };
    const set = this.listeners.get(def.name) ?? new Set();
    this.listeners.set(def.name, set.add(listener));
    return () => void set.delete(listener);
  }

  /** `auth.login`: the server's auth adapters verify `credentials` for `method`; the session id rotates. */
  login(method: string, credentials: Model) {
    return this.api(authContract).login({ method, credentials });
  }
  logout() {
    return this.api(authContract).logout({});
  }
  whoami() {
    return this.api(authContract).whoami({});
  }

  close(): void {
    this.link.close();
  }

  private invoke(proc: TProc, args: Model, opts: CallOptions): Promise<unknown> {
    return new Promise((resolve, reject) => {
      if (this.closed) return reject(new RpcError("CLOSED", "Connection closed"));
      if (opts.signal?.aborted) return reject(new RpcError("CANCELLED", "Call cancelled"));
      const id = ++this.seq;
      const ms = opts.timeoutMs ?? (proc.noTimeout ? 0 : (this.opts.timeoutMs ?? 30_000));
      const arm = () =>
        ms > 0 && Number.isFinite(ms) ? setTimeout(() => p.abandon(new RpcError("TIMEOUT", `${proc.name} timed out`)), ms) : undefined;
      let timer = arm();
      const onAbort = () => p.abandon(new RpcError("CANCELLED", "Call cancelled"));
      const span = this.opts.telemetry?.startSpan(`client.${proc.name}`, opts.trace ? { parent: opts.trace } : {});
      const deadline =
        opts.deadlineMs !== undefined ? setTimeout(() => p.abandon(new RpcError("TIMEOUT", `${proc.name} passed its deadline`), false), opts.deadlineMs) : undefined;
      const finish = () => {
        clearTimeout(timer);
        clearTimeout(deadline);
        opts.signal?.removeEventListener("abort", onAbort);
        this.pending.delete(id);
      };
      const p: Pending = {
        proc,
        sentSid: this.sid,
        onChunk: opts.onChunk,
        touch: () => {
          clearTimeout(timer);
          timer = arm();
        },
        resolve: value => (finish(), span?.end(), resolve(value)),
        reject: error => (finish(), span?.end("error"), reject(error)),
        // Give up locally and tell the server to stop. Not for a deadline: it travels in the frame, so the server
        // enforces it itself and answers with TIMEOUT rather than a cancel that raced it.
        abandon: (error, tellServer = true) => {
          finish();
          span?.end("cancelled");
          if (tellServer) this.transmit(frames.cancel(id)).catch(() => {});
          reject(error);
        },
      };
      this.pending.set(id, p);
      opts.signal?.addEventListener("abort", onAbort, { once: true });
      const frame = frames.call(id, proc.name, args, {
        sid: this.sid ?? undefined,
        auth: opts.auth ?? this.opts.auth,
        meta: span ? { ...opts.meta, trace: span.context } : opts.meta,
        timeoutMs: opts.deadlineMs,
      });
      this.transmit(frame).catch(p.reject);
    });
  }

  private async transmit(frame: InFrame): Promise<void> {
    await this.link.send(frame);
  }

  private receive(raw: unknown): void {
    const frame = parseOutFrame(raw);
    if (!frame) return;
    if (frame.kind === "event") {
      for (const { def, cb } of [...(this.listeners.get(frame.name) ?? [])]) {
        let data: unknown;
        try {
          data = validateLocal(def.data, frame.data);
        } catch {
          continue;
        }
        guarded(() => cb(data));
      }
      return;
    }
    if (frame.id === null) return;
    const p = this.pending.get(frame.id);
    if (!p) return;
    if (frame.kind === "chunk") {
      p.touch();
      const onChunk = p.onChunk;
      if (!onChunk) return;
      let chunk: unknown;
      try {
        chunk = p.proc.chunk ? parseLocal(p.proc.chunk, frame.value, "INVALID_RESULT") : frame.value;
      } catch (e) {
        return p.abandon(e as RpcError);
      }
      return guarded(() => onChunk(chunk));
    }
    if (frame.sid !== undefined && p.sentSid === this.sid) this.sid = frame.sid;
    if (frame.kind === "error") return p.reject(new RpcError(frame.code, frame.message, frame.data));
    try {
      p.resolve(p.proc.result ? parseLocal(p.proc.result, frame.value, "INVALID_RESULT") : frame.value);
    } catch (e) {
      p.reject(e);
    }
  }
}
