/**
 * Byte transports for RESP. The client only needs an ordered byte stream in both directions:
 * - `ws://…` / `wss://…`: RESP frames in binary WebSocket messages (browser dev, the Vite mock at `/resp`).
 * - `ipc://<method>`: the TAO/wry WebView IPC of the Lunarstorm desktop host (platform/desktop/app), using its one envelope
 *   `{version, kind, id, windowId, epoch, method, payload}`: out through `window.ipc.postMessage(json)`, in through
 *   `window.__zebraReceive(envelope)`, which the host calls with `evaluate_script`.
 *
 * IPC wire contract (method defaults to `resp`; the host side implements it):
 * | direction       | kind                    | payload                                   |
 * |-----------------|-------------------------|-------------------------------------------|
 * | WebView -> host | `request`               | `{session, open: true}` — new connection  |
 * | WebView -> host | `request`               | `{session, data}` — RESP bytes, base64    |
 * | WebView -> host | `request`               | `{session, close: true}`                  |
 * | host -> WebView | `response` or `event`   | `{session, data}` — RESP bytes, base64    |
 * | host -> WebView | `response` or `event`   | `{session, closed: true, error?}`         |
 * | host -> WebView | `error`                 | the host refused the message              |
 * Decision: the bytes stay RESP (base64, since envelopes are JSON text) instead of mapping commands to JSON, so the
 * host feeds them to the same RESP parser and the same per-connection state (HELLO protocol, subscriptions) as a
 * socket connection; `session` stands in for the connection, which IPC does not have.
 */

/** What a transport reports to its client. */
export interface TransportEvents {
  /** The transport can send. */
  onOpen: () => void;
  /** Bytes arrived, in order. */
  onData: (bytes: Uint8Array) => void;
  /**
   * The transport closed.
   * @param reason human-readable cause
   * @param failed true when it never opened
   */
  onClose: (reason: string, failed: boolean) => void;
}

/** An ordered, bidirectional byte stream carrying RESP. */
export interface RespTransport {
  /** The endpoint, as shown and edited in the connection bar. */
  readonly endpoint: string;
  /**
   * Opens a new connection; may be called again after a close.
   * @param ev receives open, data and close
   */
  open(ev: TransportEvents): void;
  /**
   * Writes bytes.
   * @param bytes RESP bytes
   * @throws Error when not open
   */
  send(bytes: Uint8Array): void;
  /** Closes; `onClose` follows. */
  close(): void;
}

/** The desktop host's envelope (platform/desktop/app/src/envelope.rs). */
export interface IpcEnvelope {
  /** Envelope version; see `IPC_VERSION`. */
  version: number;
  /** `request`, `response`, `event`, `error`, … */
  kind: string;
  /** Correlation id. */
  id?: number;
  /** Form name, or `@application`. */
  windowId: string;
  /** Backend epoch the sender knew. */
  epoch: number;
  /** What the message is about; `resp` for this transport. */
  method: string;
  /** Kind-specific data. */
  payload: unknown;
}

/** Envelope version the desktop host speaks (`envelope::VERSION`). */
export const IPC_VERSION = 1;

/** Globals the desktop host provides: wry's `window.ipc`, and `window.zebra` / `__zebraReceive` from its shell.js. */
interface IpcWindow {
  /** wry's IPC channel to the host. */
  ipc?: { postMessage(text: string): void };
  /** Host -> WebView entry point, called through `evaluate_script`. */
  __zebraReceive?: (envelope: IpcEnvelope) => void;
  /** The shell runtime: this form's window id and the current backend epoch. */
  zebra?: { windowId: string; epoch: number };
}

/**
 * Base64 of bytes, chunked so large buffers do not overflow the argument limit of `String.fromCharCode`.
 * @param b bytes
 */
function toBase64(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
  return btoa(s);
}

/**
 * Bytes of base64 text.
 * @param t base64
 */
function fromBase64(t: string): Uint8Array {
  return Uint8Array.from(atob(t), (c) => c.charCodeAt(0));
}

/**
 * RESP over a WebSocket.
 * @param url `ws://` or `wss://` URL
 * @param WS WebSocket constructor; tests pass a fake
 * @return the transport
 */
