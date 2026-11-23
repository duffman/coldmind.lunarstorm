import { expect, test } from "vitest";
import { compilePath, compareSpecificity, matchPath, InvalidPathError } from "@coldmind/http-routes";

test("literals, params and a wildcard compile; parameter names are listed in order", () => {
  const p = compilePath("/orders/:id/items/*rest");
  expect(p.params).toEqual(["id", "rest"]);
  expect(p.segments.map(s => s.kind)).toEqual(["literal", "param", "literal", "wildcard"]);
  expect(p.shape).toBe("/orders/:/items/*");
  expect(compilePath("/").segments).toEqual([]);
  expect(compilePath("/a/").pattern).toBe("/a");
});

test("malformed paths are rejected", () => {
  for (const bad of ["", "orders", "/a//b", "/a/:", "/a/:1x", "/a/:id/:id", "/a/*r/b", "/a?x=1", "/a#f", "/a b", "/a/%zz", "/*", "/:"])
    expect(() => compilePath(bad)).toThrow(TypeError);
});

test("matching extracts decoded params and ignores one trailing slash", () => {
  const p = compilePath("/orders/:id");
  expect(matchPath(p, "/orders/42")).toEqual({ id: "42" });
  expect(matchPath(p, "/orders/42/")).toEqual({ id: "42" });
  expect(matchPath(p, "/orders/a%20b")).toEqual({ id: "a b" });
  expect(matchPath(p, "/orders")).toBeNull();
  expect(matchPath(p, "/orders/")).toBeNull();
  expect(matchPath(p, "/orders/1/2")).toBeNull();
  expect(matchPath(p, "/other/1")).toBeNull();
  expect(matchPath(p, "orders/1")).toBeNull();
});

test("an encoded slash stays inside its segment and cannot create a new one", () => {
  const p = compilePath("/orders/:id");
  expect(matchPath(p, "/orders/a%2Fb")).toEqual({ id: "a/b" });
  expect(matchPath(p, "/orders/..%2F..%2Fx")).toEqual({ id: "../../x" });
  expect(matchPath(compilePath("/files/x"), "/files/x%2Fy")).toBeNull();
});

test("an empty middle segment never matches a param", () => {
  const p = compilePath("/a/:x/b");
  expect(matchPath(p, "/a//b")).toBeNull();
});

test("a literal is compared after decoding", () => {
  expect(matchPath(compilePath("/caf%C3%A9"), "/caf%C3%A9")).toEqual({});
  expect(matchPath(compilePath("/café"), "/caf%C3%A9")).toEqual({});
});

test("a wildcard takes one or more remaining segments, joined by /", () => {
  const p = compilePath("/files/*rest");
  expect(matchPath(p, "/files/a")).toEqual({ rest: "a" });
  expect(matchPath(p, "/files/a/b%20c/d")).toEqual({ rest: "a/b c/d" });
  expect(matchPath(p, "/files")).toBeNull();
  expect(matchPath(p, "/files/")).toBeNull();
  expect(matchPath(p, "/files/a//b")).toBeNull();
});

test("the root matches only the root", () => {
  const p = compilePath("/");
  expect(matchPath(p, "/")).toEqual({});
  expect(matchPath(p, "/a")).toBeNull();
});

test("invalid percent-encoding in a request path throws InvalidPathError", () => {
  expect(() => matchPath(compilePath("/a/:x"), "/a/%E0%A4%A")).toThrow(InvalidPathError);
});

test("specificity: literal beats param beats wildcard, left to right", () => {
  const order = (...paths: string[]) => paths.map(compilePath).sort(compareSpecificity).map(p => p.pattern);
  expect(order("/orders/:id", "/orders/new")).toEqual(["/orders/new", "/orders/:id"]);
  expect(order("/files/*r", "/files/a/b", "/files/:x/b")).toEqual(["/files/a/b", "/files/:x/b", "/files/*r"]);
  expect(order("/:a/new", "/orders/:b")).toEqual(["/orders/:b", "/:a/new"]);
});
