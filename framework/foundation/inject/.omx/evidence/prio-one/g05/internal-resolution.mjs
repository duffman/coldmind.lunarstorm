import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
for (const name of ['async','binding','view','observe','construction','resolution','mutation','host']) {
  const path = `@coldmind/inject/internal/${name}`;
  const resolved = require.resolve(path);
  if (!resolved.endsWith(`/src/core/cminject.${name}.ts`)) throw new Error(`wrong internal export: ${path} -> ${resolved}`);
  const target = await import(path);
  const original = await import(`@coldmind/inject/src/core/cminject.${name}.ts`);
  if (target !== original) throw new Error(`duplicate module identity: ${name}`);
  if (name === 'host' && Object.keys(target).length) throw new Error('type-only host emitted runtime exports');
  console.log(`${name}: resolved, imported once, ${Object.keys(target).length} runtime exports`);
}
