import type { Codec } from "@coldmind/protocol/src/codec/coldmind.framing.ts";

/**
 * Binary codec: MessagePack. Smaller than JSON on the wire and carries Uint8Array payloads as
 * they are. Like JSON it drops undefined object members and writes undefined elsewhere as nil.
 */
export const msgpackCodec: Codec = {
  name: "msgpack",
  encode: frame => encode(frame),
  decode: wire => {
    if (typeof wire === "string") throw new Error("msgpack expects binary data");
    return decode(wire);
  },
};

const MAX_DEPTH = 128;
const utf8 = new TextEncoder();
const utf8Decoder = new TextDecoder("utf-8", { fatal: true });

class Writer {
  private buf = new Uint8Array(256);
  private view = new DataView(this.buf.buffer);
  private len = 0;

  private room(n: number): void {
    if (this.len + n <= this.buf.length) return;
    const next = new Uint8Array(Math.max(this.buf.length * 2, this.len + n));
    next.set(this.buf.subarray(0, this.len));
    this.buf = next;
    this.view = new DataView(next.buffer);
  }
  u8(v: number): void {
    this.room(1);
    this.view.setUint8(this.len, v);
    this.len += 1;
  }
  u16(v: number): void {
    this.room(2);
    this.view.setUint16(this.len, v);
    this.len += 2;
  }
  u32(v: number): void {
    this.room(4);
    this.view.setUint32(this.len, v);
    this.len += 4;
  }
  i64(v: number): void {
    this.room(8);
    this.view.setBigInt64(this.len, BigInt(v));
    this.len += 8;
  }
  u64(v: number): void {
    this.room(8);
    this.view.setBigUint64(this.len, BigInt(v));
    this.len += 8;
  }
  f64(v: number): void {
    this.room(8);
    this.view.setFloat64(this.len, v);
    this.len += 8;
  }
  bytes(b: Uint8Array): void {
    this.room(b.byteLength);
    this.buf.set(b, this.len);
    this.len += b.byteLength;
  }
  done(): Uint8Array {
    return this.buf.slice(0, this.len);
  }
}

/**
 * Any JSON-like value, plus `Uint8Array` as bin, to MessagePack. Honours `toJSON`; NaN and the infinities become nil.
 * @throws {Error} On a function or symbol, or nesting deeper than 128.
 */
export function encode(value: unknown): Uint8Array {
  const w = new Writer();
  write(w, value, 0);
  return w.done();
}

function write(w: Writer, value: unknown, depth: number): void {
  if (depth > MAX_DEPTH) throw new Error("msgpack: value nested too deeply");
  if (value === null || value === undefined) return w.u8(0xc0);
  switch (typeof value) {
    case "boolean":
      return w.u8(value ? 0xc3 : 0xc2);
    case "number":
      return writeNumber(w, value);
    case "string": {
      const bytes = utf8.encode(value);
      const n = bytes.byteLength;
      if (n < 32) w.u8(0xa0 | n);
      else if (n < 0x100) (w.u8(0xd9), w.u8(n));
      else if (n < 0x10000) (w.u8(0xda), w.u16(n));
      else (w.u8(0xdb), w.u32(n));
      return w.bytes(bytes);
    }
    case "bigint":
      return writeNumber(w, Number(value));
    case "object":
      break;
    default:
      throw new Error(`msgpack: cannot encode a ${typeof value}`);
  }

  if (value instanceof Uint8Array) {
    const n = value.byteLength;
    if (n < 0x100) (w.u8(0xc4), w.u8(n));
    else if (n < 0x10000) (w.u8(0xc5), w.u16(n));
    else (w.u8(0xc6), w.u32(n));
    return w.bytes(value);
  }
  if (Array.isArray(value)) {
    const n = value.length;
    if (n < 16) w.u8(0x90 | n);
    else if (n < 0x10000) (w.u8(0xdc), w.u16(n));
    else (w.u8(0xdd), w.u32(n));
    for (const item of value) write(w, item, depth + 1);
    return;
  }
  const withJson = value as { toJSON?: () => unknown };
  if (typeof withJson.toJSON === "function") return write(w, withJson.toJSON(), depth + 1);

  const entries = Object.entries(value as Record<string, unknown>).filter(([, v]) => v !== undefined && typeof v !== "function");
  const n = entries.length;
  if (n < 16) w.u8(0x80 | n);
  else if (n < 0x10000) (w.u8(0xde), w.u16(n));
  else (w.u8(0xdf), w.u32(n));
  for (const [key, v] of entries) {
    write(w, key, depth + 1);
    write(w, v, depth + 1);
  }
}

