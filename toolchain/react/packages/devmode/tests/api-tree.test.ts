import { expect, test } from "vitest";
import { MockRedis, type Session } from "../dev/resp-mock.ts";
import { parseApiSpec, parseCommandDocs, savedCategories } from "../src/api-tree.tsx";
import { encodeReply, RespDecoder } from "../src/resp.ts";

const docs = () => {
  const s: Session = { proto: 3, channels: new Set(), patterns: new Set(), send: () => {} };
  return new MockRedis().exec(s, ["COMMAND", "DOCS"])[0]!;
};

test("COMMAND DOCS becomes categories of calls with templates and synopses", () => {
  const cats = parseCommandDocs(docs());
  expect(cats.map((c) => c.label)).toEqual(["Connection", "Hashes", "Keys", "Lists", "Pub/Sub", "Server", "Sets", "Strings"]);
  const hashes = cats.find((c) => c.label === "Hashes")!;
  expect(hashes.id).toBe("s:hash");
  expect(hashes.calls.map((c) => c.name)).toEqual(["HDEL", "HGET", "HGETALL", "HKEYS", "HSET"]);
  expect(hashes.calls.find((c) => c.name === "HSET")).toEqual({
    name: "HSET", command: "HSET key field value", args: "key field value [field value ...]", summary: "Creates or modifies the value of a field in a hash.",
  });
  const conn = cats.find((c) => c.label === "Connection")!.calls;
  expect(conn.find((c) => c.name === "PING")).toMatchObject({ command: "PING", args: "[message]" });
  expect(cats.find((c) => c.label === "Keys")!.calls.find((c) => c.name === "DEL")).toMatchObject({ command: "DEL key", args: "key [key ...]" });
});

test("the RESP2 form of COMMAND DOCS (flat arrays) parses the same", () => {
  const resp2 = new RespDecoder().push(new TextEncoder().encode(encodeReply(docs(), 2)))[0]!;
  expect(resp2.t).toBe("array");
  expect(parseCommandDocs(resp2)).toEqual(parseCommandDocs(docs()));
  expect(parseCommandDocs({ t: "error", v: "ERR unknown command 'COMMAND'" })).toEqual([]);
});

test("saved calls group by category in saving order; a multi-line call is a pipeline", () => {
  const cats = savedCategories([
    { category: "Users", command: "HGETALL user:1" },
    { category: "Saved", command: "PING" },
    { category: "Users", command: "HSET user:2 name Bo\nHGETALL user:2" },
  ]);
  expect(cats.map((c) => [c.id, c.saved, c.calls.map((k) => `${k.name}|${k.args}|${k.summary}`)])).toEqual([
    ["u:Users", true, ["HGETALL|user:1|", "HSET|user:2 name Bo|Pipeline of 2 commands"]],
    ["u:Saved", true, ["PING||"]],
  ]);
});

test("a HostApi spec becomes one category per area with AREA.FUNCTION calls; other versions are ignored", () => {
  const spec = { version: 1, areas: [{ name: "ui", module: "lunarstorm:hostapi/ui", functions: [
    { name: "spinner", summary: "Starts a spinner.", args: [{ name: "label", optional: true, multiple: false }] },
    { name: "progress", summary: "", args: [{ name: "total", optional: false, multiple: false }, { name: "label", optional: true, multiple: true }] },
  ] }] };
  expect(parseApiSpec(spec)).toEqual([{ id: "a:ui", label: "Ui", saved: false, calls: [
    { name: "UI.PROGRESS", command: "UI.PROGRESS total", args: "total [label [label ...]]", summary: "" },
    { name: "UI.SPINNER", command: "UI.SPINNER", args: "[label]", summary: "Starts a spinner." },
  ] }]);
  expect(parseApiSpec({ version: 2, areas: spec.areas })).toEqual([]);
  expect(parseApiSpec(null)).toEqual([]);
});
