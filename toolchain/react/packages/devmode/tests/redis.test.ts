import { expect, test } from "vitest";
import { MockRedis, type Session } from "../dev/resp-mock.ts";
import { RedisClient } from "../src/redis-client.ts";
import { replyText, type Reply } from "../src/resp.ts";
import { fakeSocket } from "./fake-ws.ts";

const text = (rs: Reply[]) => rs.map(replyText);

test("connect negotiates RESP3 and a pipeline answers in order", async () => {
  const c = new RedisClient("ws://x/resp", { WebSocket: fakeSocket() });
  const replies = await c.send([["SET", "a", "1"], ["INCR", "a"], ["GET", "a"], ["HGETALL", "user:1"], ["GET", "missing"], ["LPUSH", "user:1", "x"]]);
  expect(c.state).toBe("open");
  expect(c.info).toEqual({ proto: 3, server: "lunarstorm-mock", version: "0.1.0" });
  expect(text(replies)).toEqual([
    "OK", "(integer) 2", '"2"',
    '1# "name" => "Ann Berg"\n2# "role" => "admin"\n3# "email" => "ann@example.com"',
    "(nil)", "(error) WRONGTYPE Operation against a key holding the wrong kind of value",
  ]);
});

test("SUBSCRIBE resolves with its confirmation and PUBLISH reaches the subscriber as a push", async () => {
  const WS = fakeSocket();
  const sub = new RedisClient("ws://x/resp", { WebSocket: WS });
  const pub = new RedisClient("ws://x/resp", { WebSocket: WS });
  const pushes: Reply[] = [];
  sub.onPush((r) => pushes.push(r));
  expect(text(await sub.send([["SUBSCRIBE", "news"]]))).toEqual(['1) "subscribe"\n2) "news"\n3) (integer) 1']);
  expect(text(await pub.send([["PUBLISH", "news", "hi"]]))).toEqual(["(integer) 1"]);
  await new Promise((r) => setTimeout(r, 5));
  expect(text(pushes)).toEqual(['1) "message"\n2) "news"\n3) "hi"']);
});

test("a RESP2-only server works; arrays that look like pub/sub are replies until subscribed", async () => {
  class Resp2 extends MockRedis {
    exec(s: Session, args: string[]): Reply[] {
      return args[0]?.toLowerCase() === "hello" ? [{ t: "error", v: "ERR unknown command 'HELLO'" }] : super.exec(s, args);
    }
  }
  const WS = fakeSocket(new Resp2());
  const c = new RedisClient("ws://x/resp", { WebSocket: WS });
  const pushes: Reply[] = [];
  c.onPush((r) => pushes.push(r));
  expect(text(await c.send([["RPUSH", "l", "message", "x", "y"], ["LRANGE", "l", "0", "-1"]]))).toEqual(["(integer) 3", '1) "message"\n2) "x"\n3) "y"']);
  expect(c.info).toEqual({ proto: 2 });
  expect(text(await c.send([["SUBSCRIBE", "a", "b"]]))).toEqual(['1) "subscribe"\n2) "a"\n3) (integer) 1']);
  await new RedisClient("ws://x/resp", { WebSocket: WS }).send([["PUBLISH", "b", "yo"]]);
  await new Promise((r) => setTimeout(r, 5));
  expect(text(pushes)).toEqual(['1) "subscribe"\n2) "b"\n3) (integer) 2', '1) "message"\n2) "b"\n3) "yo"']);
});

test("an unreachable endpoint rejects and reports the error state", async () => {
  const c = new RedisClient("ws://down/resp", { WebSocket: fakeSocket() });
  await expect(c.send([["PING"]])).rejects.toThrow("Could not connect to ws://down/resp");
  expect(c.state).toBe("error");
});

test("the mock answers unknown commands, wrong arity and globs like Redis", () => {
  const r = new MockRedis();
  const s: Session = { proto: 3, channels: new Set(), patterns: new Set(), send: () => {} };
  expect(text(r.exec(s, ["NOPE", "a"]))).toEqual(["(error) ERR unknown command 'NOPE', with args beginning with: 'a' "]);
  expect(text(r.exec(s, ["GET"]))).toEqual(["(error) ERR wrong number of arguments for 'get' command"]);
  expect(text(r.exec(s, ["KEYS", "*er*"]))).toEqual(['1) "counter"\n2) "user:1"']);
});
