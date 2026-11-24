import { readdirSync } from 'node:fs';
import { basename, dirname, relative, resolve } from 'node:path';
import { loadTypeScript } from './typescript';

export interface ViewPair { name: string; classFile: string; layoutFile: string; }
export interface ValidationResult { pairs: ViewPair[]; errors: string[]; }

function sourceFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = resolve(directory, entry.name);
    return entry.isDirectory() ? sourceFiles(path) : [path];
  });
}

/** Checks compiler-resolved declarations. It does not infer process boundaries. */
export function validateProject(root: string, viewsDirectory = 'views'): ValidationResult {
  const ts = loadTypeScript();
  const configPath = ts.findConfigFile(root, ts.sys.fileExists, 'tsconfig.json');
  if (!configPath) throw new Error('Missing tsconfig.json.');
  const config = ts.readConfigFile(configPath, ts.sys.readFile);
  if (config.error) throw new Error(ts.flattenDiagnosticMessageText(config.error.messageText, '\n'));
  const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, root);
  // TypeScript config discovery drops .tsx when a same-name .ts exists.
  const files = sourceFiles(resolve(root, viewsDirectory));
  const roots = [...new Set([...parsed.fileNames, ...files.filter(file => /\.tsx?$/.test(file) && !file.endsWith('.test.ts'))])];
  const program = ts.createProgram(roots, parsed.options);
  const checker = program.getTypeChecker();
  const errors: string[] = [];
  const pairs: ViewPair[] = [];
  const common = program.getSourceFile(resolve(root, 'common/index.ts'));
  if (!common) return { pairs, errors: ['Cannot resolve common/index.ts.'] };
  const commonSymbol = checker.getSymbolAtLocation(common);
  const dereference = (symbol: any): any => symbol && symbol.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(symbol) : symbol;
  const exports = new Map<string, any>(checker.getExportsOfModule(commonSymbol).map((symbol: any) => [symbol.name, dereference(symbol)]));
  const decorators = (node: any): readonly any[] => ts.canHaveDecorators(node) ? ts.getDecorators(node) ?? [] : [];
  const hasDecorator = (node: any, name: string): boolean => !!exports.get(name) && decorators(node).some((decorator: any) => {
    const expression = ts.isCallExpression(decorator.expression) ? decorator.expression.expression : decorator.expression;
    return dereference(checker.getSymbolAtLocation(expression)) === exports.get(name);
  });
  const report = (node: any, message: string): void => {
    const file = node.getSourceFile();
    const position = file.getLineAndCharacterOfPosition(node.getStart());
    errors.push(`${relative(root, file.fileName)}:${position.line + 1}:${position.character + 1} ${message}`);
  };
  const layouts = files.filter(file => file.endsWith('.tsx'));
  const pairedClasses = new Set<string>();
  for (const layoutFile of layouts) {
    const name = basename(layoutFile, '.tsx');
    const classFile = resolve(dirname(layoutFile), `${name}.ts`);
    const source = program.getSourceFile(classFile);
    const layout = program.getSourceFile(layoutFile);
    if (!source || !layout) { errors.push(`${relative(root, layoutFile)} requires a paired ${name}.ts included by tsconfig.json.`); continue; }
    if (basename(dirname(layoutFile)) !== name) errors.push(`${relative(root, layoutFile)} must be inside its own ${name}/ directory.`);
    const declaration = source.statements.find((statement: any) => ts.isClassDeclaration(statement) && statement.name?.text === name);
    if (!declaration || !hasDecorator(declaration, 'ViewClass')) { errors.push(`${relative(root, classFile)} must declare @ViewClass class ${name}.`); continue; }
    pairedClasses.add(classFile);
    const viewSymbol = checker.getSymbolAtLocation(declaration.name);
    const outlets = new Set<any>();
    for (const member of declaration.members) if (hasDecorator(member, 'ViewOutlet')) outlets.add(checker.getSymbolAtLocation(member.name));
    const connected = new Set<any>();
    let layoutExport = false;
    const visit = (node: any): void => {
      if (ts.isFunctionDeclaration(node) && node.name?.text === 'layout') {
        layoutExport = !!node.modifiers?.some((modifier: any) => modifier.kind === ts.SyntaxKind.ExportKeyword);
        const parameter = node.parameters[0];
        if (!parameter || checker.getTypeAtLocation(parameter).symbol !== viewSymbol) report(node, `layout must receive the paired ${name} view type.`);
      }
      if (ts.isJsxAttribute(node)) {
        const attribute = node.name.getText();
        if (attribute === 'outlet' || /^on[A-Z]/.test(attribute)) {
          const expression = node.initializer && ts.isJsxExpression(node.initializer) ? node.initializer.expression : undefined;
          if (!expression || !ts.isPropertyAccessExpression(expression)) report(node, `${attribute} must reference a declared view member directly.`);
          else {
            const symbol = checker.getSymbolAtLocation(expression.name);
            const member = symbol?.valueDeclaration;
            if (attribute === 'outlet') {
              if (!outlets.has(symbol)) report(node, 'outlet must reference a @ViewOutlet on the paired view.');
              else if (connected.has(symbol)) report(node, 'An outlet may be connected only once.');
              else {
                connected.add(symbol);
                const outletDecorator = decorators(member).find((decorator: any) => {
                  const expression = ts.isCallExpression(decorator.expression) ? decorator.expression.expression : decorator.expression;
                  return dereference(checker.getSymbolAtLocation(expression)) === exports.get('ViewOutlet');
                });
                const token = outletDecorator && ts.isCallExpression(outletDecorator.expression) ? outletDecorator.expression.arguments[0] : undefined;
                const declaredControl = token && dereference(checker.getSymbolAtLocation(token));
                const actualControl = dereference(checker.getSymbolAtLocation(node.parent.parent.tagName));
                if (!declaredControl || declaredControl !== actualControl) report(node, 'Outlet control must match its @ViewOutlet constructor token.');
              }
            } else if (!member || member.parent !== declaration || !hasDecorator(member, 'ViewAction')) report(node, `${attribute} must reference a @ViewAction on the paired view.`);
          }
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(layout);
    if (!layoutExport) report(layout, 'The paired JSX file must export function layout.');
    for (const outlet of outlets) if (!connected.has(outlet)) report(outlet.valueDeclaration, `Outlet ${outlet.name} has no layout connection.`);
    pairs.push({ name, classFile, layoutFile });
  }
  for (const file of files.filter(file => file.endsWith('.ts') && !file.endsWith('.test.ts'))) {
    const source = program.getSourceFile(file);
    if (!source) continue;
    for (const statement of source.statements) if (ts.isClassDeclaration(statement) && hasDecorator(statement, 'ViewClass') && !pairedClasses.has(file)) report(statement, 'A view class requires a paired same-name .tsx layout.');
  }
  // Injection tokens must identify an actual service class, regardless of importing alias.
  for (const source of program.getSourceFiles().filter((file: any) => file.fileName.startsWith(root) && !file.isDeclarationFile)) {
    const visit = (node: any): void => {
      for (const decorator of decorators(node)) {
        if (!ts.isCallExpression(decorator.expression)) continue;
        const call = decorator.expression;
        if (dereference(checker.getSymbolAtLocation(call.expression)) !== exports.get('Inject')) continue;
        const token = call.arguments[0];
        const symbol = token && dereference(checker.getSymbolAtLocation(token));
        const declaration = symbol?.valueDeclaration;
        if (call.arguments.length !== 1 || !declaration || !ts.isClassDeclaration(declaration) || !hasDecorator(declaration, 'Service')) report(node, '@Inject requires one @Service class symbol.');
        else if (node.type && !checker.isTypeAssignableTo(checker.getDeclaredTypeOfSymbol(symbol), checker.getTypeAtLocation(node.type))) report(node, 'Injected service does not match the property type.');
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return { pairs: pairs.sort((a, b) => a.classFile.localeCompare(b.classFile)), errors };
}

export function generateViewModule(root: string, pairs: readonly ViewPair[]): string {
  const importPath = (file: string) => `../${relative(root, file).replaceAll('\\', '/')}`;
  return '// Generated from paired view files. Do not edit.\n' + pairs.map((pair, index) =>
    `import { ${pair.name} as View${index} } from ${JSON.stringify(importPath(pair.classFile))};\nimport { layout as Layout${index} } from ${JSON.stringify(importPath(pair.layoutFile))};`
  ).join('\n') + '\n\n' + pairs.map((pair, index) => `export const ${pair.name}View = { view: View${index}, layout: Layout${index} } as const;`).join('\n') + '\nexport const views = [' + pairs.map(pair => `${pair.name}View`).join(', ') + '] as const;\n';
}

/** Typechecks both members of every view pair, including generated imports. */
export function projectTypeErrors(root: string, pairs: readonly ViewPair[]): string[] {
  const ts = loadTypeScript();
  const configPath = resolve(root, 'tsconfig.json');
  const config = ts.readConfigFile(configPath, ts.sys.readFile);
  const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, root);
  const roots = [...new Set([...parsed.fileNames, ...pairs.flatMap(pair => [pair.classFile, pair.layoutFile])])];
  const program = ts.createProgram(roots, parsed.options);
  return ts.getPreEmitDiagnostics(program).map((diagnostic: any) => {
    const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
    if (!diagnostic.file) return message;
    const position = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start ?? 0);
    return `${relative(root, diagnostic.file.fileName)}:${position.line + 1}:${position.character + 1} ${message}`;
  });
}
