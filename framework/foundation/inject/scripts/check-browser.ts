// Browser gate: bundles every browser-safe entry for the browser and fails if any bundle pulls in the server context.
// Run with: bun run check:browser
const manifest = await Bun.file(new URL('../package.json', import.meta.url)).json();
const entrypoints = [...new Set<string>(Object.entries(manifest.exports).filter(([key]) => !key.includes('server') && key !== './package.json').map(([, route]) => (route as { bun: string }).bun))];
const forbidden = /AsyncLocalStorage|async_hooks/;

const result = await Bun.build({ entrypoints, target: 'browser', external: ['react'], conditions: ['bun'], minify: false });
if (!result.success) {
  for (const log of result.logs) console.error(log);
  process.exit(1);
}

let failed = false;
for (const output of result.outputs) {
  const text = await output.text();
  const match = text.match(forbidden);
  if (match) {
    console.error(`${output.path}: bundle contains "${match[0]}"`);
    failed = true;
  }
}
if (failed) process.exit(1);
console.log(`browser gate passed for ${result.outputs.length} output(s)`);
