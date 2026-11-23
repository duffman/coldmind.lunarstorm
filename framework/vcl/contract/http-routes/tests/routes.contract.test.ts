import { expect, test } from "vitest";
import { bool, model, num, str } from "@coldmind/model";
import { ProcObj2, procsOf } from "@coldmind/rpc";
import { RouteTable, coerceFields, describeRoutes, httpContract, procContractOf, type HttpContract } from "@coldmind/http-routes";

const orders = httpContract(
  "orders",
  {
    list: { method: "GET", path: "/orders", query: { limit: num().default(10), tags: str().optional() }, access: "public", doc: "Lists orders.\nNewest first." },
    get: { method: "GET", path: "/orders/:id", params: { id: str() }, result: { sku: str() }, access: "public" },
    latest: { method: "GET", path: "/orders/latest", access: "public" },
    create: { method: "POST", path: "/orders", body: { sku: str() }, status: 201, idempotent: true },
    feed: { method: "GET", path: "/orders/feed", stream: "sse", chunk: { sku: str() }, access: "public" },
  },
  { doc: "Order handling" },
);

test("a contract validates every route definition", () => {
  const bad = (route: object, message: string) => expect(() => httpContract("x", { r: route as never })).toThrow(message);
  bad({ method: "GET", path: "/a/:id", params: { other: str() } }, "must equal the path parameters");
  bad({ method: "GET", path: "/a/:id", params: { id: str() }, body: { a: str() } }, "cannot declare a body");
  bad({ method: "POST", path: "/a", status: 99 }, "invalid status");
  bad({ method: "POST", path: "/a", chunk: { a: str() } }, "chunk requires stream");
  bad({ method: "TRACE", path: "/a" }, "unsupported method");
  bad({ method: "GET", path: "a" }, "must start with");
  expect(() => httpContract("a/b", {})).toThrow("Invalid contract name");
  expect(() => httpContract("a", { "x+y": { method: "GET", path: "/" } })).toThrow("Invalid route name");
  expect(() => httpContract("a#", {})).toThrow(TypeError);
});

test("a model works as a params, query or body schema", () => {
  const Body = model("Body", { sku: str() });
  const c = httpContract("m", { make: { method: "POST", path: "/m/:id", params: model("P", { id: str() }), body: Body, access: "public" } });
  const proc = procsOf(procContractOf(c))[0]!;
  expect(proc.name).toBe("http.m.make");
});

test("procContractOf registers each route as http.<name>.<route> with assembled args", async () => {
  const c = procContractOf(orders);
  expect(c.name).toBe("http.orders");
  expect(c.doc).toBe("Order handling");
  const procs = procsOf(c);
  expect(procs.map(p => p.name)).toEqual(["http.orders.list", "http.orders.get", "http.orders.latest", "http.orders.create", "http.orders.feed"]);
  const obj = new ProcObj2(procs, (_proc, args) => args);
  expect(await obj.call("http.orders.list", { params: {}, query: {} })).toEqual({ params: {}, query: { limit: 10 } });
  expect(await obj.call("http.orders.get", { params: { id: "7" } })).toEqual({ params: { id: "7" } });
  // Without a params schema the path parameters are strings and are kept.
  const free = new ProcObj2(procsOf(procContractOf(httpContract("free", { r: { method: "GET", path: "/f/:a/*rest" } }))), (_p, a) => a);
  expect(await free.call("http.free.r", { params: { a: "1", rest: "x/y", junk: "z" } })).toEqual({ params: { a: "1", rest: "x/y" } });
  expect(await obj.call("http.orders.create", { params: {}, body: { sku: "A1", junk: 1 } })).toEqual({ params: {}, body: { sku: "A1" } });
  await expect(obj.call("http.orders.create", { params: {} })).rejects.toMatchObject({ code: "INVALID_ARGS" });
  await expect(obj.call("http.orders.get", { params: {} })).rejects.toMatchObject({ code: "INVALID_ARGS" });
});

test("access, doc, idempotent and noTimeout carry to the proc; result and chunk do not; a stream route gets noTimeout", () => {
  const procs = Object.fromEntries(procsOf(procContractOf(orders)).map(p => [p.name, p]));
  expect(procs["http.orders.create"]).toMatchObject({ idempotent: true });
  expect(procs["http.orders.create"]!.access).toBeUndefined();
  expect(procs["http.orders.get"]).toMatchObject({ access: "public" });
  expect(procs["http.orders.get"]!.result).toBeUndefined();
  expect(procs["http.orders.list"]!.doc).toContain("Lists orders.");
  expect(procs["http.orders.feed"]).toMatchObject({ noTimeout: true });
  expect(procs["http.orders.feed"]!.chunk).toBeUndefined();
});

test("a route body must be nestable: an opaque parse schema is rejected", () => {
  const c = httpContract("o", { r: { method: "POST", path: "/o", body: { parse: (x: unknown) => x } } });
  expect(() => procContractOf(c)).toThrow("opaque parse schema");
});

