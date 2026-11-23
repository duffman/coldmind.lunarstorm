import { afterEach, expect, test } from "vitest";

import {
  attachHotHost,
  createHotContext,
  handleHotFrame,
  isHotModuleLoaded,
  resetHotRuntime,
  updateStyle,
  type HotHost,
} from "@coldmind/devermind-shell/hot";
import type { HmrUpdate, ServerErrorPayload } from "@coldmind/devermind-shell/src/protocol.ts";

afterEach(() => resetHotRuntime());

interface Recorder {
  host: HotHost;
  sent: Array<[string, unknown]>;
  imported: string[];
  reloads: number;
  updated: string[];
  failed: string[];
  serverErrors: ServerErrorPayload[];
}

function host(modules: Record<string, unknown> = {}, overrides: Partial<HotHost> = {}): Recorder {
  const r: Recorder = { sent: [], imported: [], reloads: 0, updated: [], failed: [], serverErrors: [], host: undefined! };
  r.host = {
    send: (event, data) => (r.sent.push([event, data]), true),
    reload: () => void r.reloads++,
    importModule: async (url) => {
      r.imported.push(url);
      const key = url.split("?")[0]!;
      if (!(key in modules)) throw new Error(`failed to fetch ${key}`);
      return modules[key];
    },
    beforeUpdate: () => "apply",
    updated: (u) => void r.updated.push(u.path),
    updateFailed: (path) => void r.failed.push(path),
    serverError: (err) => void r.serverErrors.push(err),
    pagePath: () => "/",
    base: "/",
    ...overrides,
  };
  attachHotHost(r.host);
  return r;
}

const update = (path: string, extra: Partial<HmrUpdate> = {}): HmrUpdate => ({ type: "js-update", path, timestamp: 7, ...extra });

test("a self-accepting module re-imports with the timestamp and gets the new exports", async () => {
  const next = { value: 2 };
  const r = host({ "/src/a.ts": next });
  const hot = createHotContext("/src/a.ts?t=1");
  const received: unknown[] = [];
  const disposed: unknown[] = [];
  hot.data.count = 1;
  hot.dispose((data) => void disposed.push(data.count));
  hot.accept((mod) => received.push(mod));
  await handleHotFrame({ type: "update", updates: [update("/src/a.ts")] });
  expect(r.imported).toEqual(["/src/a.ts?t=7"]);
  expect(received).toEqual([next]);
  expect(disposed).toEqual([1]);
  expect(r.updated).toEqual(["/src/a.ts"]);
  expect(isHotModuleLoaded("/src/a.ts?x")).toBe(true);
});

test("a dependency boundary receives the accepted module in its dependency slot", async () => {
  const dep = { n: 1 };
  host({ "/src/dep.ts": dep });
  const hot = createHotContext("/src/parent.ts");
  const single: unknown[] = [];
  const many: unknown[] = [];
  hot.accept("/src/dep.ts?t=3", (mod) => single.push(mod));
  hot.accept(["/src/other.ts", "/src/dep.ts"], (mods) => many.push(mods));
  await handleHotFrame({ type: "update", updates: [update("/src/parent.ts", { acceptedPath: "/src/dep.ts" })] });
  expect(single).toEqual([dep]);
  expect(many).toEqual([[undefined, dep]]);
});

test("updates for modules this page never loaded are ignored", async () => {
  const r = host();
  await handleHotFrame({ type: "update", updates: [update("/src/lazy.ts")] });
  expect(r.imported).toEqual([]);
  expect(r.reloads).toBe(0);
});

test("a boundary without a matching accept handler reloads", async () => {
  const r = host({ "/src/a.ts": {} });
  createHotContext("/src/a.ts");
  await handleHotFrame({ type: "update", updates: [update("/src/a.ts")] });
  expect(r.reloads).toBe(1);
});

test("a failed import is reported, not thrown, and later updates still run", async () => {
  const r = host({ "/src/b.ts": {} });
  createHotContext("/src/a.ts").accept();
  createHotContext("/src/b.ts").accept();
  await handleHotFrame({ type: "update", updates: [update("/src/a.ts"), update("/src/b.ts")] });
  expect(r.failed).toEqual(["/src/a.ts"]);
  expect(r.updated).toEqual(["/src/b.ts"]);
  expect(r.sent[0]?.[0]).toBe("oj:hmr-result");
});