export function webSocketTransport(url: string, WS: typeof WebSocket = globalThis.WebSocket): RespTransport {
  let ws: WebSocket | null = null;
  return {
    endpoint: url,
    open(ev) {
      let opened = false;
      try {
        ws = new WS(url);
      } catch (e) {
        queueMicrotask(() => ev.onClose((e as Error).message, true));
        return;
      }
      ws.binaryType = "arraybuffer";
      ws.onopen = () => {
        opened = true;
        ev.onOpen();
      };
      ws.onmessage = (m: MessageEvent) => ev.onData(m.data instanceof ArrayBuffer ? new Uint8Array(m.data) : new TextEncoder().encode(String(m.data)));
      ws.onclose = (c: CloseEvent) => {
        ws = null;
        ev.onClose(c.reason || (opened ? "closed" : `no RESP endpoint answered at ${url}`), !opened);
      };
    },
    send(bytes) {
      if (!ws || ws.readyState !== 1) throw new Error("Not connected");
      // Fix: TS 7 rejects a Uint8Array that may sit on a SharedArrayBuffer as a WebSocket body. Decision: copy into
      // an ArrayBuffer-backed array (a command pipeline is small) rather than narrowing the transport's type.
      ws.send(new Uint8Array(bytes));
    },
    close() {
      ws?.close();
    },
  };
}

/**
 * RESP over the desktop host's WebView IPC (see the module comment for the contract).
 * Decision: chains onto an existing `window.__zebraReceive` (the shell's) instead of replacing it: envelopes for other
 * methods or sessions go to the previous handler, and a close restores it unless someone chained after us.
 * @param method envelope method carrying RESP; default `resp`
 * @param target the window (tests pass a stand-in)
 * @return the transport
 */
export function ipcTransport(method = "resp", target: IpcWindow = globalThis as unknown as IpcWindow): RespTransport {
  let session = "";
  let seq = 0;
  let active = false;
  let events: TransportEvents | null = null;
  let previous: IpcWindow["__zebraReceive"];
  let handler: IpcWindow["__zebraReceive"];

  const post = (payload: Record<string, unknown>) => {
    const envelope: IpcEnvelope = {
      version: IPC_VERSION, kind: "request", id: ++seq, windowId: target.zebra?.windowId ?? "@application",
      epoch: target.zebra?.epoch ?? 0, method, payload: { session, ...payload },
    };
    target.ipc!.postMessage(JSON.stringify(envelope));
  };
  const detach = () => {
    active = false;
    if (target.__zebraReceive === handler) target.__zebraReceive = previous;
  };

  return {
    endpoint: `ipc://${method}`,
    open(ev) {
      if (!target.ipc?.postMessage) {
        queueMicrotask(() => ev.onClose("no WebView IPC in this page (window.ipc is missing); open it in the desktop host", true));
        return;
      }
      events = ev;
      session = globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
      previous = target.__zebraReceive;
      handler = (envelope) => {
        const p = (envelope?.payload ?? {}) as { session?: string; data?: string; closed?: boolean; error?: string };
        const ours = active && envelope?.method === method && (p.session === session || envelope.kind === "error");
        if (!ours) {
          previous?.(envelope);
          return;
        }
        if (envelope.kind === "error") {
          detach();
          events?.onClose(`host refused the message: ${typeof envelope.payload === "string" ? envelope.payload : JSON.stringify(envelope.payload)}`, false);
          return;
        }
        if (typeof p.data === "string") events?.onData(fromBase64(p.data));
        if (p.closed) {
          detach();
          events?.onClose(p.error ?? "closed by host", false);
        }
      };
      target.__zebraReceive = handler;
      active = true;
      post({ open: true });
      queueMicrotask(() => ev.onOpen());
    },
    send(bytes) {
      if (!active) throw new Error("Not connected");
      post({ data: toBase64(bytes) });
    },
    close() {
      if (!active) return;
      post({ close: true });
      detach();
      const ev = events;
      queueMicrotask(() => ev?.onClose("closed", false));
    },
  };
}

/**
 * The transport for an endpoint string: `ipc://<method>` selects WebView IPC, anything else is a WebSocket URL.
 * @param endpoint e.g. `ipc://resp` or `ws://localhost:5180/resp`
 * @param WS WebSocket constructor for WebSocket endpoints
 * @return the transport
 */
export function transportFor(endpoint: string, WS?: typeof WebSocket): RespTransport {
  return endpoint.startsWith("ipc://") ? ipcTransport(endpoint.slice(6) || "resp") : webSocketTransport(endpoint, WS);
}

/**
 * Default endpoint: `ipc://resp` inside the desktop host (wry provides `window.ipc`), otherwise `/resp` on the page's
 * own host over `ws:` or `wss:` matching the page.
 * @return the endpoint string
 */
export function defaultEndpoint(): string {
  if ((globalThis as unknown as IpcWindow).ipc?.postMessage) return "ipc://resp";
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/resp`;
}
