import { execFile } from "node:child_process";
import { dirname } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";
import { str } from "@coldmind/model";
import { contractOf, defineDecorator, eventsOf, expose, procedure, rpcEvent, strayMembers } from "@coldmind/rpc";
import type { Action } from "@coldmind/wire";
import { log, tagged, traced, watched } from "./decorate.fixture.ts";

test("standard: class, method and accessor decorators see one site shape", () => {
  log.length = 0;
  @tagged("standard")
  class Box {
    @watched() accessor caption = "hello";
    @watched() accessor count = 0;
    @traced() run() {}
  }
  const box = new Box();
  expect(box.caption).toBe("HELLO");
  box.caption = "bye";
  box.caption = "bye";
  box.count = 2;
  expect(box.caption).toBe("bye");
  expect(box.count).toBe(2);
  expect(new Box().caption).toBe("HELLO");
  expect(log.filter(l => !l.startsWith("set"))).toEqual(
    expect.arrayContaining(["class Box standard", "method Box.run", "field Box.caption", "field Box.count"]),
  );
  expect(log.filter(l => l.startsWith("set"))).toEqual(["set caption bye", "set count 2"]);
});

test("standard: a plain field can be given an initial value but not wrapped", () => {
  const init = defineDecorator("init", (site) => {
    if (site.kind !== "field") throw new TypeError("field only");
    return { init: () => 7 };
  });
  const wrap = defineDecorator("wrap", () => ({ get: (read: () => unknown) => read() }));
  @tagged("fields")
  class Plain {
    @init() value!: number;
  }
  expect(new Plain().value).toBe(7);
  expect(() => {
    class Bad {
      @wrap() value = 1;
    }
    return Bad;
  }).toThrow("accessor");
});

test("standard: members whose class has no class decorator are reported", () => {
  const before = strayMembers().length;
  class Stray {
    @traced() run() {}
  }
  expect(Stray).toBeDefined();
  expect(strayMembers().length).toBe(before + 1);
  expect(strayMembers()).toContain("run");
  @tagged("flush")
  class Flush {
    @traced() run() {}
  }
  expect(Flush).toBeDefined();
});

test("legacy: the same decorators under experimentalDecorators emit", { timeout: 60_000 }, async () => {
  const fixture = fileURLToPath(new URL("./legacy/legacy.fixture.ts", import.meta.url));
  const runner = fileURLToPath(new URL("../../../../scripts/run.script.mjs", import.meta.url));
  const { stdout, stderr } = await promisify(execFile)(process.execPath, [runner, fixture], { cwd: dirname(fixture) });
  expect(stderr).toBe("");
  const result = JSON.parse(stdout) as { before: string; after: string; count: number; other: string; log: string[]; procs: string[]; events: string[]; seen: unknown[] };
  expect(result.before).toBe("HELLO");
  expect(result.after).toBe("bye");
  expect(result.count).toBe(2);
  expect(result.other).toBe("HELLO");
  expect(result.log).toEqual(expect.arrayContaining(["class Box legacy", "method Box.run", "field Box.caption", "field Box.count", "set caption bye", "set count 2"]));
  expect(result.log.filter(l => l === "set caption bye")).toHaveLength(1);
  expect(result.procs).toEqual(["place"]);
  expect(result.events).toEqual(["created"]);
  expect(result.seen).toEqual([{ id: "1" }]);
});

test("standard: @expose, @procedure and @rpcEvent declare a contract and its events", () => {
  @expose("orders")
  class Orders {
    @rpcEvent({ id: str() }) readonly created!: Action<{ id: string }>;
    @rpcEvent({ id: str() }, { doc: "gone" }) readonly removed!: Action<{ id: string }>;
    @procedure({ args: {}, access: "public" }) place() {
      this.created.emit({ id: "1" });
    }
  }
  expect(Object.keys(contractOf(Orders).procs)).toEqual(["place"]);
  const events = eventsOf(Orders);
  expect(Object.keys(events)).toEqual(["created", "removed"]);
  expect(events.created!.name).toBe("orders.created");
  expect(events.removed!.doc).toBe("gone");
  const orders = new Orders();
  const seen: unknown[] = [];
  orders.created.onData(data => seen.push(data));
  orders.place();
  expect(seen).toEqual([{ id: "1" }]);
  expect(new Orders().created).not.toBe(orders.created);
  class Plain {}
  expect(eventsOf(Plain)).toEqual({});
});