function writeNumber(w: Writer, v: number): void {
  if (!Number.isSafeInteger(v)) {
    // Like JSON: NaN and the infinities have no representation and become nil.
    if (!Number.isFinite(v)) return w.u8(0xc0);
    return (w.u8(0xcb), w.f64(v));
  }
  if (v >= 0) {
    if (v < 0x80) return w.u8(v);
    if (v < 0x100) return (w.u8(0xcc), w.u8(v));
    if (v < 0x10000) return (w.u8(0xcd), w.u16(v));
    if (v < 0x100000000) return (w.u8(0xce), w.u32(v));
    return (w.u8(0xcf), w.u64(v));
  }
  if (v >= -32) return w.u8(0x100 + v);
  if (v >= -0x80) return (w.u8(0xd0), w.u8(0x100 + v));
  if (v >= -0x8000) return (w.u8(0xd1), w.u16(0x10000 + v));
  if (v >= -0x80000000) return (w.u8(0xd2), w.u32(0x100000000 + v));
  return (w.u8(0xd3), w.i64(v));
}

/**
 * One MessagePack value. Map keys must be strings; extension types are not supported.
 * @throws {Error} On truncated, trailing or unsupported data, invalid UTF-8, or nesting deeper than 128.
 */
export function decode(bytes: Uint8Array): unknown {
  const r = { view: new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength), bytes, at: 0 };
  const value = read(r, 0);
  if (r.at !== bytes.byteLength) throw new Error("msgpack: trailing bytes");
  return value;
}

type Reader = { view: DataView; bytes: Uint8Array; at: number };

function take(r: Reader, n: number): number {
  if (r.at + n > r.bytes.byteLength) throw new Error("msgpack: unexpected end of data");
  const at = r.at;
  r.at += n;
  return at;
}

function read(r: Reader, depth: number): unknown {
  if (depth > MAX_DEPTH) throw new Error("msgpack: value nested too deeply");
  const tag = r.view.getUint8(take(r, 1));

  if (tag < 0x80) return tag;
  if (tag >= 0xe0) return tag - 0x100;
  if (tag >= 0xa0 && tag <= 0xbf) return readString(r, tag & 0x1f);
  if (tag >= 0x90 && tag <= 0x9f) return readArray(r, tag & 0x0f, depth);
  if (tag >= 0x80 && tag <= 0x8f) return readMap(r, tag & 0x0f, depth);

  switch (tag) {
    case 0xc0:
      return null;
    case 0xc2:
      return false;
    case 0xc3:
      return true;
    case 0xc4:
      return readBinary(r, r.view.getUint8(take(r, 1)));
    case 0xc5:
      return readBinary(r, r.view.getUint16(take(r, 2)));
    case 0xc6:
      return readBinary(r, r.view.getUint32(take(r, 4)));
    case 0xca:
      return r.view.getFloat32(take(r, 4));
    case 0xcb:
      return r.view.getFloat64(take(r, 8));
    case 0xcc:
      return r.view.getUint8(take(r, 1));
    case 0xcd:
      return r.view.getUint16(take(r, 2));
    case 0xce:
      return r.view.getUint32(take(r, 4));
    case 0xcf:
      return Number(r.view.getBigUint64(take(r, 8)));
    case 0xd0:
      return r.view.getInt8(take(r, 1));
    case 0xd1:
      return r.view.getInt16(take(r, 2));
    case 0xd2:
      return r.view.getInt32(take(r, 4));
    case 0xd3:
      return Number(r.view.getBigInt64(take(r, 8)));
    case 0xd9:
      return readString(r, r.view.getUint8(take(r, 1)));
    case 0xda:
      return readString(r, r.view.getUint16(take(r, 2)));
    case 0xdb:
      return readString(r, r.view.getUint32(take(r, 4)));
    case 0xdc:
      return readArray(r, r.view.getUint16(take(r, 2)), depth);
    case 0xdd:
      return readArray(r, r.view.getUint32(take(r, 4)), depth);
    case 0xde:
      return readMap(r, r.view.getUint16(take(r, 2)), depth);
    case 0xdf:
      return readMap(r, r.view.getUint32(take(r, 4)), depth);
    default:
      throw new Error(`msgpack: unsupported type 0x${tag.toString(16)}`);
  }
}

function readString(r: Reader, n: number): string {
  const at = take(r, n);
  return utf8Decoder.decode(r.bytes.subarray(at, at + n));
}

function readBinary(r: Reader, n: number): Uint8Array {
  const at = take(r, n);
  return r.bytes.slice(at, at + n);
}

function readArray(r: Reader, n: number, depth: number): unknown[] {
  // Every element takes at least one byte, so a count beyond what is left is a lie, not a large array.
  if (n > r.bytes.byteLength - r.at) throw new Error("msgpack: unexpected end of data");
  const out: unknown[] = new Array(n);
  for (let i = 0; i < n; i++) out[i] = read(r, depth + 1);
  return out;
}

function readMap(r: Reader, n: number, depth: number): Record<string, unknown> {
  if (n * 2 > r.bytes.byteLength - r.at) throw new Error("msgpack: unexpected end of data");
  const out: Record<string, unknown> = {};
  for (let i = 0; i < n; i++) {
    const key = read(r, depth + 1);
    if (typeof key !== "string") throw new Error("msgpack: map keys must be strings");
    const value = read(r, depth + 1);
    // Plain assignment to "__proto__" would replace the prototype instead of creating a member.
    if (key === "__proto__") Object.defineProperty(out, key, { value, enumerable: true, writable: true, configurable: true });
    else out[key] = value;
  }
  return out;
}
