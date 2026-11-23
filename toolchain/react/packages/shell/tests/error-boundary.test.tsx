import { afterEach, beforeEach, expect, test, vi, type MockInstance } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

import { ShellBoundary } from "@coldmind/devtools-shell/src/error-boundary.tsx";
import { ShellErrors } from "@coldmind/devtools-shell/src/error-store.ts";

let root: Root;
let consoleError: MockInstance<typeof console.error>;

beforeEach(() => {
  consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
  root = createRoot(document.createElement("div"));
});

afterEach(async () => {
  await act(async () => root.unmount());
  consoleError.mockRestore();
});

let broken = true;
function App() {
  if (broken) throw new Error("render exploded");
  return <p>ok</p>;
}

test("reports a render error and clears it on retry through resetKey", async () => {
  broken = true;
  const errors = new ShellErrors();
  await act(async () => root.render(<ShellBoundary errors={errors} resetKey={0}><App /></ShellBoundary>));
  expect(errors.list().map((e) => e.kind)).toEqual(["render"]);
  broken = false;
  await act(async () => root.render(<ShellBoundary errors={errors} resetKey={1}><App /></ShellBoundary>));
  expect(errors.list()).toEqual([]);
});

test("a fresh boundary instance (React Refresh re-mount) clears the stale render error", async () => {
  broken = true;
  const errors = new ShellErrors();
  await act(async () => root.render(<ShellBoundary key="a" errors={errors} resetKey={0}><App /></ShellBoundary>));
  expect(errors.list()).toHaveLength(1);
  broken = false;
  await act(async () => root.render(<ShellBoundary key="b" errors={errors} resetKey={0}><App /></ShellBoundary>));
  expect(errors.list()).toEqual([]);
});

test("an error caught on mount is not cleared by the same commit", async () => {
  broken = true;
  const errors = new ShellErrors();
  await act(async () => root.render(<ShellBoundary errors={errors} resetKey={0}><App /></ShellBoundary>));
  await act(async () => root.render(<ShellBoundary errors={errors} resetKey={0}><App /></ShellBoundary>));
  expect(errors.list()).toHaveLength(1);
});
