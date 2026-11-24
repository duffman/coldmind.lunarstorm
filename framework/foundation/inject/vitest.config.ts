import { readFileSync } from 'node:fs';
import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";
import { shared } from "@coldmind/config/vitest";

/** Coverage combines the runtime suites under Node so V8 can measure every source entry. */
const coverage = process.env.CMINJECT_COVERAGE === "1";

export default defineConfig(shared({
  resolve: { conditions: ['bun'], alias: Object.entries(JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).exports).filter(([, value]) => typeof value === 'object').map(([key, value]) => ({ find: new RegExp('^' + (key === '.' ? '@coldmind/inject' : '@coldmind/inject/' + key.slice(2)).replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '$'), replacement: fileURLToPath(new URL((value as { bun: string }).bun, import.meta.url)) })) },
  root: fileURLToPath(new URL(".", import.meta.url)),
  test: {
    name: "@coldmind/inject",
    include: coverage ? ["tests/bun/**/*.test.ts", "tests/react/**/*.test.{ts,tsx}"] : ["tests/bun/**/*.test.ts"],
    setupFiles: coverage ? ["tests/runtime.setup.ts", "tests/react/setup.ts"] : ["tests/runtime.setup.ts"],
    coverage: {
      provider: "v8",
      include: ["src/**/*.ts"],
      reporter: ["text", "json", "json-summary", "html"],
      reportsDirectory: ".omx/evidence/prio-one/current-coverage",
      reportOnFailure: true,
      thresholds: {
        perFile: true,
        "src/cminject.hot.ts": { statements: 0, branches: 100, functions: 0, lines: 0 },
        "src/cminject.inspect.ts": { statements: 0, branches: 100, functions: 0, lines: 0 },
        "src/cminject.react.ts": { statements: 95.16, branches: 90.9, functions: 93.33, lines: 100 },
        "src/core/cminject.container.ts": { statements: 85.45, branches: 77.82, functions: 80, lines: 90.06 },
        // Each extracted responsibility inherits the unchanged pre-extraction container floor.
        "src/core/cminject.async.ts": { statements: 85.45, branches: 77.82, functions: 80, lines: 90.06 },
        "src/core/cminject.binding.ts": { statements: 85.45, branches: 77.82, functions: 80, lines: 90.06 },
        "src/core/cminject.view.ts": { statements: 85.45, branches: 77.82, functions: 80, lines: 90.06 },
        "src/core/cminject.observe.ts": { statements: 85.45, branches: 77.82, functions: 80, lines: 90.06 },
        "src/core/cminject.construction.ts": { statements: 85.45, branches: 77.82, functions: 80, lines: 90.06 },
        "src/core/cminject.resolution.ts": { statements: 85.45, branches: 77.82, functions: 80, lines: 90.06 },
        "src/core/cminject.mutation.ts": { statements: 85.45, branches: 77.82, functions: 80, lines: 90.06 },
        // The host module is statically proven type-only; it emits no runtime coverage units.
        "src/core/cminject.lazy.ts": { statements: 90, branches: 80, functions: 90, lines: 95 },
        "src/core/cminject.module.ts": { statements: 90, branches: 80, functions: 90, lines: 95 },
        "src/cminject.testing.ts": { statements: 80, branches: 75, functions: 80, lines: 95 },
        "src/core/cminject.host.ts": { statements: 100, branches: 100, functions: 100, lines: 100 },
        "src/core/cminject.contracts.ts": { statements: 100, branches: 100, functions: 100, lines: 100 },
        "src/core/cminject.core.ts": { statements: 100, branches: 100, functions: 100, lines: 100 },
        "src/core/cminject.decorators.ts": { statements: 88.6, branches: 82.35, functions: 89.28, lines: 95.23 },
        "src/core/cminject.errors.ts": { statements: 85, branches: 75, functions: 72.72, lines: 84.61 },
        "src/core/cminject.graph.ts": { statements: 90.33, branches: 82.57, functions: 91.83, lines: 97.17 },
        "src/core/cminject.token.ts": { statements: 90.24, branches: 73.07, functions: 85.71, lines: 90.62 },
        "src/server/cminject.context.ts": { statements: 77.77, branches: 50, functions: 75, lines: 83.33 },
        "src/server/cminject.runtime.ts": { statements: 73.37, branches: 48.31, functions: 87.87, lines: 86.08 },
        "src/server/cminject.server.ts": { statements: 100, branches: 100, functions: 100, lines: 100 },
      },
    },
  },
}));
