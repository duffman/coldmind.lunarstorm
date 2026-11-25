import { defineConfig } from "vitest/config";
import { onBun, shared } from "@coldmind/config/vitest";

// The retention tests force collections: Bun has Bun.gc, Node needs --expose-gc.
export default defineConfig(shared({ test: { name: "@coldmind/wire", execArgv: onBun ? [] : ["--expose-gc"] } }));
