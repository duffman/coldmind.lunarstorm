/**
 * A Redis-protocol client over a byte transport (transport.ts): a WebSocket in the browser, or the desktop host's
 * TAO/wry WebView IPC. A browser cannot open the TCP socket a Redis server listens on, so RESP always rides inside one.
 */
import { encodeCommand, RespDecoder, type Reply } from "./resp.ts";
import { transportFor, type RespTransport } from "./transport.ts";

/** Connection state, as shown by the status pill. */
export type ConnState = "idle" | "connecting" | "open" | "closed" | "error";

/** What `HELLO` reported. */
export interface ServerInfo {
  /** Protocol in use after negotiation. */
  proto: 2 | 3;
  /** `server` field of HELLO, e.g. `redis`. */
  server?: string;
  /** `version` field of HELLO. */
  version?: string;
}

/** Options of `RedisClient`. */
export interface ClientOptions {
  /** WebSocket constructor for `ws://` endpoints; tests pass a fake. */
  WebSocket?: typeof WebSocket;
  /** How long to wait for the HELLO reply before giving up, in ms. Default 5000. */
  helloTimeout?: number;
}

/** Pub/sub confirmation kinds; each answers one (P|S)(UN)SUBSCRIBE argument. */
const SUB_KINDS = new Set(["subscribe", "psubscribe", "ssubscribe", "unsubscribe", "punsubscribe", "sunsubscribe"]);
/** Pub/sub message kinds; never answers to a command. */
const MSG_KINDS = new Set(["message", "pmessage", "smessage"]);

/** A command waiting for its reply. `kind` is set for pub/sub commands, whose reply is a confirmation frame. */
interface Pending {
  /** Receives the reply. */
  resolve: (r: Reply) => void;
  /** Lower-cased command name for (P|S)(UN)SUBSCRIBE. */
  kind?: string;
}

/** The text of a bulk or simple string reply, else undefined. */
function str(r: Reply | undefined): string | undefined {
  return r && (r.t === "bulk" || r.t === "simple") ? r.v : undefined;
}

/**
 * Replies arrive in command order, so each one resolves the oldest pending command, except pub/sub frames:
 * messages go to push listeners, and a confirmation resolves the pending SUBSCRIBE it answers (a SUBSCRIBE with
 * several channels gets one confirmation per channel; the extra ones go to push listeners).
 */
export class RedisClient {
  /** Current connection state. */
  state: ConnState = "idle";
  /** Set after a successful connect. */
  info: ServerInfo | null = null;
  /** Last connection error text, for the status pill tooltip. */
  lastError = "";
  /** Endpoint, as shown in the connection bar. */
  readonly url: string;
  /** The byte stream. */
  private readonly transport: RespTransport;
  /** HELLO reply deadline in ms. */
  private readonly helloTimeout: number;
  /** True between the transport's open and close. */
  private connected = false;
  /** Decoder for the current connection. */
  private decoder = new RespDecoder();
  /** Commands sent and not yet answered, oldest first. */
  private pending: Pending[] = [];
  /** Channels + patterns subscribed, from the last confirmation; with RESP2 a message is only recognised while > 0. */
  private subCount = 0;
  /** The in-flight connect, shared by concurrent callers. */
  private opening: Promise<void> | null = null;
  /** State change listeners. */
  private listeners = new Set<() => void>();
  /** Push message listeners. */
  private pushListeners = new Set<(r: Reply) => void>();

  /**
   * @param endpoint `ipc://resp`, `ws://host/resp`, or a ready transport
   * @param options WebSocket constructor and HELLO timeout
   */
  constructor(endpoint: string | RespTransport, options: ClientOptions = {}) {
    this.transport = typeof endpoint === "string" ? transportFor(endpoint, options.WebSocket) : endpoint;
    this.url = this.transport.endpoint;
    this.helloTimeout = options.helloTimeout ?? 5000;
  }

  /**
   * Listens for state changes (shaped for `useSyncExternalStore`).
   * @param fn called after every change
   * @return unsubscribe
   */
  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  /**
   * Listens for pub/sub messages and other server pushes.
   * @param fn receives each push frame
   * @return unsubscribe
   */
  onPush(fn: (r: Reply) => void): () => void {
    this.pushListeners.add(fn);
    return () => this.pushListeners.delete(fn);
  }

