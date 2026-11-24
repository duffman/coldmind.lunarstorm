/**
 * Runs real vitest on a fixture package configured with `shared()` and checks where it
 * keeps its module cache.
 *
 * Regression: vitest's default `fsModuleCachePath` is `<workspace root>/node_modules/
 * .vitest-cache`, one directory for every package, and each run clears it on start.
 * `pnpm -r test` runs packages in parallel, so one package's clear raced another's and
 * failed with `ENOTEMPTY: directory not empty, rmdir '.../node_modules/.vitest-cache'`.
 */
import { spawnSync } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { expect, test } from "vitest";

const fixture = join(import.meta.dirname, "fixtures", "cache-probe");
/** The repository's own vitest, installed at the root. */
const vitest = resolve(import.meta.dirname, "../../node_modules/.bin/vitest");

test("each package keeps its vitest module cache in its own node_modules", () => {
  const cache = join(fixture, "node_modules", ".vitest-cache");
  rmSync(join(fixture, "node_modules"), { recursive: true, force: true });
  const run = spawnSync(vitest, ["run", "--root", fixture], { cwd: fixture, encoding: "utf8" });
  expect(run.status, run.stdout + run.stderr).toBe(0);
  expect(existsSync(cache)).toBe(true);
}, 60_000);
