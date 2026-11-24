// Run by decorate.test.ts under `experimentalDecorators` (this directory's tsconfig), so the legacy dialect is emitted.
import { str } from "@coldmind/model";
import { contractOf, eventsOf, expose, procedure, rpcEvent } from "@coldmind/rpc";
import type { Action } from "@coldmind/wire";
import { log, tagged, traced, watched } from "../decorate.fixture.ts";

@expose("orders")
class Orders {
  @rpcEvent({ id: str() }) readonly created!: Action<{ id: string }>;
  @procedure({ args: {}, access: "public" }) place() {
    this.created.emit({ id: "1" });
  }
}

const orders = new Orders();
const seen: unknown[] = [];
orders.created.onData(data => seen.push(data));
orders.place();

@tagged("legacy")
class Box {
  @watched() caption = "hello";
  @watched() count = 0;
  @traced() run() {}
}

const box = new Box();
const before = box.caption;
box.caption = "bye";
box.caption = "bye";
box.count = 2;
console.log(JSON.stringify({ before, after: box.caption, count: box.count, other: new Box().caption, log, procs: Object.keys(contractOf(Orders).procs), events: Object.keys(eventsOf(Orders)), seen }));
