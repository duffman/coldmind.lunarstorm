/**
 * Dev-only stand-in for the app's Redis-protocol API: an in-memory keyspace with strings, hashes, lists, sets and
 * pub/sub, answering in RESP2 or RESP3 as negotiated by HELLO. Served at `/resp` by the Vite dev server (see
 * vite.config.ts) so the dev mode API tab has something to talk to; never part of a build.
 */
import type { Reply } from "../src/resp.ts";

/** A stored value. */
type Value =
  | { t: "string"; v: string }
  | { t: "hash"; v: Map<string, string> }
  | { t: "list"; v: string[] }
  | { t: "set"; v: Set<string> };

/** One client connection. */
export interface Session {
  /** Protocol chosen by HELLO; connections start in RESP2 like Redis. */
  proto: 2 | 3;
  /** Subscribed channels. */
  channels: Set<string>;
  /** Subscribed patterns. */
  patterns: Set<string>;
  /** Writes an out-of-band reply (a pub/sub message) to this connection. */
  send: (r: Reply) => void;
}

const ok: Reply = { t: "simple", v: "OK" };
const nil: Reply = { t: "nil" };
const bulk = (v: string): Reply => ({ t: "bulk", v });
const int = (n: number): Reply => ({ t: "int", v: String(n) });
const err = (v: string): Reply => ({ t: "error", v });
const arr = (v: string[]): Reply => ({ t: "array", v: v.map(bulk) });
const WRONGTYPE = err("WRONGTYPE Operation against a key holding the wrong kind of value");

/** Minimum argument count (name included) per command; Redis answers fewer with a wrong-arity error. */
const ARITY: Record<string, number> = {
  ping: 1, echo: 2, hello: 1, select: 2, set: 3, get: 2, del: 2, exists: 2, type: 2, keys: 2, dbsize: 1, flushall: 1, flushdb: 1,
  incr: 2, decr: 2, incrby: 3, hset: 4, hget: 3, hgetall: 2, hdel: 3, hkeys: 2, lpush: 3, rpush: 3, lrange: 4, llen: 2,
  sadd: 3, smembers: 2, publish: 3, subscribe: 2, psubscribe: 2, unsubscribe: 1, punsubscribe: 1, command: 2,
};

/**
 * One `COMMAND DOCS` argument, in Redis's shape: `{name, type, flags?, arguments?}`.
 * @param name argument name
 * @param type `key`, `string`, `integer`, `pattern`, `block`, …
 * @param flags any of `optional`, `multiple`
 * @param subs sub-arguments of a block
 */
function arg(name: string, type = "string", flags: string[] = [], subs: Reply[] = []): Reply {
  const v: [Reply, Reply][] = [[bulk("name"), bulk(name)], [bulk("type"), bulk(type)]];
  if (flags.length) v.push([bulk("flags"), { t: "array", v: flags.map((f) => ({ t: "simple", v: f })) }]);
  if (subs.length) v.push([bulk("arguments"), { t: "array", v: subs }]);
  return { t: "map", v };
}
const key = arg("key", "key");
const keys = arg("key", "key", ["multiple"]);

