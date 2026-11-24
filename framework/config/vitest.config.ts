import { defineConfig } from "vitest/config";
import { shared } from "./vitest.shared";

/** The config package's own tests; the fixtures are run by those tests, not collected. */
export default defineConfig(shared({ test: { name: "@coldmind/config", include: ["*.test.ts"] } }));