  /**
   * Opens the connection and negotiates RESP3 with `HELLO 3`; a server that rejects HELLO is used with RESP2.
   * Decision: HELLO doubles as the handshake and has a deadline, because IPC "opens" without asking the host, so an
   * endpoint that never answers would otherwise leave the connect pending forever.
   * @return resolves when commands can be sent
   * @throws Error when the transport cannot open, closes during the handshake, or HELLO gets no reply in time
   */
  connect(): Promise<void> {
    if (this.state === "open") return Promise.resolve();
    if (this.opening) return this.opening;
    this.decoder = new RespDecoder();
    this.pending = [];
    this.subCount = 0;
    this.info = null;
    this.setState("connecting");
    this.opening = new Promise<void>((resolve, reject) => {
      const fail = (why: string) => {
        if (this.state !== "connecting") return;
        this.lastError = why;
        this.setState("error");
        reject(new Error(`Could not connect to ${this.url}: ${why}`));
      };
      this.transport.open({
        onOpen: () => {
          this.connected = true;
          const timer = setTimeout(() => {
            fail(`no reply to HELLO within ${this.helloTimeout} ms`);
            this.transport.close();
          }, this.helloTimeout);
          this.write([["HELLO", "3"]]).then(([hello]) => {
            clearTimeout(timer);
            if (this.state !== "connecting") return;
            if (hello?.t === "map") {
              const field = (k: string) => str(hello.v.find(([key]) => str(key) === k)?.[1]);
              this.info = { proto: 3, server: field("server"), version: field("version") };
            } else {
              this.info = { proto: 2 };
            }
            this.setState("open");
            resolve();
          }, (e: Error) => {
            clearTimeout(timer);
            fail(e.message);
          });
        },
        onData: (bytes) => this.receive(bytes),
        onClose: (reason) => {
          this.connected = false;
          for (const p of this.pending.splice(0)) p.resolve({ t: "error", v: "connection closed" });
          if (this.state === "connecting") fail(reason);
          else if (this.state === "open") this.setState("closed");
        },
      });
    }).finally(() => {
      this.opening = null;
    });
    return this.opening;
  }

  /**
   * Sends commands as one pipeline, connecting first when needed.
   * @param cmds commands, each `[name, ...args]`
   * @return one reply per command, in order; server errors are `{ t: "error" }` replies, not rejections
   * @throws Error when the connection cannot be opened
   */
  async send(cmds: string[][]): Promise<Reply[]> {
    await this.connect();
    return this.write(cmds);
  }

  /** Closes the connection; pending commands resolve with a "connection closed" error reply. */
  close(): void {
    this.transport.close();
  }

  /**
   * Writes a pipeline on the open socket and queues its reply slots.
   * @param cmds commands
   * @return replies in order
   */
  private write(cmds: string[][]): Promise<Reply[]> {
    if (!this.connected) return Promise.reject(new Error("Not connected"));
    const replies = cmds.map((args) => new Promise<Reply>((resolve) => {
      const name = args[0]?.toLowerCase() ?? "";
      this.pending.push({ resolve, kind: SUB_KINDS.has(name) ? name : undefined });
    }));
    const parts = cmds.map(encodeCommand);
    const bytes = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    parts.reduce((at, p) => (bytes.set(p, at), at + p.length), 0);
    try {
      this.transport.send(bytes);
    } catch (e) {
      this.pending.splice(-cmds.length);
      return Promise.reject(e as Error);
    }
    return Promise.all(replies);
  }

  /**
   * Feeds received bytes to the decoder and routes each complete reply.
   * Decision: on a protocol error the connection is closed; the byte stream cannot be resynchronised.
   * @param bytes received RESP bytes
   */
  private receive(bytes: Uint8Array): void {
    let replies: Reply[];
    try {
      replies = this.decoder.push(bytes);
    } catch (e) {
      this.lastError = (e as Error).message;
      this.transport.close();
      return;
    }
    for (const r of replies) {
      const kind = this.frameKind(r);
      if (kind && SUB_KINDS.has(kind)) {
        const count = r.t === "push" || r.t === "array" ? r.v[2] : undefined;
        if (count?.t === "int") this.subCount = Number(count.v);
        const head = this.pending[0];
        if (head?.kind === kind) {
          this.pending.shift();
          head.resolve(r);
        } else {
          this.emitPush(r);
        }
      } else if (kind || r.t === "push" || this.pending.length === 0) {
        this.emitPush(r);
      } else {
        this.pending.shift()!.resolve(r);
      }
    }
  }

  /**
   * The pub/sub kind of a frame, or undefined for ordinary replies. RESP3 marks frames as pushes; with RESP2 an array
   * is a frame only when its first element is a confirmation answering a pending SUBSCRIBE, or a message while subscribed.
   * @param r a reply
   */
  private frameKind(r: Reply): string | undefined {
    if (r.t !== "push" && r.t !== "array") return undefined;
    const kind = str(r.v[0])?.toLowerCase();
    if (!kind || !(SUB_KINDS.has(kind) || MSG_KINDS.has(kind))) return undefined;
    if (r.t === "push") return kind;
    if (this.info?.proto === 3) return undefined;
    if (SUB_KINDS.has(kind)) return this.pending[0]?.kind === kind || this.subCount > 0 ? kind : undefined;
    return this.subCount > 0 ? kind : undefined;
  }

  /** Delivers a push to listeners. */
  private emitPush(r: Reply): void {
    for (const fn of this.pushListeners) fn(r);
  }

  /** Sets the state and notifies listeners. */
  private setState(s: ConnState): void {
    this.state = s;
    for (const fn of this.listeners) fn();
  }
}