/** `COMMAND DOCS` entries: group, summary and arguments of every command the mock implements (texts as in Redis). */
const DOCS: Record<string, [group: string, summary: string, args: Reply[]]> = {
  ping: ["connection", "Returns the server's liveliness response.", [arg("message", "string", ["optional"])]],
  echo: ["connection", "Returns the given string.", [arg("message")]],
  hello: ["connection", "Handshakes with the Redis server.", [arg("protover", "integer", ["optional"])]],
  select: ["connection", "Changes the selected database.", [arg("index", "integer")]],
  set: ["string", "Sets the string value of a key, ignoring its type.", [key, arg("value")]],
  get: ["string", "Returns the string value of a key.", [key]],
  incr: ["string", "Increments the integer value of a key by one.", [key]],
  decr: ["string", "Decrements the integer value of a key by one.", [key]],
  incrby: ["string", "Increments the integer value of a key by a number.", [key, arg("increment", "integer")]],
  del: ["generic", "Deletes one or more keys.", [keys]],
  exists: ["generic", "Determines whether one or more keys exist.", [keys]],
  type: ["generic", "Determines the type of value stored at a key.", [key]],
  keys: ["generic", "Returns all key names that match a pattern.", [arg("pattern", "pattern")]],
  dbsize: ["server", "Returns the number of keys in the database.", []],
  flushall: ["server", "Removes all keys from all databases.", []],
  flushdb: ["server", "Removes all keys from the current database.", []],
  command: ["server", "Returns documentary information about commands.", [arg("subcommand", "pure-token"), arg("command-name", "string", ["optional", "multiple"])]],
  hset: ["hash", "Creates or modifies the value of a field in a hash.", [key, arg("data", "block", ["multiple"], [arg("field"), arg("value")])]],
  hget: ["hash", "Returns the value of a field in a hash.", [key, arg("field")]],
  hgetall: ["hash", "Returns all fields and values in a hash.", [key]],
  hkeys: ["hash", "Returns all fields in a hash.", [key]],
  hdel: ["hash", "Deletes one or more fields and their values from a hash.", [key, arg("field", "string", ["multiple"])]],
  lpush: ["list", "Prepends one or more elements to a list.", [key, arg("element", "string", ["multiple"])]],
  rpush: ["list", "Appends one or more elements to a list.", [key, arg("element", "string", ["multiple"])]],
  lrange: ["list", "Returns a range of elements from a list.", [key, arg("start", "integer"), arg("stop", "integer")]],
  llen: ["list", "Returns the length of a list.", [key]],
  sadd: ["set", "Adds one or more members to a set.", [key, arg("member", "string", ["multiple"])]],
  smembers: ["set", "Returns all members of a set.", [key]],
  publish: ["pubsub", "Posts a message to a channel.", [arg("channel"), arg("message")]],
  subscribe: ["pubsub", "Listens for messages published to channels.", [arg("channel", "string", ["multiple"])]],
  psubscribe: ["pubsub", "Listens for messages published to channels that match one or more patterns.", [arg("pattern", "pattern", ["multiple"])]],
  unsubscribe: ["pubsub", "Stops listening to messages posted to channels.", [arg("channel", "string", ["optional", "multiple"])]],
  punsubscribe: ["pubsub", "Stops listening to messages published to channels that match one or more patterns.", [arg("pattern", "pattern", ["optional", "multiple"])]],
};

/**
 * The `COMMAND DOCS` reply for some commands, or all when none are named; unknown names are left out, as in Redis.
 * @param names lower-case command names
 */
function commandDocs(names: string[]): Reply {
  const pick = names.length ? names.filter((n) => n in DOCS) : Object.keys(DOCS);
  return {
    t: "map",
    v: pick.map((n) => {
      const [group, summary, args] = DOCS[n]!;
      const doc: [Reply, Reply][] = [[bulk("summary"), bulk(summary)], [bulk("since"), bulk("1.0.0")], [bulk("group"), bulk(group)]];
      if (args.length) doc.push([bulk("arguments"), { t: "array", v: args }]);
      return [bulk(n), { t: "map", v: doc }];
    }),
  };
}

/**
 * Converts a Redis glob (`*`, `?`, `[…]`, `\x`) to an anchored RegExp.
 * @param glob the pattern
 */
export function globToRegExp(glob: string): RegExp {
  let re = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i]!;
    if (c === "*") re += ".*";
    else if (c === "?") re += ".";
    else if (c === "[") {
      const end = glob.indexOf("]", i + 1);
      if (end < 0) re += "\\[";
      else {
        re += `[${glob.slice(i + 1, end).replace(/\\/g, "\\\\")}]`;
        i = end;
      }
    } else if (c === "\\" && i + 1 < glob.length) re += `\\${glob[++i]}`;
    else re += c.replace(/[.+^${}()|\\/]/g, "\\$&");
  }
  return new RegExp(`^${re}$`, "s");
}

