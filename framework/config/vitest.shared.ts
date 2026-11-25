/**
 * Shared vitest configuration. Tests run on Bun (`bun --bun vitest run`) and on Node (`vitest run`); the
 * runtime is whichever one runs vitest. Each package's vitest.config.ts is `defineConfig(shared(...))`.
 */
import { join } from "node:path";
import { transform } from "esbuild";
import type { Plugin } from "vite";
import { mergeConfig, type ViteUserConfig } from "vitest/config";

/** True when vitest itself runs on Bun. */
export const onBun = "bun" in process.versions;

/** Matches a decorator at the start of a line, the only way the codebase applies them. */
const decorated = /^\s*@[A-Za-z_$]/m;

/**
 * Oxc lowers legacy decorators (experimentalDecorators, emitDecoratorMetadata, from each file's tsconfig) but
 * leaves standard ones, which neither runtime parses yet. Runs after Oxc, so whatever still carries a
 * decorator is standard: esbuild lowers it, on either runtime.
 */
function standardDecorators(): Plugin {
  return {
    name: "coldmind:standard-decorators",
    enforce: "post",
    async transform(code, id) {
      const file = id.split("?", 1)[0]!;
      if (!/\.[cm]?tsx?$/.test(file) || file.includes("/node_modules/") || !decorated.test(code)) return;
      const result = await transform(code, { loader: "js", format: "esm", target: "es2022", sourcemap: "external", sourcefile: file });
      return { code: result.code, map: result.map };
    },
  };
}

/** Bun's `import.meta.dir` exists in neither vite's module runner nor Node; `import.meta.dirname` is the same value. */
function bunImportMeta(): Plugin {
  const dir = /\bimport\.meta\.dir\b/g;
  return {
    name: "coldmind:bun-import-meta",
    transform(code, id) {
      if (id.includes("/node_modules/") || !dir.test(code)) return;
      dir.lastIndex = 0;
      return { code: code.replace(dir, "import.meta.dirname"), map: null };
    },
  };
}

/** The base every package extends; `overrides` are merged on top. */
export function shared(overrides: ViteUserConfig = {}): ViteUserConfig {
  return mergeConfig({
    plugins: [standardDecorators(), bunImportMeta()],
    test: {
      server: { deps: { external: [/^bun:/] } },
      // Fix: vitest's default module cache is `<workspace root>/node_modules/.vitest-cache`,
      // shared by every package and cleared on each run, so parallel `pnpm -r test` runs
      // failed with ENOTEMPTY. Decision: one cache per package, in the package's own
      // node_modules (the directory vitest runs in, which pnpm sets per package), so a
      // reinstall still invalidates it.
      fsModuleCachePath: join(process.cwd(), "node_modules", ".vitest-cache"),
    },
  } satisfies ViteUserConfig, overrides);
}
