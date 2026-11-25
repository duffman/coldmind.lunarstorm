/**
 * RESP2/RESP3 wire format: command encoding, an incremental reply decoder, reply encoding (for servers and tests),
 * the redis-cli command-line tokenizer, and redis-cli style reply formatting.
 */

/**
 * A decoded reply, tagged by wire type.
 * Decision: integers, doubles and big numbers stay strings, so 64-bit values and `inf`/`nan` print exactly as sent.
 */
export type Reply =
  | { t: "simple"; v: string }
  | { t: "error"; v: string }
  | { t: "int"; v: string }
  | { t: "bulk"; v: string }
  | { t: "nil" }
  | { t: "double"; v: string }
  | { t: "bool"; v: boolean }
  | { t: "big"; v: string }
  | { t: "verbatim"; format: string; v: string }
  | { t: "array" | "set" | "push"; v: Reply[] }
  | { t: "map"; v: [Reply, Reply][] };

/** One coloured run of text in a formatted reply; `c` is a style name (`str`, `num`, `err`, `idx`, …). */
export interface Seg {
  /** Style name, rendered as the class `lsdm-r-<c>`. */
  c: string;
  /** The text. */
  s: string;
}

/** One output line of a formatted reply. */
export type Line = Seg[];

/** Thrown on bytes that are not valid RESP; the connection cannot recover and should be closed. */
export class RespProtocolError extends Error {}

const utf8 = new TextEncoder();
const text = new TextDecoder();

/**
 * Encodes a command as a RESP array of bulk strings, the only form servers accept from clients.
 * @param args command name and arguments
 * @return the bytes to write
 */
export function encodeCommand(args: string[]): Uint8Array {
  let out = `*${args.length}\r\n`;
  for (const a of args) out += `$${utf8.encode(a).length}\r\n${a}\r\n`;
  return utf8.encode(out);
}

/**
 * Encodes a reply. With protocol 2, RESP3-only types are downgraded the way Redis does it:
 * map and set become flat arrays, null becomes `$-1`, booleans become `:1`/`:0`, doubles and big numbers become bulk strings.
 * @param r the reply
 * @param proto 2 or 3
 * @return the RESP text
 */
export function encodeReply(r: Reply, proto: 2 | 3 = 3): string {
  const bulk = (s: string) => `$${utf8.encode(s).length}\r\n${s}\r\n`;
  switch (r.t) {
    case "simple": return `+${r.v}\r\n`;
    case "error": return `-${r.v}\r\n`;
    case "int": return `:${r.v}\r\n`;
    case "bulk": return bulk(r.v);
    case "nil": return proto === 3 ? "_\r\n" : "$-1\r\n";
    case "double": return proto === 3 ? `,${r.v}\r\n` : bulk(r.v);
    case "bool": return proto === 3 ? `#${r.v ? "t" : "f"}\r\n` : `:${r.v ? 1 : 0}\r\n`;
    case "big": return proto === 3 ? `(${r.v}\r\n` : bulk(r.v);
    case "verbatim": return proto === 3 ? `=${utf8.encode(r.v).length + 4}\r\n${r.format}:${r.v}\r\n` : bulk(r.v);
    case "array": case "set": case "push": {
      const mark = proto === 2 ? "*" : r.t === "array" ? "*" : r.t === "set" ? "~" : ">";
      return `${mark}${r.v.length}\r\n${r.v.map((x) => encodeReply(x, proto)).join("")}`;
    }
    case "map":
      return proto === 3
        ? `%${r.v.length}\r\n${r.v.map(([k, v]) => encodeReply(k, proto) + encodeReply(v, proto)).join("")}`
        : `*${r.v.length * 2}\r\n${r.v.map(([k, v]) => encodeReply(k, proto) + encodeReply(v, proto)).join("")}`;
  }
}

/**
 * Incremental decoder: feed it bytes as they arrive, in any split, and it returns each reply once it is complete.
 * Decision: a partial reply is re-parsed from its start on the next chunk instead of keeping a resumable parse stack;
 * replies a dev console sees are small, and the restart keeps the parser a plain recursive function.
 */
export class RespDecoder {
  /** Bytes received but not yet consumed by a complete reply. */
  private buf = new Uint8Array(0);

  /**
   * Adds bytes and returns every reply they complete. RESP3 attributes (`|`) are read and dropped.
   * @param chunk the received bytes
   * @return complete replies, in order
   * @throws RespProtocolError on bytes that are not RESP
   */
  push(chunk: Uint8Array): Reply[] {
    const joined = new Uint8Array(this.buf.length + chunk.length);
    joined.set(this.buf);
    joined.set(chunk, this.buf.length);
    this.buf = joined;
    const out: Reply[] = [];
    let pos = 0;
    for (let r = this.parse(pos); r; r = this.parse(pos)) {
      pos = r[1];
      if (r[0]) out.push(r[0]);
    }
    this.buf = this.buf.slice(pos);
    return out;
  }

