/**
 * A WebSocket stand-in wired to the dev RESP mock, for tests. Every frame the "server" writes is delivered in two
 * chunks, so the client's decoder always sees replies split mid-frame. A URL containing "down" never opens.
 */
import { MockRedis, type Session } from "../dev/resp-mock.ts";
import { encodeReply, RespDecoder } from "../src/resp.ts";

/**
 * Builds a WebSocket class whose instances share one mock keyspace.
 * @param redis the keyspace (shared between sockets so PUBLISH reaches other connections)
 * @return a constructor usable as `typeof WebSocket`
 */
export function fakeSocket(redis = new MockRedis()): typeof WebSocket {
  class FakeWS {
    /** WebSocket ready state. */
    readyState = 0;
    /** Set by the client; ignored (frames are always ArrayBuffers). */
    binaryType = "blob";
    /** Event handlers the client assigns. */
    onopen: (() => void) | null = null;
    onmessage: ((e: { data: ArrayBuffer }) => void) | null = null;
    onclose: ((e: { reason: string }) => void) | null = null;
    /** Server-side state of this connection. */
    private session: Session;
    /** Server-side command decoder. */
    private decoder = new RespDecoder();

    /** @param url endpoint; "down" in it simulates an unreachable server */
    constructor(readonly url: string) {
      this.session = { proto: 2, channels: new Set(), patterns: new Set(), send: (r) => this.deliver(encodeReply(r, this.session.proto)) };
      redis.sessions.add(this.session);
      setTimeout(() => {
        if (url.includes("down")) {
          this.readyState = 3;
          this.onclose?.({ reason: "" });
          return;
        }
        this.readyState = 1;
        this.onopen?.();
      });
    }

    /** Runs the commands in `bytes` on the mock and delivers the replies. */
    send(bytes: Uint8Array) {
      let out = "";
      for (const c of this.decoder.push(bytes)) {
        const args = c.t === "array" ? c.v.map((x) => (x.t === "bulk" ? x.v : "")) : [];
        for (const r of redis.exec(this.session, args)) out += encodeReply(r, this.session.proto);
      }
      if (out) this.deliver(out);
    }

    /** Delivers RESP text in two chunks, asynchronously and in order. */
    private deliver(text: string) {
      const b = new TextEncoder().encode(text);
      const mid = Math.floor(b.length / 2);
      setTimeout(() => {
        this.onmessage?.({ data: b.slice(0, mid).buffer });
        this.onmessage?.({ data: b.slice(mid).buffer });
      });
    }

    /** Closes the connection. */
    close() {
      if (this.readyState === 3) return;
      this.readyState = 3;
      redis.sessions.delete(this.session);
      setTimeout(() => this.onclose?.({ reason: "" }));
    }
  }
  return FakeWS as unknown as typeof WebSocket;
}
