import { defineConfig } from "vitest/config";
import { shared } from "@coldmind/config/vitest";

export default defineConfig(shared({ test: { name: "@coldmind/core" } }));