test("a failed import inside a circular import reloads instead of reporting", async () => {
  const r = host();
  createHotContext("/src/a.ts").accept();
  await handleHotFrame({ type: "update", updates: [update("/src/a.ts", { isWithinCircularImport: true })] });
  expect(r.reloads).toBe(1);
  expect(r.failed).toEqual([]);
});

test("beforeUpdate can turn an update into a reload", async () => {
  const r = host({}, { beforeUpdate: () => "reload" });
  createHotContext("/src/a.ts").accept();
  await handleHotFrame({ type: "update", updates: [update("/src/a.ts")] });
  expect(r.reloads).toBe(1);
  expect(r.imported).toEqual([]);
});

test("invalidate sends vite:invalidate with the module that started the chain", async () => {
  const r = host({ "/src/a.ts": {} });
  const hot = createHotContext("/src/a.ts");
  hot.accept(() => hot.invalidate("exports changed"));
  await handleHotFrame({ type: "update", updates: [update("/src/a.ts", { firstInvalidatedBy: "/src/root.ts" })] });
  expect(r.sent).toContainEqual([
    "vite:invalidate",
    { path: "/src/a.ts", message: "exports changed", firstInvalidatedBy: "/src/root.ts" },
  ]);
});

test("invalidate reloads when the socket cannot carry it", () => {
  const r = host({}, { send: () => false });
  createHotContext("/src/a.ts").invalidate();
  expect(r.reloads).toBe(1);
});

test("full-reload reloads only pages it concerns", async () => {
  const r = host({}, { pagePath: () => "/admin/", base: "/" });
  await handleHotFrame({ type: "full-reload", path: "/other.html" });
  expect(r.reloads).toBe(0);
  await handleHotFrame({ type: "full-reload", path: "/admin/index.html" });
  await handleHotFrame({ type: "full-reload", path: "/index.html" });
  await handleHotFrame({ type: "full-reload", path: "*" });
  expect(r.reloads).toBe(3);
});

test("error frames reach the host; custom frames reach hot listeners", async () => {
  const r = host();
  const hot = createHotContext("/src/a.ts");
  const events: unknown[] = [];
  hot.on("my:event", (data) => events.push(data));
  await handleHotFrame({ type: "error", err: { message: "bad", frame: "1 | x" } });
  await handleHotFrame({ type: "error", message: "legacy" });
  await handleHotFrame({ type: "custom", event: "my:event", data: 5 });
  expect(r.serverErrors).toEqual([{ message: "bad", frame: "1 | x" }, { message: "legacy" }]);
  expect(events).toEqual([5]);
});

test("re-executing a module keeps its data and drops its old callbacks", async () => {
  host({ "/src/a.ts": {} });
  const first = createHotContext("/src/a.ts");
  first.data.kept = true;
  const calls: string[] = [];
  first.accept(() => calls.push("old"));
  first.on("e", () => calls.push("old-listener"));
  const second = createHotContext("/src/a.ts");
  second.accept(() => calls.push("new"));
  expect(second.data.kept).toBe(true);
  await handleHotFrame({ type: "custom", event: "e" });
  await handleHotFrame({ type: "update", updates: [update("/src/a.ts")] });
  expect(calls).toEqual(["new"]);
});

test("prune runs dispose and prune callbacks", async () => {
  host();
  const hot = createHotContext("/src/a.css?import");
  const calls: string[] = [];
  hot.dispose(() => void calls.push("dispose"));
  hot.prune(() => void calls.push("prune"));
  await handleHotFrame({ type: "prune", paths: ["/src/a.css?import"] });
  expect(calls).toEqual(["dispose", "prune"]);
});

test("updateStyle writes one style tag per id", () => {
  updateStyle("/src/a.css", "a{}");
  updateStyle("/src/a.css", "b{}");
  const tags = document.querySelectorAll('style[data-devermind-id="/src/a.css"]');
  expect(tags.length).toBe(1);
  expect(tags[0]?.textContent).toBe("b{}");
});
