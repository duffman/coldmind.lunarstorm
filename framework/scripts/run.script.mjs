/**
 * Runs one TypeScript file as a script through vite's module pipeline, the transform vitest itself uses: each
 * file's tsconfig applies and extensionless imports resolve. Node has no TypeScript loader that does both, so
 * the tests that run example scripts spawn this on either runtime.
 *   node scripts/run.script.mjs <file> [args...]
 */
import { resolve } from "node:path";
import { createServer } from "vite";

const [file, ...args] = process.argv.slice(2);
const target = resolve(file);
process.argv = [process.execPath, target, ...args];
const server = await createServer({
  root: process.cwd(),
  configFile: false,
  logLevel: "silent",
  appType: "custom",
  server: { middlewareMode: true, hmr: false, watch: null },
  optimizeDeps: { noDiscovery: true },
});
try {
  await server.ssrLoadModule(target);
} finally {
  await server.close();
}
