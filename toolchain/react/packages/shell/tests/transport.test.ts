import { afterEach, beforeEach, expect, test } from "vitest";

import { ShellTransport, defaultSocketUrl } from "@coldmind/devtools-shell/src/transport.ts";
import { FakeSocket, fakeWebSocket, tick } from "./fake-socket.ts";

let transport: ShellTransport;
const reloads: number[] = [];

beforeEach(() => {
  FakeSocket.reset();
  reloads.length = 0;
});
afterEach(() => transport?.close());

function make(fetch: () => Promise<unknown> = () => Promise.resolve()) {
  transport = new ShellTransport({
    url: "ws://localhost:5173/__devtools_hmr",
    WebSocket: fakeWebSocket,
    fetch,
    reload: () => reloads.push(1),
    retryMs: 5,
  });
  transport.start();
  return FakeSocket.last();
}

test("default URL follows the page protocol and host", () => {
  expect(defaultSocketUrl({ protocol: "http:", host: "a:1" })).toBe("ws://a:1/__devtools_hmr");
  expect(defaultSocketUrl({ protocol: "https:", host: "b" })).toBe("wss://b/__devtools_hmr");
});

test("queues sends until open, then flushes in order", () => {
  const socket = make();
  expect(transport.status).toBe("connecting");
  transport.send("app:a", 1);
  transport.channel("a").send(2);
  expect(socket.sent).toEqual([]);
  socket.open();
  expect(transport.status).toBe("open");
  expect(socket.sent).toEqual([
    { type: "custom", event: "app:a", data: 1 },
    { type: "custom", event: "app:a", data: 2 },
  ]);
});

test("sendNow does not queue while closed", () => {
  const socket = make();
  expect(transport.sendNow("vite:invalidate", {})).toBe(false);
  socket.open();
  expect(transport.sendNow("vite:invalidate", { path: "/a" })).toBe(true);
  expect(socket.sent).toEqual([{ type: "custom", event: "vite:invalidate", data: { path: "/a" } }]);
});

test("drops sends past the queue limit", () => {
  transport = new ShellTransport({ url: "ws://x/", WebSocket: fakeWebSocket, maxQueue: 2 });
  transport.start();
  expect([transport.send("e", 1), transport.send("e", 2), transport.send("e", 3)]).toEqual([true, true, false]);
});

test("frames reach frame listeners; custom frames also reach their event and channel", () => {
  const socket = make();
  socket.open();
  const frames: string[] = [];
  const chat: unknown[] = [];
  transport.onFrame((frame) => frames.push(frame.type));
  const off = transport.channel<{ text: string }>("chat").subscribe((data) => chat.push(data));
  socket.frame({ type: "update", updates: [] });
  socket.frame({ type: "custom", event: "app:chat", data: { text: "hi" } });
  socket.frame({ type: "custom", event: "app:other", data: 1 });
  socket.dispatchEvent(new MessageEvent("message", { data: "not json" }));
  off();
  socket.frame({ type: "custom", event: "app:chat", data: { text: "late" } });
  expect(frames).toEqual(["update", "custom", "custom", "custom"]);
  expect(chat).toEqual([{ text: "hi" }]);
});

test("a first connection that fails is retried", async () => {
  const first = make();
  first.close();
  await tick(20);
  expect(FakeSocket.all.length).toBeGreaterThan(1);
  expect(transport.status).toBe("connecting");
  expect(reloads).toEqual([]);
});

test("losing an open connection probes until the server answers, then reloads", async () => {
  let failures = 2;
  const probes: string[] = [];
  const socket = make(async () => {
    probes.push("probe");
    if (failures-- > 0) throw new Error("down");
  });
  const statuses: string[] = [];
  transport.onStatus((s) => statuses.push(s));
  socket.open();
  socket.close();
  expect(transport.status).toBe("lost");
  await tick(40);
  expect(probes.length).toBe(3);
  expect(reloads).toEqual([1]);
  expect(statuses).toEqual(["open", "lost"]);
});

test("close stops retries and further sends", async () => {
  const socket = make();
  transport.close();
  expect(socket.closed).toBe(true);
  expect(transport.status).toBe("closed");
  await tick(20);
  expect(FakeSocket.all.length).toBe(1);
  expect(transport.send("e", 1)).toBe(false);
});
