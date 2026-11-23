import { readdir, readFile, access } from 'node:fs/promises';
import { resolve, extname } from 'node:path';
const root = resolve(import.meta.dirname, '..');
const manifest = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'));
if (Object.keys(manifest.exports).some(key => key.includes('*'))) throw new Error('Wildcard package exports are forbidden');
async function scan(directory: string): Promise<void> {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = resolve(directory, entry.name);
    if (entry.isDirectory()) { if (!['node_modules', 'dist', '.cache', '.omx'].includes(entry.name)) await scan(file); continue; }
    if (!['.ts', '.tsx', '.mjs', '.mts'].includes(extname(file))) continue;
    const source = await readFile(file, 'utf8');
    for (const match of source.matchAll(/(?:from\s*|import\s*\(|import\s*)['"]([^'"]+)['"]/g)) {
      const name = match[1]!;
      if (name.startsWith('@coldmind/inject/src/')) throw new Error(`Source-path package import: ${file}: ${name}`);
      if (file.startsWith(resolve(root, 'src') + '/') && name.startsWith('.')) throw new Error(`Relative source import: ${file}: ${name}`);
      if (name === '@coldmind/inject' || name.startsWith('@coldmind/inject/')) {
        const key = name === '@coldmind/inject' ? '.' : './' + name.slice('@coldmind/inject/'.length);
        if (!manifest.exports[key]) throw new Error(`Unexported package import: ${name}`);
      }
    }
  }
}
for (const folder of ['src', 'tests', 'scripts', 'examples']) await scan(resolve(root, folder));
for (const [key, value] of Object.entries(manifest.exports)) {
  const route = value as string | { bun: string; types: string; default: string };
  if (typeof route === 'string') { await access(resolve(root, route)); continue; }
  await access(resolve(root, route.bun));
  if (process.argv.includes('--dist')) { await access(resolve(root, route.types)); await access(resolve(root, route.default)); }
  if (Object.keys(route)[0] !== 'types') throw new Error(`types must precede runtime conditions: ${key}`);
}
console.log('Explicit import policy and entry resolution passed');