  /**
   * Reads one CRLF-terminated header line.
   * @param pos offset of the first byte after the type byte
   * @return the line text and the offset after CRLF, or null when the CRLF has not arrived
   */
  private line(pos: number): [string, number] | null {
    for (let i = pos; i + 1 < this.buf.length; i++) {
      if (this.buf[i] === 13 && this.buf[i + 1] === 10) return [text.decode(this.buf.subarray(pos, i)), i + 2];
    }
    return null;
  }

  /**
   * Parses one value at `pos`.
   * @param pos offset of a type byte
   * @return the value (null for a dropped attribute) and the offset after it, or null when more bytes are needed
   */
  private parse(pos: number): [Reply | null, number] | null {
    if (pos >= this.buf.length) return null;
    const type = String.fromCharCode(this.buf[pos]!);
    const head = this.line(pos + 1);
    if (!head) return null;
    const [h, next] = head;
    const count = () => {
      const n = Number(h);
      if (!Number.isInteger(n)) throw new RespProtocolError(`Protocol error: bad length ${JSON.stringify(h)} after '${type}'`);
      return n;
    };
    switch (type) {
      case "+": return [{ t: "simple", v: h }, next];
      case "-": return [{ t: "error", v: h }, next];
      case ":": return [{ t: "int", v: h }, next];
      case "_": return [{ t: "nil" }, next];
      case ",": return [{ t: "double", v: h }, next];
      case "#": return [{ t: "bool", v: h === "t" }, next];
      case "(": return [{ t: "big", v: h }, next];
      case "$": case "!": case "=": {
        const n = count();
        if (n < 0) return [{ t: "nil" }, next];
        if (this.buf.length < next + n + 2) return null;
        const s = text.decode(this.buf.subarray(next, next + n));
        const end = next + n + 2;
        if (type === "!") return [{ t: "error", v: s }, end];
        if (type === "=") return [{ t: "verbatim", format: s.slice(0, 3), v: s.slice(4) }, end];
        return [{ t: "bulk", v: s }, end];
      }
      case "*": case "~": case ">": {
        const n = count();
        if (n < 0) return [{ t: "nil" }, next];
        const items: Reply[] = [];
        let p = next;
        while (items.length < n) {
          const r = this.parse(p);
          if (!r) return null;
          p = r[1];
          if (r[0]) items.push(r[0]);
        }
        return [{ t: type === "*" ? "array" : type === "~" ? "set" : "push", v: items }, p];
      }
      case "%": case "|": {
        const n = count();
        const flat: Reply[] = [];
        let p = next;
        while (flat.length < n * 2) {
          const r = this.parse(p);
          if (!r) return null;
          p = r[1];
          if (r[0]) flat.push(r[0]);
        }
        if (type === "|") return [null, p];
        const pairs: [Reply, Reply][] = [];
        for (let i = 0; i < flat.length; i += 2) pairs.push([flat[i]!, flat[i + 1]!]);
        return [{ t: "map", v: pairs }, p];
      }
      default:
        throw new RespProtocolError(`Protocol error: unexpected byte ${JSON.stringify(type)}`);
    }
  }
}

/**
 * Splits one command line into arguments with redis-cli rules: whitespace separates; `"…"` supports
 * `\n \r \t \b \a \\ \" \xHH`; `'…'` supports only `\'`; a closing quote must be followed by whitespace or the end.
 * `\xHH` gives the code point U+00HH, which is sent UTF-8 encoded (bytes ≥ 0x80 become two bytes).
 * @param line the command line
 * @return the arguments
 * @throws Error on unbalanced quotes, like redis-cli's "Invalid argument(s)"
 */
export function tokenize(line: string): string[] {
  const args: string[] = [];
  const bad = () => new Error("Invalid argument(s): unbalanced quotes");
  let i = 0;
  while (i < line.length) {
    while (i < line.length && /\s/.test(line[i]!)) i++;
    if (i >= line.length) break;
    let cur = "";
    let quote: '"' | "'" | null = null;
    for (;;) {
      const c = line[i];
      if (quote === '"') {
        if (c === undefined) throw bad();
        if (c === "\\" && line[i + 1] === "x" && /^[0-9a-fA-F]{2}$/.test(line.slice(i + 2, i + 4))) {
          cur += String.fromCharCode(parseInt(line.slice(i + 2, i + 4), 16));
          i += 4;
        } else if (c === "\\" && i + 1 < line.length) {
          const e = line[i + 1]!;
          cur += ({ n: "\n", r: "\r", t: "\t", b: "\b", a: "\x07" } as Record<string, string>)[e] ?? e;
          i += 2;
        } else if (c === '"') {
          if (i + 1 < line.length && !/\s/.test(line[i + 1]!)) throw bad();
          i++;
          break;
        } else {
          cur += c;
          i++;
        }
      } else if (quote === "'") {
        if (c === undefined) throw bad();
        if (c === "\\" && line[i + 1] === "'") {
          cur += "'";
          i += 2;
        } else if (c === "'") {
          if (i + 1 < line.length && !/\s/.test(line[i + 1]!)) throw bad();
          i++;
          break;
        } else {
          cur += c;
          i++;
        }
      } else if (c === undefined || /\s/.test(c)) {
        break;
      } else if (c === '"' || c === "'") {
        quote = c;
        i++;
      } else {
        cur += c;
        i++;
      }
    }
    args.push(cur);
  }
  return args;
}

