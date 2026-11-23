import { expect, test } from "vitest";

import { ShellErrors, toShellError } from "@coldmind/devtools-shell/src/error-store.ts";

test("reports, notifies and keeps a stable snapshot between changes", () => {
  const errors = new ShellErrors();
  let calls = 0;
  errors.subscribe(() => calls++);
  const before = errors.list();
  expect(errors.list()).toBe(before);
  errors.report({ kind: "server", message: "a" });
  expect(errors.list()).not.toBe(before);
  expect(errors.list().map((e) => e.message)).toEqual(["a"]);
  expect(calls).toBe(1);
  expect(errors.blocking()).toBe(true);
});

test("a repeat of the newest error is not added twice", () => {
  const errors = new ShellErrors();
  const first = errors.report({ kind: "runtime", message: "x" });
  expect(errors.report({ kind: "runtime", message: "x" })).toBe(first);
  errors.report({ kind: "runtime", message: "y" });
  errors.report({ kind: "runtime", message: "x" });
  expect(errors.list().map((e) => e.message)).toEqual(["x", "y", "x"]);
});

test("runtime errors are capped, others are kept", () => {
  const errors = new ShellErrors(2);
  errors.report({ kind: "server", message: "s" });
  for (const m of ["1", "2", "3"]) errors.report({ kind: "runtime", message: m });
  expect(errors.list().map((e) => e.message)).toEqual(["s", "2", "3"]);
  expect(errors.blocking()).toBe(true);
});

test("clear by kind, clear all, dismiss", () => {
  const errors = new ShellErrors();
  errors.report({ kind: "server", message: "s" });
  const r = errors.report({ kind: "runtime", message: "r" });
  errors.report({ kind: "hmr", message: "h" });
  errors.clear("server", "hmr");
  expect(errors.list().map((e) => e.kind)).toEqual(["runtime"]);
  expect(errors.blocking()).toBe(false);
  errors.dismiss(r.id);
  expect(errors.list()).toEqual([]);
  errors.report({ kind: "load", message: "l" });
  errors.clear();
  expect(errors.list()).toEqual([]);
});

test("toShellError accepts errors, error-like objects and anything else", () => {
  const e = new Error("boom");
  expect(toShellError("render", e)).toEqual({ kind: "render", message: "boom", stack: e.stack });
  expect(toShellError("hmr", { message: "m" })).toEqual({ kind: "hmr", message: "m" });
  expect(toShellError("runtime", 42)).toEqual({ kind: "runtime", message: "42" });
});
