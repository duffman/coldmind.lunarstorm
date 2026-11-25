import { expect, test } from "vitest";
import { buildRequest, envVars, execute, interpolate, newRequest, parseCurl, splitQuery, toCurl } from "../src/request.ts";

test("interpolate substitutes known variables and leaves unknown ones visible", () => {
  expect(interpolate("{{base}}/users/{{ id }}?x={{missing}}", { base: "https://api.test", id: "7" })).toBe("https://api.test/users/7?x={{missing}}");
});

test("envVars keeps enabled rows with a key only", () => {
  expect(envVars([{ key: "a", value: "1", enabled: true }, { key: "b", value: "2", enabled: false }, { key: "", value: "3", enabled: true }])).toEqual({ a: "1" });
});

test("buildRequest appends params to relative and query-bearing URLs and before the hash", () => {
  const params = [{ key: "q", value: "a b", enabled: true }, { key: "off", value: "x", enabled: false }];
  expect(buildRequest(newRequest({ url: "/api/users", params }), {}).url).toBe("/api/users?q=a%20b");
  expect(buildRequest(newRequest({ url: "/api?x=1#top", params }), {}).url).toBe("/api?x=1&q=a%20b#top");
});

test("buildRequest applies auth helpers and a default content type", () => {
  const base = newRequest({ method: "POST", url: "{{base}}/login", bodyMode: "json", body: '{"u":"{{user}}"}' });
  const bearer = buildRequest({ ...base, auth: { ...base.auth, kind: "bearer", token: "{{tok}}" } }, { base: "http://h", user: "ann", tok: "T" });
  expect(bearer).toEqual({
    method: "POST", url: "http://h/login", body: '{"u":"ann"}',
    headers: [["Authorization", "Bearer T"], ["Content-Type", "application/json"]],
  });
  const basic = buildRequest({ ...base, auth: { ...base.auth, kind: "basic", user: "åsa", pass: "p" } }, {});
  expect(basic.headers[0]).toEqual(["Authorization", "Basic w6VzYTpw"]);
});

test("buildRequest keeps a user content type, sends form bodies urlencoded and never a GET body", () => {
  const form = buildRequest(newRequest({ method: "PUT", bodyMode: "form", form: [{ key: "a", value: "1 2", enabled: true }], headers: [{ key: "content-type", value: "x/y", enabled: true }] }), {});
  expect(form.body).toBe("a=1+2");
  expect(form.headers).toEqual([["content-type", "x/y"]]);
  expect(buildRequest(newRequest({ method: "GET", bodyMode: "text", body: "x" }), {}).body).toBeUndefined();
});

test("execute reports status, headers, body, size; failures come back as status 0", async () => {
  const ok = (async () => new Response("héllo", { status: 201, statusText: "Created", headers: { "x-a": "1" } })) as typeof fetch;
  const res = await execute({ method: "GET", url: "/x", headers: [] }, undefined, ok);
  expect([res.status, res.statusText, res.body, res.size]).toEqual([201, "Created", "héllo", 6]);
  expect(res.headers).toContainEqual(["x-a", "1"]);
  const bad = (async () => { throw new TypeError("Failed to fetch"); }) as typeof fetch;
  const err = await execute({ method: "GET", url: "/x", headers: [] }, undefined, bad);
  expect(err.status).toBe(0);
  expect(err.error).toContain("Failed to fetch");
});

test("toCurl and parseCurl round-trip method, URL, headers, JSON body and query params", () => {
  const built = buildRequest(newRequest({
    method: "PATCH", url: "https://api.test/it's", params: [{ key: "v", value: "2", enabled: true }],
    headers: [{ key: "X-Trace", value: "a'b", enabled: true }], bodyMode: "json", body: '{"n":1}',
  }), {});
  const curl = toCurl(built);
  const back = parseCurl(curl)!;
  expect([back.method, back.url, back.params, back.headers, back.bodyMode, JSON.parse(back.body)]).toEqual([
    "PATCH", "https://api.test/it's", [{ key: "v", value: "2", enabled: true }],
    [{ key: "X-Trace", value: "a'b", enabled: true }, { key: "Content-Type", value: "application/json", enabled: true }],
    "json", { n: 1 },
  ]);
});

test("parseCurl reads browser-style commands: double quotes, continuations, -u, implied POST", () => {
  const req = parseCurl('curl "https://h/a" \\\n  -H "Accept: */*" --compressed -u bob:s3 --data-raw "x=1"')!;
  expect([req.method, req.url, req.headers.length, req.bodyMode, req.body, req.auth.kind, req.auth.user, req.auth.pass])
    .toEqual(["POST", "https://h/a", 1, "text", "x=1", "basic", "bob", "s3"]);
  expect(parseCurl("curl -s")).toBeNull();
});

test("splitQuery moves the query into rows and keeps the hash", () => {
  expect(splitQuery("/a?x=1&y=two%20w#h")).toEqual({ url: "/a#h", params: [{ key: "x", value: "1", enabled: true }, { key: "y", value: "two w", enabled: true }] });
});
