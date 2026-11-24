import ts from 'typescript';
import { readdir, readFile } from 'node:fs/promises';
import { resolve, basename } from 'node:path';

/** Source-only import graph: type imports do not create emitted runtime edges. */
const root = process.argv[2] ? resolve(process.argv[2]) : resolve(import.meta.dirname, '..');
const directory = resolve(root, 'src/core');
const files = (await readdir(directory)).filter(name => name.endsWith('.ts')).map(name => resolve(directory, name));
const manifest = JSON.parse(await readFile(resolve(import.meta.dirname, '../package.json'), 'utf8'));
const known = new Set(files);
const edges = new Map<string, string[]>();
const helpers = new Set(['async', 'binding', 'view', 'observe', 'construction', 'resolution', 'mutation', 'host'].map(name => `cminject.${name}.ts`));
const fail = (message: string): never => { throw new Error(message); };
for (const file of files) {
  const source = ts.createSourceFile(file, await readFile(file, 'utf8'), ts.ScriptTarget.Latest, true);
  const runtime: string[] = [];
  for (const statement of source.statements) {
    if (!ts.isImportDeclaration(statement) && !ts.isExportDeclaration(statement)) continue;
    const specifier = statement.moduleSpecifier;
    if (!specifier || !ts.isStringLiteral(specifier)) continue;
    const clause = ts.isImportDeclaration(statement) ? statement.importClause : undefined;
    const named = clause?.namedBindings;
    const typeOnly = ts.isImportDeclaration(statement)
      ? !!clause && (clause.isTypeOnly || (!clause.name && !!named && ts.isNamedImports(named) && named.elements.length > 0 && named.elements.every(item => item.isTypeOnly)))
      : statement.isTypeOnly || (!!statement.exportClause && ts.isNamedExports(statement.exportClause) && statement.exportClause.elements.length > 0 && statement.exportClause.elements.every(item => item.isTypeOnly));
    if (typeOnly) continue;
    if (basename(file) === 'cminject.host.ts') fail('cminject.host.ts must have only type imports/exports');
    const subpath = specifier.text === '@coldmind/inject' ? '.' : specifier.text.startsWith('@coldmind/inject/') ? './' + specifier.text.slice('@coldmind/inject/'.length) : undefined;
    const exported = subpath === undefined ? undefined : manifest.exports[subpath];
    const exportedSource = typeof exported === 'string' ? exported : exported?.bun;
    if (subpath && !exportedSource) fail(`Unknown explicit package import: ${specifier.text}`);
    const target = exportedSource ? resolve(root, exportedSource) : specifier.text.startsWith('.') ? resolve(file, '..', specifier.text) : undefined;
    if (!target || !known.has(target)) continue;
    if (helpers.has(basename(file)) && ['cminject.container.ts', 'cminject.core.ts'].includes(basename(target))) fail(`${basename(file)} has a forbidden runtime back-import to ${basename(target)}`);
    runtime.push(target);
  }
  if (basename(file) === 'cminject.host.ts' && source.statements.some(node => !ts.isImportDeclaration(node) && !ts.isInterfaceDeclaration(node) && !ts.isTypeAliasDeclaration(node) && !ts.isExportDeclaration(node))) fail('cminject.host.ts must contain types only');
  edges.set(file, runtime);
}
const visited = new Set<string>(); const active = new Set<string>();
function visit(file: string, path: string[]): void {
  if (active.has(file)) fail(`Core runtime import cycle: ${[...path, file].map(item => basename(item)).join(' -> ')}`);
  if (visited.has(file)) return;
  active.add(file); for (const child of edges.get(file) ?? []) visit(child, [...path, file]); active.delete(file); visited.add(file);
}
for (const file of files) visit(file, []);
console.log(`core runtime import graph is acyclic: ${files.length} modules, ${[...edges.values()].reduce((sum, items) => sum + items.length, 0)} edges`);
