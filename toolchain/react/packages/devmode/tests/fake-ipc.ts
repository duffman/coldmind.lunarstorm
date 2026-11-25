/**
 * A stand-in for the desktop host's WebView IPC (platform/desktop): `win.ipc.postMessage(json)` receives envelopes and
 * answers through `win.__zebraReceive(envelope)` asynchronously, running RESP on the dev mock, one session per `open`.
 */
import { MockRedis, type Session } from "../dev/resp-mock.ts";
import { encodeReply, RespDecoder } from "../src/resp.ts";
import type { IpcEnvelope } from "../src/transport.ts";

/** Host behaviour switches. */
export interface HostOptions {
  /** Never answer (a host without a `resp` handler that drops the message). */
  silent?: boolean;
  /** Refuse every message with an `error` envelope, as today's host does for a WebView `request`. */
  refuse?: boolean;
}

/** UTF-8 text to base64. */
const b64 = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
/** Base64 to bytes. */
const bytes = (t: string) => Uint8Array.from(atob(t), (c) => c.charCodeAt(0));

/**
 * Builds a fake window with `ipc`, `zebra` and a shell `__zebraReceive` that records what reaches it.
 * @param redis shared keyspace
 * @param opts host behaviour
 * @return the window, every envelope posted to the host, and what the shell handler received
 */
export function fakeHost(redis = new MockRedis(), opts: HostOptions = {}) {
  const posted: IpcEnvelope[] = [];
  const shellGot: IpcEnvelope[] = [];
  const shell = (e: IpcEnvelope) => void shellGot.push(e);
  const conns = new Map<string, { s: Session; d: RespDecoder }>();
  const win: {
    ipc: { postMessage(text: string): void };
    __zebraReceive?: (e: IpcEnvelope) => void;
    zebra: { windowId: string; epoch: number };
  } = {
    zebra: { windowId: "Main", epoch: 3 },
    __zebraReceive: shell,
    ipc: {
      postMessage(text) {
        const env = JSON.parse(text) as IpcEnvelope;
        posted.push(env);
        if (opts.silent) return;
        const send = (kind: string, payload: unknown, id?: number) =>
          setTimeout(() => win.__zebraReceive?.({ version: 1, kind, id, windowId: "Main", epoch: 3, method: "resp", payload }));
        if (opts.refuse) return send("error", "kind Request not allowed here", env.id);
        const p = env.payload as { session: string; open?: boolean; data?: string; close?: boolean };
        if (p.open) {
          const s: Session = { proto: 2, channels: new Set(), patterns: new Set(), send: (r) => send("event", { session: p.session, data: b64(encodeReply(r, s.proto)) }) };
          conns.set(p.session, { s, d: new RespDecoder() });
          redis.sessions.add(s);
        } else if (p.data) {
          const c = conns.get(p.session)!;
          let out = "";
          for (const cmd of c.d.push(bytes(p.data))) {
            const args = cmd.t === "array" ? cmd.v.map((x) => (x.t === "bulk" ? x.v : "")) : [];
            for (const r of redis.exec(c.s, args)) out += encodeReply(r, c.s.proto);
          }
          if (out) send("response", { session: p.session, data: b64(out) }, env.id);
        } else if (p.close) {
          redis.sessions.delete(conns.get(p.session)!.s);
          conns.delete(p.session);
        }
      },
    },
  };
  return { win, posted, shell, shellGot };
}
