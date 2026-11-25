import { defineConfig } from "vitest/config";
import { shared } from "@coldmind/config/vitest";

export default defineConfig(shared({ test: { name: "@lunarstorm/devmode", setupFiles: ["tests/setup.ts"], include: ["tests/*.test.ts", "tests/*.test.tsx"] } }));
