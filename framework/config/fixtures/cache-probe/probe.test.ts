import { expect, test } from "vitest";
import { double } from "./probe";

/** Imports a module, so vitest has a transform to put in its module cache. */
test("probe runs", () => {
  expect(double(2)).toBe(4);
});
