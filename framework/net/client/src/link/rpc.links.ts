import { linkEndpoint, type Link } from "@coldmind/net.client/src/link/rpc.link.ts";
import { SID_CLEARED, frames, isObj, type CallFrame, type FrameId } from "@coldmind/protocol";
import { RpcError } from "@coldmind/rpc";
import type { InFrame } from "@coldmind/protocol";
import { Unpacker, decodeOr, jsonCodec, subprotocol, type Codec } from "@coldmind/protocol";

/**
 * Native link over WebSocket. Uses the global WebSocket (browser, Bun, Node 22+).
 * Offers `codecs` as subprotocols (`subzero.v1+json`, ...) and uses the one the server picks.
 * Sends are queued until the socket is open.
 */
export function wsLink(url: string, opts: { codec?: Codec; codecs?: Codec[] } = {}): Link {
  const codecs = opts.codecs ?? [opts.codec ?? jsonCodec];
  let codec = codecs[0]!;
  const ws = new WebSocket(url, codecs.map(subprotocol));
  ws.binaryType = "arraybuffer";
  const queue: InFrame[] = [];
  const ep = linkEndpoint(
    frame => {
      if (ws.readyState === WebSocket.OPEN) ws.send(codec.encode(frame) as string | BufferSource);
      else queue.push(frame);
    },
    () => ws.close(),
    { open: false },
  );
  ws.onopen = () => {
    codec = codecs.find(c => subprotocol(c) === ws.protocol) ?? codec;
    for (const frame of queue.splice(0)) ws.send(codec.encode(frame) as string | BufferSource);
    ep.opened();
  };
  ws.onmessage = ev => ep.emit(decodeOr(codec, typeof ev.data === "string" ? ev.data : new Uint8Array(ev.data as ArrayBuffer)));
  ws.onclose = () => ep.closed();
  return ep.link;
}

/**
 * Keeps a link alive across lost connections. `connect` makes a fresh link (for example
 * `() => wsLink(url)`); when it closes, a new one is made after a backoff with jitter. Frames sent
 * while the connection is down wait, up to `maxQueued`, and go out when it is back. Reports
 * "down" and "up" through `onState`, which is what lets a Client and a RemoteBus recover.
 */
export function reconnectingLink(connect: () => Link, opts: { minDelayMs?: number; maxDelayMs?: number; maxQueued?: number } = {}): Link {
  const minDelay = opts.minDelayMs ?? 250;
  const maxDelay = opts.maxDelayMs ?? 30_000;
  const maxQueued = opts.maxQueued ?? 1000;
  const stateCbs: ((state: "up" | "down") => void)[] = [];
  const queue: InFrame[] = [];
  let inner: Link | null = null;
  let up = false;
  let stopped = false;
  let attempt = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const outer = linkEndpoint(
    frame => {
      if (up && inner) return inner.send(frame);
      if (queue.length >= maxQueued) throw new RpcError("CLOSED", "Connection lost and the send queue is full");
      queue.push(frame);
    },
    () => {
      stopped = true;
      clearTimeout(timer);
      inner?.close();
    },
    { open: false },
  );

  const retry = () => {
    if (stopped) return;
    const delay = Math.min(maxDelay, minDelay * 2 ** attempt++) * (0.5 + Math.random() / 2);
    timer = setTimeout(open, delay);
  };

  const open = () => {
    if (stopped) return;
    let link: Link;
    try {
      link = connect();
    } catch {
      return retry();
    }
    inner = link;
    link.onFrame(frame => {
      if (inner === link) outer.emit(frame);
    });
    link.onOpen(() => {
      if (inner !== link || stopped) return;
      attempt = 0;
      up = true;
      for (const frame of queue.splice(0)) {
        try {
          void Promise.resolve(link.send(frame)).catch(() => {});
        } catch {
          // The connection went away again; the caller's own timeout covers this frame.
        }
      }
      outer.opened();
      for (const cb of stateCbs) cb("up");
    });
    link.onClose(() => {
      if (inner !== link) return;
      inner = null;
      const wasUp = up;
      up = false;
      if (wasUp) for (const cb of stateCbs) cb("down");
      retry();
    });
  };

  open();
  return Object.assign(outer.link, { onState: (cb: (state: "up" | "down") => void) => void stateCbs.push(cb) });
}

