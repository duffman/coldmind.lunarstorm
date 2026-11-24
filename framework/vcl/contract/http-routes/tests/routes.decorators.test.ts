import { spawn } from "node:child_process";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";
import { str } from "@coldmind/model";
import { del, get, httpController, httpRoute, patch, post, put, routeHandlersOf, routesOf } from "@coldmind/http-routes";

@httpController("orders", { doc: "Order handling" })
class Orders {
  constructor(private readonly prefix: string) {}

  @get("/orders/:id", { params: { id: str() }, access: "public", doc: "Fetches one." })
  get({ params }: { params: { id: string } }) {
    return `${this.prefix}${params.id}`;
  }

  @post("/orders", { body: { sku: str() }, status: 201 })
  create() {}

  @put("/orders/:id", { params: { id: str() } })
  replace() {}

  @patch("/orders/:id", { params: { id: str() } })
  amend() {}

  @del("/orders/:id", { params: { id: str() }, idempotent: true })
  remove() {}

  @httpRoute({ method: "GET", path: "/orders/feed", stream: "sse" })
  feed() {}
}

test("a decorated class declares its route group", () => {
  const c = routesOf(Orders);
  expect(c.name).toBe("orders");
  expect(c.doc).toBe("Order handling");
  expect(Object.keys(c.routes)).toEqual(["get", "create", "replace", "amend", "remove", "feed"]);
  expect(c.routes.get).toMatchObject({ method: "GET", path: "/orders/:id", access: "public", doc: "Fetches one." });
  expect(c.routes.create).toMatchObject({ method: "POST", status: 201 });
  expect([c.routes.replace!.method, c.routes.amend!.method, c.routes.remove!.method]).toEqual(["PUT", "PATCH", "DELETE"]);
  expect(c.routes.feed).toMatchObject({ stream: "sse" });
});

test("handlers are bound to the instance", async () => {
  const handlers = routeHandlersOf(new Orders("order-"));
  expect(Object.keys(handlers)).toEqual(["get", "create", "replace", "amend", "remove", "feed"]);
  expect(await handlers.get!({ params: { id: "7" } }, {})).toBe("order-7");
});

test("a subclass adds routes and may redeclare one", () => {
  @httpController("orders2")
  class More extends Orders {
    @get("/orders/mine", { access: "public" })
    mine() {}

    @get("/orders/:id", { params: { id: str() }, access: { roles: ["admin"] } })
    override get() {
      return "x";
    }
  }
  const c = routesOf(More);
  expect(c.name).toBe("orders2");
  expect(Object.keys(c.routes)).toEqual(["get", "create", "replace", "amend", "remove", "feed", "mine"]);
  expect(c.routes.get!.access).toEqual({ roles: ["admin"] });
  expect(routesOf(Orders).routes.get!.access).toBe("public");
});

test("misuse fails loudly", () => {
  class Plain {}
  expect(() => routesOf(Plain)).toThrow("not decorated with @httpController");
  expect(() => routeHandlersOf(new Plain())).toThrow("not decorated");
  expect(() => {
    @httpController("bad name")
    class Bad {
      @get("/x")
      x() {}
    }
    return routesOf(Bad);
  }).toThrow("Invalid contract name");
  expect(() => {
    @httpController("withdup")
    class Dup {
      @get("/a/:id", { params: { nope: str() } })
      a() {}
    }
    return routesOf(Dup);
  }).toThrow("must equal the path parameters");
});

// The fixture runs as a script under its own experimentalDecorators tsconfig: the repo's script runner applies that tsconfig on Node and Bun alike.
test("under experimentalDecorators emit the same class declares the same group", async () => {
  const fixture = fileURLToPath(new URL("./legacy/legacy.fixture.ts", import.meta.url));
  const runner = fileURLToPath(new URL("../../../../scripts/run.script.mjs", import.meta.url));
  const run = spawn(process.execPath, [runner, fixture], { cwd: dirname(fixture) });
  let out = "";
  let err = "";
  run.stdout.setEncoding("utf8").on("data", chunk => { out += chunk; });
  run.stderr.setEncoding("utf8").on("data", chunk => { err += chunk; });
  const status = await new Promise<number | null>(resolve => run.on("close", resolve));
  expect(err).toBe("");
  expect(status).toBe(0);
  expect(JSON.parse(out)).toEqual({ name: "notes", routes: ["list", "add"], methods: ["GET", "POST"], doc: "Notes", listed: "base-1" });
});