test("a field is accepted as a body", async () => {
  const c = httpContract("f", { r: { method: "POST", path: "/f", body: str().min(2), access: "public" } });
  const obj = new ProcObj2(procsOf(procContractOf(c)), (_p, a) => a);
  expect(await obj.call("http.f.r", { params: {}, body: "ok" })).toEqual({ params: {}, body: "ok" });
  await expect(obj.call("http.f.r", { params: {}, body: "x" })).rejects.toMatchObject({ code: "INVALID_ARGS" });
});

// ---- table ----

const table = new RouteTable([orders]);

test("the table matches by method and path; literal beats param", () => {
  const m = table.match("GET", "/orders/latest");
  expect(m).toMatchObject({ kind: "match", route: { key: "latest", topic: "http/orders/latest", proc: "http.orders.latest" }, params: {}, viaHead: false });
  expect(table.match("GET", "/orders/42")).toMatchObject({ kind: "match", route: { key: "get" }, params: { id: "42" } });
  expect(table.match("GET", "/orders/feed")).toMatchObject({ kind: "match", route: { key: "feed" } });
  expect(table.match("POST", "/orders")).toMatchObject({ kind: "match", route: { key: "create" } });
});

test("404, 405 with Allow, HEAD from GET, and bad paths", () => {
  expect(table.match("GET", "/nope")).toEqual({ kind: "not-found" });
  expect(table.match("DELETE", "/orders")).toEqual({ kind: "method-not-allowed", allow: ["GET", "HEAD", "POST"] });
  expect(table.match("POST", "/orders/42")).toEqual({ kind: "method-not-allowed", allow: ["GET", "HEAD"] });
  expect(table.match("HEAD", "/orders/42")).toMatchObject({ kind: "match", route: { key: "get" }, viaHead: true });
  expect(table.match("HEAD", "/orders")).toMatchObject({ viaHead: true, route: { key: "list" } });
  expect(table.match("GET", "/orders/%zz")).toEqual({ kind: "bad-path" });
});

test("the table refuses duplicates by method and path shape, and duplicate contract names", () => {
  const a = httpContract("a", { x: { method: "GET", path: "/p/:one" } });
  const b = httpContract("b", { y: { method: "GET", path: "/p/:two" } });
  expect(() => new RouteTable([a, b])).toThrow("Duplicate route GET /p/:two");
  expect(() => new RouteTable([a, a])).toThrow("Duplicate route contract");
  const c = httpContract("c", { z: { method: "POST", path: "/p/:one" } });
  expect(new RouteTable([a, c]).routes).toHaveLength(2);
});

test("a path is never a way to reach another route's topic", () => {
  const dangerous = ["/orders/a%2Fb", "/orders/%2B", "/orders/%23", "/orders/$SYS", "/orders/reply%2Fx", "/orders/..%2F..%2Fadmin"];
  for (const path of dangerous) {
    const m = table.match("GET", path);
    expect(m).toMatchObject({ kind: "match", route: { key: "get", topic: "http/orders/get" } });
  }
});

test("procContracts converts every contract for the server", () => {
  const t = new RouteTable([orders, httpContract("users", { me: { method: "GET", path: "/me", access: "public" } })]);
  expect(t.procContracts().map(c => c.name)).toEqual(["http.orders", "http.users"]);
});

// ---- coerce ----

test("coerceFields converts by field kind and leaves what does not convert", () => {
  const shape = { limit: num(), on: bool(), tags: str().optional(), ids: num().optional() };
  expect(coerceFields(shape, { limit: "5", on: "true", tags: "a" })).toEqual({ limit: 5, on: true, tags: "a" });
  expect(coerceFields(shape, { limit: "abc", on: "maybe" })).toEqual({ limit: "abc", on: "maybe" });
  expect(coerceFields(shape, { limit: "", on: "0" })).toEqual({ limit: "", on: false });
  expect(coerceFields(shape, { limit: ["1", "2"] })).toEqual({ limit: ["1", "2"] });
  expect(coerceFields(shape, { unknown: "x" })).toEqual({ unknown: "x" });
});

test("coerceFields turns one or several values into a list and coerces items", async () => {
  const { list } = await import("@coldmind/model");
  const shape = { ids: list(num()), names: list(str()) };
  expect(coerceFields(shape, { ids: "1", names: ["a", "b"] })).toEqual({ ids: [1], names: ["a", "b"] });
  expect(coerceFields(shape, { ids: ["1", "x"] })).toEqual({ ids: [1, "x"] });
});

// ---- describe ----

test("describeRoutes is JSON data with paths, topics, defaults and schemas", () => {
  const info = describeRoutes(orders as HttpContract);
  expect(info.name).toBe("orders");
  expect(info.doc).toBe("Order handling");
  expect(info.order).toEqual(["list", "get", "latest", "create", "feed"]);
  expect(info.routes.get).toMatchObject({ name: "orders.get", method: "GET", path: "/orders/:id", topic: "http/orders/get", access: "public", status: 200 });
  expect(info.routes.create).toMatchObject({ status: 201, idempotent: true, access: "authenticated" });
  expect(info.routes.feed).toMatchObject({ stream: "sse", noTimeout: true });
  expect(info.routes.list!.query).toMatchObject({ form: "shape" });
  expect(info.routes.latest!.body).toBeUndefined();
  expect(info.routes.list!.doc).toContain("Newest first.");
  expect(JSON.parse(JSON.stringify(info))).toEqual(info);
});
