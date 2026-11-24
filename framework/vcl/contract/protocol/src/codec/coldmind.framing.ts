import { RpcError } from "@coldmind/rpc";
import type { Frame } from "@coldmind/protocol/src/frames/coldmind.protocol.ts";

/** An encoded frame: text for text codecs, bytes for binary ones. */
export type Wire = string | Uint8Array;

/**
 * Encoding slot: a frame is structure, the codec makes it bytes. `decode` throws on undecodable input.
 * `name` is what two ends agree on when they negotiate a codec (see `subprotocol`).
 */
export type Codec = { name: string; encode(frame: Frame): Wire; decode(wire: Wire): unknown };

/** WebSocket subprotocol (and TCP preamble token) naming this protocol version and a codec, e.g. `subzero.v1+json`. */
export const subprotocol = (codec: Codec): string => `subzero.v1+${codec.name}`;

/** Picks the codec for a list of offered subprotocol names, in the order of `codecs` (our preference). */
export function negotiate(codecs: readonly Codec[], offered: readonly string[]): Codec | undefined {
  return codecs.find(c => offered.includes(subprotocol(c)));
}

/** Text codec: JSON. The default everywhere; `Uint8Array` payloads do not survive it, use `msgpackCodec` for those. */
export const jsonCodec: Codec = {
  name: "json",
  encode: frame => JSON.stringify(frame),
  decode: wire => JSON.parse(typeof wire === "string" ? wire : new TextDecoder().decode(wire)),
};

/** Decoded structure, or undefined when the bytes are not decodable (a server answers that with BAD_FRAME). */
export function decodeOr(codec: Codec, wire: Wire): unknown {
  try {
    return codec.decode(wire);
  } catch {
    return undefined;
  }
}

/** Length-prefixed framing for byte-stream carriers (HTTP bodies, TCP): 4-byte big-endian length, then the payload. */
export function pack(wire: Wire): Uint8Array {
  const payload = typeof wire === "string" ? new TextEncoder().encode(wire) : wire;
  const out = new Uint8Array(4 + payload.byteLength);
  new DataView(out.buffer).setUint32(0, payload.byteLength);
  out.set(payload, 4);
  return out;
}

/**
 * The reading side of `pack`: reassembles length-prefixed payloads from a byte stream.
 * @throws {RpcError} BAD_FRAME (from `push`) when a declared length exceeds `maxFrameBytes` (default 16 MiB).
 */
export class Unpacker {
  private buf = new Uint8Array(0);
  constructor(private readonly maxFrameBytes = 16 * 1024 * 1024) {}

  /** Feeds bytes in whatever pieces the carrier delivers them; returns the complete payloads. */
  push(bytes: Uint8Array): Uint8Array[] {
    const joined = new Uint8Array(this.buf.byteLength + bytes.byteLength);
    joined.set(this.buf);
    joined.set(bytes, this.buf.byteLength);
    const out: Uint8Array[] = [];
    let at = 0;
    while (joined.byteLength - at >= 4) {
      const len = new DataView(joined.buffer, joined.byteOffset + at).getUint32(0);
      if (len > this.maxFrameBytes) throw new RpcError("BAD_FRAME", `Frame of ${len} bytes exceeds limit`);
      if (joined.byteLength - at - 4 < len) break;
      out.push(joined.slice(at + 4, at + 4 + len));
      at += 4 + len;
    }
    this.buf = joined.slice(at);
    return out;
  }
}