type HttpOpts = { fetch?: typeof fetch; headers?: Record<string, string> };
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** One request per call, shared by the HTTP links: tracks the abort handle so a cancel frame aborts the request. */
function perCall(run: (frame: CallFrame, signal: AbortSignal, emit: (frame: unknown) => void) => Promise<void>): Link {
  const inflight = new Map<FrameId, AbortController>();
  const ep = linkEndpoint(
    frame => {
      if (frame.kind === "cancel") return inflight.get(frame.id)?.abort();
      const ac = new AbortController();
      inflight.set(frame.id, ac);
      run(frame, ac.signal, ep.emit)
        .catch(e => ac.signal.aborted || ep.emit(frames.error(frame.id, "TRANSPORT", message(e))))
        .finally(() => inflight.delete(frame.id));
    },
    () => {
      for (const ac of inflight.values()) ac.abort();
    },
  );
  return ep.link;
}

/**
 * Native link over HTTP: POST one call frame, read the answering frames (chunks, then result or error)
 * from the response body as they arrive. No events: nothing can reach the client between calls.
 */
export function httpLink(url: string, opts: HttpOpts & { codec?: Codec } = {}): Link {
  const codec = opts.codec ?? jsonCodec;
  const doFetch = opts.fetch ?? fetch;
  return perCall(async (frame, signal, emit) => {
    const res = await doFetch(url, {
      method: "POST",
      body: codec.encode(frame) as BodyInit,
      signal,
      headers: { "content-type": "application/octet-stream", ...opts.headers },
    });
    if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
    const unpacker = new Unpacker();
    const reader = res.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      for (const bytes of unpacker.push(value)) emit(decodeOr(codec, bytes));
    }
    // Ignored by the client when the call was already answered.
    emit(frames.error(frame.id, "TRANSPORT", "Response ended without a result"));
  });
}

const FROM_JSONRPC: Record<number, string> = { [-32700]: "BAD_FRAME", [-32600]: "BAD_FRAME", [-32601]: "UNKNOWN_PROC", [-32602]: "INVALID_ARGS", [-32603]: "INTERNAL" };

/**
 * Foreign-protocol link: translates call frames to JSON-RPC 2.0 over HTTP and the responses back to frames.
 * JSON-RPC has no chunks or events. `token` credentials travel as a Bearer header; pass `headersFor` for anything else.
 */
export function jsonRpcLink(url: string, opts: HttpOpts & { sessionHeader?: string; headersFor?: (frame: CallFrame) => Record<string, string> } = {}): Link {
  const doFetch = opts.fetch ?? fetch;
  const sessionHeader = opts.sessionHeader ?? "x-session-id";
  const headersFor =
    opts.headersFor ??
    ((frame: CallFrame): Record<string, string> =>
      frame.auth?.method === "token" && typeof frame.auth.credentials.token === "string"
        ? { authorization: `Bearer ${frame.auth.credentials.token}` }
        : {});
  return perCall(async (frame, signal, emit) => {
    const res = await doFetch(url, {
      method: "POST",
      body: JSON.stringify({ jsonrpc: "2.0", id: frame.id, method: frame.name, params: frame.args }),
      signal,
      headers: {
        "content-type": "application/json",
        ...(frame.sid !== undefined ? { [sessionHeader]: frame.sid } : {}),
        ...headersFor(frame),
        ...opts.headers,
      },
    });
    const body: unknown = await res.json();
    if (!isObj(body)) throw new Error(`HTTP ${res.status}`);
    const header = res.headers.get(sessionHeader);
    const sid = header === null ? undefined : header === SID_CLEARED ? null : header;
    const err = body.error;
    if (isObj(err)) {
      const data = isObj(err.data) ? err.data : {};
      const code = typeof data.code === "string" ? data.code : (FROM_JSONRPC[err.code as number] ?? "INTERNAL");
      return emit(frames.error(frame.id, code, String(err.message), data.data, sid));
    }
    emit(frames.result(frame.id, body.result ?? undefined, sid));
  });
}
