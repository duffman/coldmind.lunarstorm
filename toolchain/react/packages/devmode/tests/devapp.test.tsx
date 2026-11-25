import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { capture, DevApp } from "../src/index.ts";

let root: Root;
beforeEach(() => {
  localStorage.clear();
  capture.clear();
  // The app renders in place (unlike the dev panel's portal), so its container must be in the document.
  root = createRoot(document.body.appendChild(document.createElement("div")));
  // React reports caught render errors on console.error; keep the test output clean.
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.restoreAllMocks();
});

/** Throws while rendering until `fixed.current` is set. */
const fixed = { current: false };
function Broken() {
  if (!fixed.current) throw new Error("render exploded");
  return <p className="healthy">recovered</p>;
}

test("a healthy app renders inside DevApp next to the docked dev mode", async () => {
  await act(async () => root.render(<DevApp><p className="healthy">app</p></DevApp>));
  expect(document.querySelector(".healthy")?.textContent).toBe("app");
  expect(document.querySelector(".lsdm")).not.toBeNull();
});

test("a render error is caught: fallback shows it, the console has it, Retry remounts the app", async () => {
  fixed.current = false;
  await act(async () => root.render(<DevApp><Broken /></DevApp>));
  const fallback = document.querySelector(".lsdm-crash");
  expect(fallback?.textContent).toContain("render exploded");
  expect(document.querySelector(".lsdm")).not.toBeNull();
  capture.flush();
  expect(capture.snapshot().some((e) => e.type === "error" && e.args.some((a) => a instanceof Error && a.message === "render exploded"))).toBe(true);
  fixed.current = true;
  await act(async () => document.querySelector<HTMLButtonElement>(".lsdm-crash-retry")!.click());
  expect(document.querySelector(".lsdm-crash")).toBeNull();
  expect(document.querySelector(".healthy")?.textContent).toBe("recovered");
});

test("disabled DevApp renders only the app, without boundary chrome or dev mode", async () => {
  function Counter() {
    const [n] = useState(1);
    return <p className="healthy">{n}</p>;
  }
  await act(async () => root.render(<DevApp enabled={false}><Counter /></DevApp>));
  expect(document.querySelector(".healthy")?.textContent).toBe("1");
  expect(document.querySelector(".lsdm")).toBeNull();
});
