import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { capture, DevMode } from "../src/index.ts";

let root: Root;
beforeEach(() => {
  localStorage.clear();
  capture.clear();
  root = createRoot(document.createElement("div"));
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.restoreAllMocks();
});

const panel = () => document.querySelector(".lsdm-panel");
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

test("the API tab sends the built request and shows status and JSON body", async () => {
  const fetch = vi.fn(async (_url: string, _init?: RequestInit) => new Response('{"ok":true}', { status: 200, statusText: "OK" }));
  vi.stubGlobal("fetch", fetch);
  localStorage.setItem("lunarstorm.devmode.open", "true");
  localStorage.setItem("lunarstorm.devmode.tab", '"api"');
  await act(async () => root.render(<DevMode env={{ baseUrl: "http://api.test" }} />));
  const url = document.querySelector<HTMLInputElement>(".lsdm-urlinput")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(url, "{{baseUrl}}/users");
    url.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => document.querySelector<HTMLButtonElement>(".lsdm-send")!.click());
  expect(fetch.mock.calls[0]![0]).toBe("http://api.test/users");
  expect(document.querySelector(".lsdm-res .lsdm-status")!.textContent).toBe("200 OK");
  expect(document.querySelector(".lsdm-json")!.textContent).toContain("ok");
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