/** The keyspace and the connected sessions. */
export class MockRedis {
  /** All keys. */
  readonly data = new Map<string, Value>();
  /** Connected sessions, for PUBLISH. */
  readonly sessions = new Set<Session>();

  /** Seeds a few keys so `KEYS *` shows something on first use. */
  constructor() {
    this.data.set("greeting", { t: "string", v: "hello" });
    this.data.set("counter", { t: "string", v: "41" });
    this.data.set("user:1", { t: "hash", v: new Map([["name", "Ann Berg"], ["role", "admin"], ["email", "ann@example.com"]]) });
    this.data.set("jobs:email", { t: "list", v: ["welcome:7", "reset:12", "digest:3"] });
    this.data.set("tags", { t: "set", v: new Set(["alpha", "beta", "stable"]) });
  }

  /**
   * Runs one command.
   * @param s the calling session
   * @param args `[name, ...args]`
   * @return the replies to write, in order (SUBSCRIBE-family commands answer once per channel)
   */
  exec(s: Session, args: string[]): Reply[] {
    const name = (args[0] ?? "").toLowerCase();
    const min = ARITY[name];
    if (min === undefined) return [err(`ERR unknown command '${args[0] ?? ""}', with args beginning with: ${args.slice(1, 4).map((a) => `'${a}' `).join("")}`)];
    if (args.length < min) return [err(`ERR wrong number of arguments for '${name}' command`)];
    const [, a1 = "", a2 = ""] = args;
    const rest = args.slice(1);
    const typed = <T extends Value["t"]>(key: string, t: T): Extract<Value, { t: T }> | null | undefined => {
      const v = this.data.get(key);
      return v === undefined ? undefined : v.t === t ? (v as Extract<Value, { t: T }>) : null;
    };
    switch (name) {
      case "ping": return [args.length > 1 ? bulk(a1) : { t: "simple", v: "PONG" }];
      case "echo": return [bulk(a1)];
      case "command":
        if (a1.toLowerCase() !== "docs") return [err(`ERR unknown subcommand '${a1}'. Try COMMAND HELP.`)];
        return [commandDocs(args.slice(2).map((n) => n.toLowerCase()))];
      case "select": return [ok];
      case "hello": {
        if (a1 && a1 !== "2" && a1 !== "3") return [err("NOPROTO unsupported protocol version")];
        if (a1) s.proto = Number(a1) as 2 | 3;
        return [{ t: "map", v: [
          [bulk("server"), bulk("lunarstorm-mock")], [bulk("version"), bulk("0.1.0")], [bulk("proto"), int(s.proto)],
          [bulk("mode"), bulk("standalone")], [bulk("role"), bulk("master")], [bulk("modules"), { t: "array", v: [] }],
        ] }];
      }
      case "set": this.data.set(a1, { t: "string", v: a2 }); return [ok];
      case "get": {
        const v = typed(a1, "string");
        return [v === null ? WRONGTYPE : v ? bulk(v.v) : nil];
      }
      case "del": return [int(rest.filter((k) => this.data.delete(k)).length)];
      case "exists": return [int(rest.filter((k) => this.data.has(k)).length)];
      case "type": return [{ t: "simple", v: this.data.get(a1)?.t ?? "none" }];
      case "keys": {
        const re = globToRegExp(a1);
        return [arr([...this.data.keys()].filter((k) => re.test(k)).sort())];
      }
      case "dbsize": return [int(this.data.size)];
      case "flushall": case "flushdb": this.data.clear(); return [ok];
      case "incr": case "decr": case "incrby": {
        const v = typed(a1, "string");
        if (v === null) return [WRONGTYPE];
        const by = name === "incrby" ? Number(a2) : name === "incr" ? 1 : -1;
        const cur = v ? Number(v.v) : 0;
        if (!Number.isSafeInteger(cur) || !Number.isSafeInteger(by)) return [err("ERR value is not an integer or out of range")];
        this.data.set(a1, { t: "string", v: String(cur + by) });
        return [int(cur + by)];
      }
      case "hset": {
        if (rest.length % 2 === 0) return [err("ERR wrong number of arguments for 'hset' command")];
        const v = typed(a1, "hash");
        if (v === null) return [WRONGTYPE];
        const h = v?.v ?? new Map<string, string>();
        let added = 0;
        for (let i = 1; i < rest.length; i += 2) {
          if (!h.has(rest[i]!)) added++;
          h.set(rest[i]!, rest[i + 1]!);
        }
        this.data.set(a1, { t: "hash", v: h });
        return [int(added)];
      }
      case "hget": {
        const v = typed(a1, "hash");
        return [v === null ? WRONGTYPE : v?.v.has(a2) ? bulk(v.v.get(a2)!) : nil];
      }
      case "hgetall": {
        const v = typed(a1, "hash");
        return [v === null ? WRONGTYPE : { t: "map", v: [...(v?.v ?? [])].map(([k, x]) => [bulk(k), bulk(x)] as [Reply, Reply]) }];
      }
      case "hkeys": {
        const v = typed(a1, "hash");
        return [v === null ? WRONGTYPE : arr([...(v?.v.keys() ?? [])])];
      }
      case "hdel": {
        const v = typed(a1, "hash");
        if (v === null) return [WRONGTYPE];
        return [int(rest.slice(1).filter((f) => v?.v.delete(f)).length)];
      }
      case "lpush": case "rpush": {
        const v = typed(a1, "list");
        if (v === null) return [WRONGTYPE];
        const list = v?.v ?? [];
        for (const x of rest.slice(1)) name === "lpush" ? list.unshift(x) : list.push(x);
        this.data.set(a1, { t: "list", v: list });
        return [int(list.length)];
      }
      case "lrange": {
        const v = typed(a1, "list");
        if (v === null) return [WRONGTYPE];
        const list = v?.v ?? [];
        const at = (n: number) => (n < 0 ? Math.max(0, list.length + n) : n);
        return [arr(list.slice(at(Number(a2)), at(Number(args[3])) + 1))];
      }
      case "llen": {
        const v = typed(a1, "list");
        return [v === null ? WRONGTYPE : int(v?.v.length ?? 0)];
      }
      case "sadd": {
        const v = typed(a1, "set");
        if (v === null) return [WRONGTYPE];
        const set = v?.v ?? new Set<string>();
        const before = set.size;
        for (const x of rest.slice(1)) set.add(x);
        this.data.set(a1, { t: "set", v: set });
        return [int(set.size - before)];
      }
      case "smembers": {
        const v = typed(a1, "set");
        return [v === null ? WRONGTYPE : { t: "set", v: [...(v?.v ?? [])].map(bulk) }];
      }
      case "publish": {
        let receivers = 0;
        for (const peer of this.sessions) {
          if (peer.channels.has(a1)) {
            peer.send({ t: "push", v: [bulk("message"), bulk(a1), bulk(a2)] });
            receivers++;
          }
          for (const p of peer.patterns) {
            if (globToRegExp(p).test(a1)) {
              peer.send({ t: "push", v: [bulk("pmessage"), bulk(p), bulk(a1), bulk(a2)] });
              receivers++;
            }
          }
        }
        return [int(receivers)];
      }
      case "subscribe": case "psubscribe": case "unsubscribe": case "punsubscribe": {
        const set = name.startsWith("p") ? s.patterns : s.channels;
        const adding = !name.includes("un");
        const targets = adding || rest.length ? rest : [...set];
        const frame = (target: string | null): Reply => ({
          t: "push",
          v: [bulk(name), target === null ? nil : bulk(target), int(s.channels.size + s.patterns.size)],
        });
        if (!targets.length) return [frame(null)];
        return targets.map((t) => {
          if (adding) set.add(t);
          else set.delete(t);
          return frame(t);
        });
      }
    }
    return [err(`ERR unknown command '${args[0]}'`)];
  }
}
