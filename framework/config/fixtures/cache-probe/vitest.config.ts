import { defineConfig } from "vitest/config";
import { shared } from "@coldmind/config/vitest";

/** A package as the monorepo's packages configure vitest: `shared()` plus a name. */
export default defineConfig(shared({ test: { name: "cache-probe" } }));
