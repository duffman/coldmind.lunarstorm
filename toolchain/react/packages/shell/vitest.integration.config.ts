import { defineConfig } from "vitest/config";
import { shared } from "@coldmind/config/vitest";

export default defineConfig(shared({ test: {
  name: "@coldmind/devtools-shell-integration",
  include: ["tests/integration/**/*.test.tsx"],
  setupFiles: ["tests/integration/setup.ts"],
  testTimeout: 30000,
  hookTimeout: 30000,
  fileParallelism: false,
} }));
