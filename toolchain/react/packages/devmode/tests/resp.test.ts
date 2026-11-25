import { expect, test } from "vitest";
import { encodeCommand, encodeReply, parseCommands, RespDecoder, RespProtocolError, replyText, tokenize, type Reply } from "../src/resp.ts";

const utf8 = (s: string) => new TextEncoder().encode(s);
const b = (v: string): Reply => ({ t: "bulk", v });

test("commands encode as arrays of bulk strings with byte lengths", () => {
  expect(new TextDecoder().decode(encodeCommand(["SET", "k", "héllo"]))).toBe("*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$6\r\nhéllo\r\n");
});

test("every RESP3 type round-trips, fed one byte at a time", () => {
  const replies: Reply[] = [
    { t: "simple", v: "OK" }, { t: "error", v: "ERR boom" }, { t: "int", v: "-9007199254740993" }, b("två\r\nrader"), { t: "nil" },
    { t: "double", v: "1.5" }, { t: "bool", v: true }, { t: "big", v: "3492890328409238509324850943850943825024385" },
    { t: "verbatim", format: "txt", v: "Some string" }, { t: "array", v: [b("a"), { t: "array", v: [] }] },
    { t: "set", v: [b("x")] }, { t: "push", v: [b("message"), b("news"), b("hi")] }, { t: "map", v: [[b("k"), { t: "int", v: "1" }]] },
  ];
  const bytes = utf8(replies.map((r) => encodeReply(r, 3)).join(""));
  const d = new RespDecoder();
  const out: Reply[] = [];
  for (const byte of bytes) out.push(...d.push(Uint8Array.of(byte)));
  expect(out).toEqual(replies);
});

test("RESP2 encoding downgrades maps, null and booleans; attributes are dropped", () => {
  expect(encodeReply({ t: "map", v: [[b("a"), b("1")]] }, 2)).toBe("*2\r\n$1\r\na\r\n$1\r\n1\r\n");
  expect(encodeReply({ t: "nil" }, 2)).toBe("$-1\r\n");
  expect(encodeReply({ t: "bool", v: false }, 2)).toBe(":0\r\n");
  expect(new RespDecoder().push(utf8("|1\r\n+ttl\r\n:3\r\n:7\r\n*-1\r\n"))).toEqual([{ t: "int", v: "7" }, { t: "nil" }]);
});

test("bytes that are not RESP throw a protocol error", () => {
  expect(() => new RespDecoder().push(utf8("hello\r\n"))).toThrow(RespProtocolError);
});

test("tokenize follows redis-cli quoting", () => {
  expect(tokenize(`SET  "a b" 'it\\'s' "\\x41\\n" plain`)).toEqual(["SET", "a b", "it's", "A\n", "plain"]);
  expect(tokenize(`SET k ""`)).toEqual(["SET", "k", ""]);
  expect(() => tokenize(`SET "open`)).toThrow(/unbalanced/);
  expect(() => tokenize(`SET "a"b`)).toThrow(/unbalanced/);
  expect(parseCommands("# seed\nSET a 1\n\n  GET a  ")).toEqual([["SET", "a", "1"], ["GET", "a"]]);
  expect(() => parseCommands("PING\nSET 'x")).toThrow(/^Line 2:/);
});

test("replies print like redis-cli", () => {
  expect(replyText({ t: "array", v: [b("a"), { t: "array", v: [{ t: "int", v: "1" }, { t: "nil" }] }] })).toBe('1) "a"\n2) 1) (integer) 1\n   2) (nil)');
  expect(replyText({ t: "map", v: [[b("name"), b("Ann")], [b("tags"), { t: "set", v: [b("x"), b("y")] }]] })).toBe('1# "name" => "Ann"\n2# "tags" => 1~ "x"\n             2~ "y"');
  expect(replyText({ t: "array", v: Array.from({ length: 10 }, (_, i) => b(String(i))) }).split("\n")[0]).toBe(' 1) "0"');
  expect(replyText(b('say "hi"\n\x01'))).toBe('"say \\"hi\\"\\n\\x01"');
  expect(replyText({ t: "error", v: "ERR nope" })).toBe("(error) ERR nope");
  expect(replyText({ t: "array", v: [] })).toBe("(empty array)");
});
