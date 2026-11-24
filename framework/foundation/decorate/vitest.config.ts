import { defineConfig } from "vitest/config";
import { shared } from "@coldmind/config/vitest";

export default defineConfig(shared({ test: {
  name: "@coldmind/decorate",
  include: ["tests/standard/**/*.test.ts", "tests/legacy/**/*.test.ts"],
  setupFiles: [new URL("./tests/setup.ts", import.meta.url).pathname],
} }));
