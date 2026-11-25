import { expect, test } from "vitest";
import { MockRedis } from "../dev/resp-mock.ts";
import { RedisClient } from "../src/redis-client.ts";
import { replyText, type Reply } from "../src/resp.ts";
import { ipcTransport, transportFor } from "../src/transport.ts";
import { fakeHost } from "./fake-ipc.ts";

const text = (rs: Reply[]) => rs.map(replyText);
const tick = () => new Promise((r) => setTimeout(r, 5));

test("RESP runs over the host envelope: request envelopes out, __zebraReceive in, pub/sub across sessions", async () => {
  const { win, posted } = fakeHost();
  const a = new RedisClient(ipcTransport("resp", win));
  const b = new RedisClient(ipcTransport("resp", win));
  expect(a.url).toBe("ipc://resp");
  expect(text(await a.send([["SET", "k", "v"], ["GET", "k"], ["SUBSCRIBE", "news"]]))).toEqual(["OK", '"v"', '1) "subscribe"\n2) "news"\n3) (integer) 1']);
  expect(a.info).toEqual({ proto: 3, server: "lunarstorm-mock", version: "0.1.0" });
  const pushes: Reply[] = [];
  a.onPush((r) => pushes.push(r));
  expect(text(await b.send([["PUBLISH", "news", "hi"]]))).toEqual(["(integer) 1"]);
  await tick();
  expect(text(pushes)).toEqual(['1) "message"\n2) "news"\n3) "hi"']);
  const first = posted[0]!;
  expect({ ...first, payload: undefined }).toEqual({ version: 1, kind: "request", id: 1, windowId: "Main", epoch: 3, method: "resp", payload: undefined });
  expect(first.payload).toEqual({ session: expect.any(String), open: true });
  expect(posted[1]!.payload).toEqual({ session: (first.payload as { session: string }).session, data: expect.any(String) });
});

test("other envelopes still reach the shell's handler, and closing restores it", async () => {
  const { win, posted, shell, shellGot } = fakeHost();
  const c = new RedisClient(ipcTransport("resp", win));
  await c.send([["PING"]]);
  win.__zebraReceive!({ version: 1, kind: "update", windowId: "Main", epoch: 3, method: "update", payload: { controls: {} } });
  expect(shellGot.map((e) => e.kind)).toEqual(["update"]);
  c.close();
  await tick();
  expect(win.__zebraReceive).toBe(shell);
  expect(posted.at(-1)!.payload).toMatchObject({ close: true });
  expect(c.state).toBe("closed");
});

test("connect fails clearly without window.ipc, without a reply, and when the host refuses", async () => {
  await expect(new RedisClient(ipcTransport("resp", {})).send([["PING"]])).rejects.toThrow("window.ipc is missing");
  const silent = new RedisClient(ipcTransport("resp", fakeHost(new MockRedis(), { silent: true }).win), { helloTimeout: 30 });
  await expect(silent.send([["PING"]])).rejects.toThrow("no reply to HELLO within 30 ms");
  expect(silent.state).toBe("error");
  const refused = new RedisClient(ipcTransport("resp", fakeHost(new MockRedis(), { refuse: true }).win));
  await expect(refused.send([["PING"]])).rejects.toThrow("host refused the message: kind Request not allowed here");
});

test("the endpoint scheme picks the transport", () => {
  expect(transportFor("ipc://resp").endpoint).toBe("ipc://resp");
  expect(transportFor("ipc://cache").endpoint).toBe("ipc://cache");
  expect(transportFor("ws://h/resp").endpoint).toBe("ws://h/resp");
});
