import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import { defineConfig } from "vitest/config";
import { shared } from "@coldmind/config/vitest";

// The React adapter suite renders into a DOM, so it runs apart from the rest.
export default defineConfig(shared({
  resolve: { conditions: ['bun'], alias: Object.entries(JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).exports).filter(([, value]) => typeof value === 'object').map(([key, value]) => ({ find: new RegExp('^' + (key === '.' ? '@coldmind/inject' : '@coldmind/inject/' + key.slice(2)).replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '$'), replacement: fileURLToPath(new URL((value as { bun: string }).bun, import.meta.url)) })) }, test: { name: "@coldmind/inject:react", include: ["tests/react/**/*.test.{ts,tsx}"], setupFiles: ["tests/runtime.setup.ts", "tests/react/setup.ts"] } }));
