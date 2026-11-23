import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { existsSync, realpathSync } from 'node:fs';

const require = createRequire(import.meta.url);
const isCompiler = (module: any): boolean => !!module.sys && typeof module.createProgram === 'function';

/** Reuses an existing compiler; never asks Bun to auto-install a bare import. */
export function loadTypeScript(): any {
  let directory = import.meta.dir;
  while (true) {
    const installed = resolve(directory, 'node_modules/typescript');
    if (existsSync(installed)) {
      const compiler = require(installed);
      if (isCompiler(compiler)) return compiler;
    }
    const parent = dirname(directory);
    if (parent === directory) break;
    directory = parent;
  }
  const executable = Bun.which('tsc');
  if (!executable) throw new Error('LS tooling requires the TypeScript compiler API. Put tsc on PATH or install typescript locally.');
  const packageRoot = resolve(dirname(realpathSync(executable)), '..');
  try {
    const compiler = require(resolve(packageRoot, 'lib/typescript.js'));
    if (isCompiler(compiler)) return compiler;
  } catch { /* Report the supported requirement below. */ }
  throw new Error(`The tsc installation at ${executable} does not expose the TypeScript compiler API.`);
}