/**
 * Splits editor text into commands: one per non-blank line; lines starting with `#` are comments.
 * @param source editor text
 * @return tokenized commands, in order
 * @throws Error from `tokenize`, prefixed with the 1-based line number
 */
export function parseCommands(source: string): string[][] {
  const out: string[][] = [];
  source.split("\n").forEach((raw, n) => {
    const line = raw.trim();
    if (!line || line.startsWith("#")) return;
    try {
      out.push(tokenize(line));
    } catch (e) {
      throw new Error(`Line ${n + 1}: ${(e as Error).message}`);
    }
  });
  return out;
}

/**
 * Quotes a bulk string the way redis-cli does: double quotes, backslash escapes, `\xHH` for other control bytes.
 * @param s the string
 * @return the quoted form
 */
export function quote(s: string): string {
  const esc: Record<string, string> = { "\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t", "\x07": "\\a", "\b": "\\b" };
  return `"${s.replace(/[\\"\n\r\t\x07\b]|[\x00-\x1f\x7f]/g, (c) => esc[c] ?? `\\x${c.charCodeAt(0).toString(16).padStart(2, "0")}`)}"`;
}

/**
 * Lays out aggregate children with right-aligned indices (` 1)`, `10)`) and continuation lines indented under them.
 * @param children each child's lines
 * @param mark index suffix: `)` array, `~` set, `#` map
 */
function aggregate(children: Line[][], mark: string): Line[] {
  const width = String(children.length).length;
  return children.flatMap((lines, i) => {
    const prefix = `${String(i + 1).padStart(width)}${mark} `;
    return lines.map((l, j) => [{ c: "idx", s: j === 0 ? prefix : " ".repeat(prefix.length) }, ...l]);
  });
}

/**
 * Formats a reply as redis-cli prints it: `OK`, `(integer) 1`, `"text"`, `(nil)`, `(error) …`,
 * numbered nested arrays, `1# "key" => value` maps and `1~` sets.
 * @param r the reply
 * @return coloured lines
 */
export function formatReply(r: Reply): Line[] {
  switch (r.t) {
    case "simple": return [[{ c: "ok", s: r.v }]];
    case "error": return [[{ c: "err", s: `(error) ${r.v}` }]];
    case "int": return [[{ c: "type", s: "(integer) " }, { c: "num", s: r.v }]];
    case "double": return [[{ c: "type", s: "(double) " }, { c: "num", s: r.v }]];
    case "big": return [[{ c: "type", s: "(big number) " }, { c: "num", s: r.v }]];
    case "bool": return [[{ c: "num", s: r.v ? "(true)" : "(false)" }]];
    case "nil": return [[{ c: "nil", s: "(nil)" }]];
    case "bulk": return [[{ c: "str", s: quote(r.v) }]];
    case "verbatim": return r.v.replace(/\r?\n$/, "").split(/\r?\n/).map((l) => [{ c: "text", s: l }]);
    case "array": case "push":
      return r.v.length ? aggregate(r.v.map(formatReply), ")") : [[{ c: "nil", s: "(empty array)" }]];
    case "set":
      return r.v.length ? aggregate(r.v.map(formatReply), "~") : [[{ c: "nil", s: "(empty set)" }]];
    case "map":
      if (!r.v.length) return [[{ c: "nil", s: "(empty hash)" }]];
      return aggregate(r.v.map(([k, v]) => {
        const key = formatReply(k)[0] ?? [];
        const keyWidth = key.reduce((n, s) => n + s.s.length, 0) + 4;
        const [first = [], ...rest] = formatReply(v);
        return [[...key.map((s) => (s.c === "str" ? { ...s, c: "key" } : s)), { c: "idx", s: " => " }, ...first], ...rest.map((l) => [{ c: "idx", s: " ".repeat(keyWidth) }, ...l])];
      }), "#");
  }
}

/**
 * The plain text of `formatReply`, for copying and tests.
 * @param r the reply
 * @return redis-cli output text
 */
export function replyText(r: Reply): string {
  return formatReply(r).map((l) => l.map((s) => s.s).join("")).join("\n");
}
