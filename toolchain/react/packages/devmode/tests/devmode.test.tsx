import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { capture, DevMode } from "../src/index.ts";
import { fakeSocket } from "./fake-ws.ts";

let root: Root;
beforeEach(() => {
  localStorage.clear();
  capture.clear();
  root = createRoot(document.createElement("div"));
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const panel = () => document.querySelector(".lsdm-panel");
/** Lets timers and React updates run until `cond` holds (at most ~1 s). */
const until = async (cond: () => boolean) => {
  for (let i = 0; i < 100 && !cond(); i++) await act(() => new Promise((r) => setTimeout(r, 10)));
};
const press = (init: KeyboardEventInit) => act(async () => { window.dispatchEvent(new KeyboardEvent("keydown", init)); });

test("disabled renders nothing; the hotkey toggles the panel", async () => {
  await act(async () => root.render(<DevMode enabled={false} />));
  expect(document.querySelector(".lsdm")).toBeNull();
  await act(async () => root.render(<DevMode />));
  expect(panel()).toBeNull();
  await press({ key: "`", ctrlKey: true });
  expect(panel()).not.toBeNull();
  await press({ key: "`", ctrlKey: true });
  expect(panel()).toBeNull();
});

test("minimize collapses the docked panel to its title bar; a tab click restores it", async () => {
  localStorage.setItem("lunarstorm.devmode.open", "true");
  await act(async () => root.render(<DevMode />));
  expect(document.querySelector(".lsdm-resize")).not.toBeNull();
  await act(async () => document.querySelector<HTMLButtonElement>(".lsdm-min")!.click());
  expect(panel()!.classList.contains("lsdm-minimized")).toBe(true);
  expect((panel() as HTMLElement).style.height).toBe("50px");
  expect(document.body.style.paddingBottom).toBe("50px");
  expect(document.querySelector(".lsdm-resize")).toBeNull();
  expect(document.querySelector(".lsdm-min")!.getAttribute("aria-label")).toBe("Restore");
  await act(async () => document.querySelectorAll<HTMLButtonElement>(".lsdm-tab")[1]!.click());
  expect(panel()!.classList.contains("lsdm-minimized")).toBe(false);
  expect(document.querySelector(".lsdm-min")!.getAttribute("aria-label")).toBe("Minimize");
});

test("the API tab runs a pipeline over RESP and prints redis-cli style replies", async () => {
  vi.stubGlobal("WebSocket", fakeSocket());
  localStorage.setItem("lunarstorm.devmode.open", "true");
  localStorage.setItem("lunarstorm.devmode.tab", '"api"');
  await act(async () => root.render(<DevMode redisUrl="ws://api.test/resp" />));
  const ta = document.querySelector<HTMLTextAreaElement>(".lsdm-cmdbox textarea")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(ta, 'SET greeting "hi there"\nGET greeting\nNOPE');
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(document.querySelector(".lsdm-send")!.textContent).toBe("Pipeline 3");
  await act(async () => { ta.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
  await until(() => document.querySelectorAll(".lsdm-reply").length === 3);
  expect([...document.querySelectorAll(".lsdm-cmdline")].map((e) => e.textContent)).toEqual(['› SET greeting "hi there"', "› GET greeting", "› NOPE"]);
  expect([...document.querySelectorAll(".lsdm-reply")].map((e) => e.textContent)).toEqual(["OK", '"hi there"', "(error) ERR unknown command 'NOPE', with args beginning with: "]);
  expect(document.querySelector(".lsdm-conn")!.textContent).toBe("Connected");
  expect(document.querySelector(".lsdm-proto")!.textContent).toBe("RESP3");
});

test("the API tree lists the server's calls by category; click loads, double-click runs, Save files under a category", async () => {
  vi.stubGlobal("WebSocket", fakeSocket());
  localStorage.setItem("lunarstorm.devmode.open", "true");
  localStorage.setItem("lunarstorm.devmode.tab", '"api"');
  await act(async () => root.render(<DevMode redisUrl="ws://api.test/resp" />));
  const labels = () => [...document.querySelectorAll(".lsdm-cat-label")].map((e) => e.textContent);
  await until(() => labels().includes("Hashes"));
  expect(labels()).toEqual(["Saved", "Connection", "Hashes", "Keys", "Lists", "Pub/Sub", "Server", "Sets", "Strings"]);
  const cat = (name: string) => [...document.querySelectorAll<HTMLElement>(".lsdm-cat-row")].find((r) => r.textContent!.startsWith(name))!;
  const call = (name: string) => [...document.querySelectorAll<HTMLElement>(".lsdm-call")].find((r) => r.querySelector(".lsdm-cmd-name")!.textContent === name)!;
  const editor = () => document.querySelector<HTMLTextAreaElement>(".lsdm-cmdbox textarea")!;

  await act(async () => cat("Hashes").click());
  expect(cat("Hashes").parentElement!.getAttribute("aria-expanded")).toBe("true");
  await act(async () => call("HSET").click());
  expect(editor().value).toBe("HSET key field value");

  await act(async () => call("PING").dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
  await until(() => document.querySelectorAll(".lsdm-reply").length === 1);
  expect(document.querySelector(".lsdm-reply")!.textContent).toBe("PONG");

  const filter = document.querySelector<HTMLInputElement>(".lsdm-tree-filter")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(filter, "posts a message");
    filter.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(labels()).toEqual(["Pub/Sub"]);
  expect([...document.querySelectorAll(".lsdm-call .lsdm-cmd-name")].map((e) => e.textContent)).toEqual(["PUBLISH"]);

  const category = document.querySelector<HTMLInputElement>(".lsdm-save input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(category, "Users");
    category.dispatchEvent(new Event("input", { bubbles: true }));
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(filter, "");
    filter.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => document.querySelector<HTMLButtonElement>(".lsdm-save .lsdm-btn")!.click());
  expect(labels()[0]).toBe("Saved");
  expect(labels()[1]).toBe("Users");
  expect(JSON.parse(localStorage.getItem("lunarstorm.devmode.redis.collection")!).at(-1)).toEqual({ category: "Users", command: "HSET key field value" });
});

test("with a HostApi spec the tree lists its areas instead of asking the server", async () => {
  vi.stubGlobal("WebSocket", fakeSocket());
  localStorage.setItem("lunarstorm.devmode.open", "true");
  localStorage.setItem("lunarstorm.devmode.tab", '"api"');
  const api = { version: 1, areas: [{ name: "ui", module: "lunarstorm:hostapi/ui", functions: [{ name: "spinner", summary: "", args: [] }] }] };
  await act(async () => root.render(<DevMode redisUrl="ws://api.test/resp" api={api} />));
  const labels = () => [...document.querySelectorAll(".lsdm-cat-label")].map((e) => e.textContent);
  await until(() => document.querySelector(".lsdm-conn")?.textContent === "Connected");
  expect(labels()).toEqual(["Saved", "Ui"]);
  await act(async () => [...document.querySelectorAll<HTMLElement>(".lsdm-cat-row")].find((r) => r.textContent!.startsWith("Ui"))!.click());
  await act(async () => document.querySelector<HTMLElement>(".lsdm-cat:not(.lsdm-cat-saved) .lsdm-call")!.click());
  expect(document.querySelector<HTMLTextAreaElement>(".lsdm-cmdbox textarea")!.value).toBe("UI.SPINNER");
});

test("under StrictMode the API tab still connects on show (the dev remount must not close the socket)", async () => {
  vi.stubGlobal("WebSocket", fakeSocket());
  localStorage.setItem("lunarstorm.devmode.open", "true");
  localStorage.setItem("lunarstorm.devmode.tab", '"api"');
  await act(async () => root.render(<StrictMode><DevMode redisUrl="ws://api.test/resp" /></StrictMode>));
  await until(() => document.querySelector(".lsdm-conn")!.textContent !== "Connecting…");
  expect(document.querySelector(".lsdm-conn")!.textContent).toBe("Connected");
});

test("the console REPL evaluates input and shows the result", async () => {
  localStorage.setItem("lunarstorm.devmode.open", "true");
  await act(async () => root.render(<DevMode />));
  const ta = document.querySelector<HTMLTextAreaElement>(".lsdm-repl textarea")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(ta, "20 + 22");
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => { ta.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
  await act(async () => { await Promise.resolve(); });
  expect(document.querySelector(".lsdm-t-result")!.textContent).toContain("42");
});
