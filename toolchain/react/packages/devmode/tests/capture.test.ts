import { afterEach, expect, test, vi } from "vitest";
import { Capture, MAX_ENTRIES } from "../src/capture.ts";

/** A fake global with just what `install` patches. */
function fakeGlobal() {
  const calls: unknown[][] = [];
  const con = { log: (...a: unknown[]) => calls.push(["log", ...a]), info: () => {}, warn: () => {}, error: (...a: unknown[]) => calls.push(["error", ...a]), debug: () => {} };
  const target = new EventTarget() as EventTarget & { console: typeof con; fetch: typeof fetch };
  target.console = con;
  target.fetch = (async (_: unknown, init?: RequestInit) => {
    if (init?.method === "DELETE") throw new TypeError("offline");
    return new Response("ok", { status: 204 });
  }) as typeof fetch;
  return { target: target as unknown as typeof globalThis, con, calls };
}

let cap: Capture;
afterEach(() => cap?.uninstall());

test("console calls are recorded and still reach the original console", () => {
  cap = new Capture();
  const { target, calls } = fakeGlobal();
  cap.install(target);
  target.console.log("hi", { a: 1 });
  cap.flush();
  expect(calls).toEqual([["log", "hi", { a: 1 }]]);
  expect(cap.snapshot().map((e) => [e.type, e.args])).toEqual([["log", ["hi", { a: 1 }]]]);
});

test("uninstall restores console and fetch", () => {
  cap = new Capture();
  const { target, con } = fakeGlobal();
  const log = con.log, f = target.fetch;
  cap.install(target);
  expect(target.console.log).not.toBe(log);
  cap.uninstall();
  expect([target.console.log, target.fetch]).toEqual([log, f]);
});

test("listeners hear about entries once per batch, after the current task", async () => {
  cap = new Capture();
  const fn = vi.fn();
  cap.subscribe(fn);
  cap.push("log", [1]);
  cap.push("log", [2]);
  expect(fn).not.toHaveBeenCalled();
  await Promise.resolve();
  expect(fn).toHaveBeenCalledTimes(1);
  expect(cap.snapshot()).toHaveLength(2);
});

test("fetch is logged with method, status and body; failures keep the error and still throw", async () => {
  cap = new Capture();
  const { target } = fakeGlobal();
  cap.install(target);
  await target.fetch("/api/x", { method: "post", body: "{}", headers: { "X-A": "1" } });
  await expect(target.fetch("/api/y", { method: "DELETE" })).rejects.toThrow("offline");
  cap.flush();
  expect(cap.snapshot().map((e) => [e.net?.method, e.net?.url, e.net?.status, e.net?.requestBody, e.net?.error])).toEqual([
    ["POST", "/api/x", 204, "{}", undefined],
    ["DELETE", "/api/y", 0, undefined, "offline"],
  ]);
  expect(cap.snapshot()[0]!.net!.requestHeaders).toEqual([["X-A", "1"]]);
});

test("uncaught errors and unhandled rejections become error entries", () => {
  cap = new Capture();
  const { target } = fakeGlobal();
  cap.install(target);
  const boom = new Error("boom");
  target.dispatchEvent(Object.assign(new Event("error"), { error: boom, message: "boom" }));
  target.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason: "nope" }));
  cap.flush();
  expect(cap.snapshot().map((e) => [e.type, e.args])).toEqual([["error", ["Uncaught", boom]], ["error", ["Unhandled rejection:", "nope"]]]);
});

test("the store keeps the newest MAX_ENTRIES", () => {
  cap = new Capture();
  for (let i = 0; i < MAX_ENTRIES + 5; i++) cap.push("log", [i]);
  cap.flush();
  expect(cap.snapshot()).toHaveLength(MAX_ENTRIES);
  expect(cap.snapshot()[0]!.args).toEqual([5]);
});
