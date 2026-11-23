import { resolve } from 'node:path';
import { validateProject, generateViewModule, projectTypeErrors } from './validate';

const root = resolve(import.meta.dir, '..');
const project = await Bun.file(resolve(root, 'lsapp.project.json')).json();
const result = validateProject(root, project.views);
if (result.errors.length) throw new Error(`LS project validation failed:\n${result.errors.join('\n')}`);
if (!result.pairs.length) throw new Error('The project must contain at least one paired view.');
await Bun.write(resolve(root, 'tooling/generated-views.ts'), generateViewModule(root, result.pairs));
if (process.argv.includes('--generate-only')) {
  console.log(`Validated and paired ${result.pairs.length} view(s).`);
} else {
  const typeErrors = projectTypeErrors(root, result.pairs);
  if (typeErrors.length) throw new Error(`Typecheck failed:\n${typeErrors.join('\n')}`);
  const build = await Bun.build({
    entrypoints: [resolve(root, project.entry)],
    outdir: resolve(root, project.output),
    target: 'browser',
    naming: 'application.main.[ext]',
    sourcemap: 'linked',
  });
  if (!build.success) throw new AggregateError(build.logs, 'Application build failed.');
  console.log(`Validated ${result.pairs.length} paired view(s); built ${build.outputs.length} browser assets.`);
}
