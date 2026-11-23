import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { basename, dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { loadTypeScript } from 'lsapp-starter/tooling/typescript';

export interface SourceLocation { file: string; line: number; column: number; endLine: number; endColumn: number; }
export interface Diagnostic { severity: 'error' | 'warning'; code: string; message: string; location?: SourceLocation; }
export interface SymbolReference { id: string; name: string; file: string; location: SourceLocation; }
export type Literal = string | number | boolean | null | Literal[] | { [key: string]: Literal };
export interface ProjectConfig { name: string; version?: string; entry: string; views: string; output: string; host?: string; }
export interface InjectionModel { property: SymbolReference; service: SymbolReference | null; }
export interface MethodModel { symbol: SymbolReference; parameters: { name: string; type: string; optional: boolean }[]; returnType: string; async: boolean; }
export interface ServiceCall { source: SourceLocation; target: SymbolReference; }
export interface ServiceAccess { source: SourceLocation; target: SymbolReference; operation: 'read' | 'write' | 'read-write'; }
export interface ClassModel { symbol: SymbolReference; injections: InjectionModel[]; methods: MethodModel[]; serviceCalls: ServiceCall[]; serviceAccesses: ServiceAccess[]; }
export interface OutletModel { property: SymbolReference; control: SymbolReference | null; }
export interface ControlModel { component: SymbolReference | null; location: SourceLocation; properties: Record<string, Literal>; outlet?: SymbolReference; events: Record<string, SymbolReference>; children: ControlModel[]; }
export interface ViewModel extends ClassModel { classFile: string; layoutFile: string; properties: Record<string, Literal>; outlets: OutletModel[]; actions: SymbolReference[]; layout: ControlModel | null; }
export interface ProjectModel { schemaVersion: 1; root: string; config: ProjectConfig | null; views: ViewModel[]; services: ClassModel[]; diagnostics: Diagnostic[]; }
export function hasErrors(model: ProjectModel): boolean { return model.diagnostics.some(item => item.severity === 'error'); }

/** Extracts declarations and connections. This pass never loads project modules or assigns execution sides. */
export function extractProject(projectRoot: string): ProjectModel {
  const root = resolve(projectRoot);
  const result: ProjectModel = { schemaVersion: 1, root, config: null, views: [], services: [], diagnostics: [] };
  const error = (code: string, message: string, location?: SourceLocation) => result.diagnostics.push({ severity: 'error', code, message, ...(location ? { location } : {}) });
  const local = (file: string) => relative(root, file).split(sep).join('/');
  const inside = (file: string) => { const path = relative(root, file); return path !== '..' && !path.startsWith(`..${sep}`) && !isAbsolute(path); };
  let raw: any;
  try { raw = JSON.parse(readFileSync(resolve(root, 'lsapp.project.json'), 'utf8')); }
  catch (cause) { error('PROJECT_CONFIG', `Cannot read lsapp.project.json: ${cause instanceof Error ? cause.message : String(cause)}`); return result; }
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) { error('PROJECT_CONFIG', 'Project configuration must be an object.'); return result; }
  for (const key of ['name', 'entry', 'views', 'output']) if (typeof raw[key] !== 'string' || !raw[key].trim()) error('PROJECT_CONFIG', `${key} must be a nonempty string.`);
  for (const key of ['version', 'host']) if (raw[key] !== undefined && typeof raw[key] !== 'string') error('PROJECT_CONFIG', `${key} must be a string.`);
  if (hasErrors(result)) return result;
  result.config = { name: raw.name, entry: raw.entry, views: raw.views, output: raw.output, ...(raw.version ? { version: raw.version } : {}), ...(raw.host ? { host: raw.host } : {}) };
  for (const key of ['entry', 'views', 'output']) if (!inside(resolve(root, raw[key]))) error('PROJECT_PATH', `${key} must stay within the project directory.`);
  if (hasErrors(result)) return result;
  if (!existsSync(resolve(root, raw.entry))) error('PROJECT_ENTRY', `Entry file does not exist: ${raw.entry}`);
  if (!existsSync(resolve(root, raw.views))) { error('PROJECT_VIEWS', `Views directory does not exist: ${raw.views}`); return result; }
  let ts: any;
  try { ts = loadTypeScript(); } catch (cause) { error('TYPESCRIPT', String(cause)); return result; }
  const configPath = resolve(root, 'tsconfig.json');
  const config = ts.readConfigFile(configPath, ts.sys.readFile);
  if (config.error) { error('TSCONFIG', ts.flattenDiagnosticMessageText(config.error.messageText, '\n')); return result; }
  const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, root);
  for (const item of parsed.errors) error('TSCONFIG', ts.flattenDiagnosticMessageText(item.messageText, '\n'));
  const walk = (directory: string): string[] => readdirSync(directory, { withFileTypes: true }).flatMap(entry => entry.isDirectory() ? walk(resolve(directory, entry.name)) : entry.isFile() ? [resolve(directory, entry.name)] : []);
  const viewFiles = walk(resolve(root, raw.views)).filter(file => /\.tsx?$/.test(file) && !file.endsWith('.test.ts'));
  const program = ts.createProgram([...new Set([...parsed.fileNames, ...viewFiles])], parsed.options);
  const checker = program.getTypeChecker();
  const location = (node: any): SourceLocation => {
    const file = node.getSourceFile(); const start = file.getLineAndCharacterOfPosition(node.getStart()); const end = file.getLineAndCharacterOfPosition(node.getEnd());
    return { file: local(file.fileName), line: start.line + 1, column: start.character + 1, endLine: end.line + 1, endColumn: end.character + 1 };
  };
  const report = (node: any, code: string, message: string) => error(code, message, location(node));
  const deref = (symbol: any): any => symbol && symbol.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(symbol) : symbol;
  const symbolAt = (node: any) => node && deref(checker.getSymbolAtLocation(node));
  const reference = (symbol: any): SymbolReference | null => {
    symbol = deref(symbol); const declaration = symbol?.valueDeclaration ?? symbol?.declarations?.[0];
    if (!declaration) return null;
    const names: string[] = [symbol.name];
    for (let parent = declaration.parent; parent; parent = parent.parent) if (ts.isClassDeclaration(parent) && parent.name) names.unshift(parent.name.text);
    const source = location(declaration);
    return { id: `${source.file}#${names.join('.')}`, name: symbol.name, file: source.file, location: source };
  };
  const common = program.getSourceFile(resolve(root, 'common/index.ts'));
  const commonSymbol = common && checker.getSymbolAtLocation(common);
  if (!commonSymbol) { error('COMMON_LIBRARY', 'Cannot resolve the common library at common/index.ts.'); return result; }
  const exports = new Map<string, any>(checker.getExportsOfModule(commonSymbol).map((item: any) => [item.name, deref(item)]));
  const isControlSymbol = (symbol: any): boolean => {
    if (!symbol) return false;
    const seen = new Set<any>();
    const visit = (type: any): boolean => {
      if (!type || seen.has(type)) return false;
      seen.add(type);
      return type.symbol === exports.get('Control') || !!type.getBaseTypes?.().some(visit);
    };
    return visit(checker.getDeclaredTypeOfSymbol(symbol));
  };
  const decorators = (node: any): any[] => ts.canHaveDecorators(node) ? [...ts.getDecorators(node) ?? []] : [];
  const decorator = (node: any, name: string) => decorators(node).find(item => symbolAt(ts.isCallExpression(item.expression) ? item.expression.expression : item.expression) === exports.get(name) && exports.has(name));
  const tokenOf = (item: any) => item && ts.isCallExpression(item.expression) && item.expression.arguments.length === 1 ? symbolAt(item.expression.arguments[0]) : undefined;
  const classes: any[] = [];
  for (const source of program.getSourceFiles()) {
    if (source.isDeclarationFile || !inside(source.fileName) || source.fileName.includes(`${sep}node_modules${sep}`)) continue;
    for (const item of source.statements) if (ts.isClassDeclaration(item) && item.name) classes.push(item);
    for (const item of program.getSyntacticDiagnostics(source)) error('SYNTAX', ts.flattenDiagnosticMessageText(item.messageText, '\n'), item.start !== undefined ? location(source) : undefined);
  }
  const serviceClasses = classes.filter(item => decorator(item, 'Service'));
  const serviceSymbols = new Set(serviceClasses.map(item => symbolAt(item.name)));
  const method = (item: any): MethodModel => {
    const signature = checker.getSignatureFromDeclaration(item);
    return { symbol: reference(symbolAt(item.name))!, parameters: item.parameters.map((parameter: any) => ({ name: parameter.name.getText(), type: checker.typeToString(checker.getTypeAtLocation(parameter)), optional: !!parameter.questionToken || !!parameter.initializer })), returnType: signature ? checker.typeToString(checker.getReturnTypeOfSignature(signature)) : 'unknown', async: !!item.modifiers?.some((modifier: any) => modifier.kind === ts.SyntaxKind.AsyncKeyword) };
  };
  const classModel = (declaration: any): ClassModel => {
    const injections: InjectionModel[] = [];
    for (const member of declaration.members) {
      const injection = decorator(member, 'Inject'); if (!injection) continue;
      const token = tokenOf(injection);
      if (!serviceSymbols.has(token)) report(member, 'SERVICE_TOKEN', '@Inject must reference a declared @Service class symbol.');
      else if (member.type && !checker.isTypeAssignableTo(checker.getDeclaredTypeOfSymbol(token), checker.getTypeAtLocation(member.type))) report(member, 'SERVICE_TYPE', 'Injected service must match the property type.');
      injections.push({ property: reference(symbolAt(member.name))!, service: serviceSymbols.has(token) ? reference(token) : null });
    }
    const serviceCalls: ServiceCall[] = [];
    const serviceAccesses: ServiceAccess[] = [];
    const visit = (node: any): void => {
      if (ts.isCallExpression(node)) {
        const symbol = symbolAt(ts.isPropertyAccessExpression(node.expression) ? node.expression.name : node.expression);
        const target = symbol?.valueDeclaration;
        if (target && ts.isMethodDeclaration(target) && serviceSymbols.has(symbolAt(target.parent.name))) serviceCalls.push({ source: location(node), target: reference(symbol)! });
      }
      if (ts.isPropertyAccessExpression(node)) {
        const symbol = symbolAt(node.name); const target = symbol?.valueDeclaration;
        if (target && (ts.isPropertyDeclaration(target) || ts.isGetAccessorDeclaration(target) || ts.isSetAccessorDeclaration(target)) && serviceSymbols.has(symbolAt(target.parent.name))) {
          const parent = node.parent;
          let operation: ServiceAccess['operation'] = 'read';
          if (ts.isBinaryExpression(parent) && parent.left === node && parent.operatorToken.kind >= ts.SyntaxKind.FirstAssignment && parent.operatorToken.kind <= ts.SyntaxKind.LastAssignment) operation = parent.operatorToken.kind === ts.SyntaxKind.EqualsToken ? 'write' : 'read-write';
          else if ((ts.isPrefixUnaryExpression(parent) || ts.isPostfixUnaryExpression(parent)) && (parent.operator === ts.SyntaxKind.PlusPlusToken || parent.operator === ts.SyntaxKind.MinusMinusToken)) operation = 'read-write';
          serviceAccesses.push({ source: location(node), target: reference(symbol)!, operation });
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(declaration);
    return { symbol: reference(symbolAt(declaration.name))!, injections, methods: declaration.members.filter((item: any) => ts.isMethodDeclaration(item) && item.name && !ts.isComputedPropertyName(item.name)).map(method), serviceCalls, serviceAccesses };
  };
  result.services = serviceClasses.map(classModel);
  const literal = (node: any): { ok: true; value: Literal } | { ok: false } => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return { ok: true, value: node.text };
    if (ts.isNumericLiteral(node)) return { ok: true, value: Number(node.text) };
    if (node.kind === ts.SyntaxKind.TrueKeyword || node.kind === ts.SyntaxKind.FalseKeyword) return { ok: true, value: node.kind === ts.SyntaxKind.TrueKeyword };
    if (node.kind === ts.SyntaxKind.NullKeyword) return { ok: true, value: null };
    if (ts.isPrefixUnaryExpression(node) && (node.operator === ts.SyntaxKind.MinusToken || node.operator === ts.SyntaxKind.PlusToken) && ts.isNumericLiteral(node.operand)) return { ok: true, value: (node.operator === ts.SyntaxKind.MinusToken ? -1 : 1) * Number(node.operand.text) };
    if (ts.isArrayLiteralExpression(node)) { const values = node.elements.map(literal); return values.every((item: any) => item.ok) ? { ok: true, value: values.map((item: any) => item.value) } : { ok: false }; }
    return { ok: false };
  };
  const paired = new Set<any>();
  for (const layoutFile of viewFiles.filter(file => file.endsWith('.tsx')).sort()) {
    const name = basename(layoutFile, '.tsx'); const classFile = resolve(dirname(layoutFile), `${name}.ts`);
    const source = program.getSourceFile(classFile); const layoutSource = program.getSourceFile(layoutFile);
    const declaration = source?.statements.find((item: any) => ts.isClassDeclaration(item) && item.name?.text === name && decorator(item, 'ViewClass'));
    if (!declaration || !layoutSource) { error('VIEW_PAIR', `${local(layoutFile)} requires @ViewClass ${name} in its paired ${name}.ts.`); continue; }
    paired.add(declaration);
    if (basename(dirname(layoutFile)) !== name) report(declaration, 'VIEW_DIRECTORY', 'Each view must be in its own same-named directory.');
    const view = { ...classModel(declaration), classFile: local(classFile), layoutFile: local(layoutFile), properties: {}, outlets: [], actions: [], layout: null } as ViewModel;
    const outletSymbols = new Map<any, OutletModel>(); const actionSymbols = new Set<any>(); const used = new Set<any>();
    for (const member of declaration.members) {
      const outlet = decorator(member, 'ViewOutlet'); const memberSymbol = symbolAt(member.name);
      if (outlet) { const token = tokenOf(outlet); const model = { property: reference(memberSymbol)!, control: reference(token) }; outletSymbols.set(memberSymbol, model); view.outlets.push(model); if (!token || !model.control || !isControlSymbol(token)) report(member, 'OUTLET_TOKEN', '@ViewOutlet requires a control constructor symbol.'); }
      if (decorator(member, 'ViewAction')) { actionSymbols.add(memberSymbol); view.actions.push(reference(memberSymbol)!); }
      if (ts.isPropertyDeclaration(member) && ['title', 'width', 'height'].includes(member.name.getText()) && member.initializer) { const value = literal(member.initializer); if (value.ok) view.properties[member.name.getText()] = value.value; else report(member, 'WINDOW_PROPERTY', 'Initial window properties must be literal values.'); }
    }
    const layoutFunction = layoutSource.statements.find((item: any) => ts.isFunctionDeclaration(item) && item.name?.text === 'layout' && item.modifiers?.some((modifier: any) => modifier.kind === ts.SyntaxKind.ExportKeyword));
    if (!layoutFunction?.body || layoutFunction.parameters.length !== 1 || checker.getTypeAtLocation(layoutFunction.parameters[0]).symbol !== symbolAt(declaration.name)) { report(layoutSource, 'LAYOUT_FUNCTION', `Export layout(view: ${name}): LSNode from the paired layout.`); result.views.push(view); continue; }
    const parameterSymbol = symbolAt(layoutFunction.parameters[0].name);
    const binding = (expression: any) => ts.isPropertyAccessExpression(expression) && symbolAt(expression.expression) === parameterSymbol ? symbolAt(expression.name) : undefined;
    const parseControl = (node: any): ControlModel | null => {
      while (ts.isParenthesizedExpression(node)) node = node.expression;
      if (!ts.isJsxElement(node) && !ts.isJsxSelfClosingElement(node)) { report(node, 'DYNAMIC_LAYOUT', 'Layout must be a static JSX control tree.'); return null; }
      const opening = ts.isJsxElement(node) ? node.openingElement : node;
      const componentSymbol = symbolAt(opening.tagName); const component = reference(componentSymbol);
      if (!component || ![...exports.values()].includes(componentSymbol) || !isControlSymbol(componentSymbol)) report(opening.tagName, 'CONTROL_SYMBOL', 'Control must resolve to an exported common-library control component.');
      const control: ControlModel = { component, location: location(node), properties: {}, events: {}, children: [] };
      for (const attribute of opening.attributes.properties) {
        if (!ts.isJsxAttribute(attribute)) { report(attribute, 'DYNAMIC_PROPERTY', 'Spread attributes cannot be extracted.'); continue; }
        const key = attribute.name.getText(); const expression = attribute.initializer && ts.isJsxExpression(attribute.initializer) ? attribute.initializer.expression : attribute.initializer;
        if (key === 'outlet' || /^on[A-Z]/.test(key)) {
          const symbol = expression && binding(expression);
          if (key === 'outlet') {
            const outlet = outletSymbols.get(symbol);
            if (!outlet) report(attribute, 'OUTLET_REFERENCE', 'outlet must directly reference a @ViewOutlet on the paired view.');
            else { control.outlet = outlet.property; if (used.has(symbol)) report(attribute, 'DUPLICATE_OUTLET', 'An outlet can only be connected once.'); used.add(symbol); if (outlet.control?.id !== component?.id) report(attribute, 'OUTLET_TYPE', 'The outlet constructor must match its JSX component.'); }
          } else if (!actionSymbols.has(symbol)) report(attribute, 'ACTION_REFERENCE', 'Event connections must directly reference a @ViewAction on the paired view.');
          else control.events[key] = reference(symbol)!;
        } else if (!expression) control.properties[key] = true;
        else { const value = literal(expression); if (value.ok) control.properties[key] = value.value; else report(attribute, 'DYNAMIC_PROPERTY', 'Control properties must be literals; calls and computed values cannot be extracted.'); }
      }
      if (ts.isJsxElement(node)) for (const child of node.children) {
        if (ts.isJsxText(child) && !child.text.trim()) continue;
        if (ts.isJsxElement(child) || ts.isJsxSelfClosingElement(child)) { const nested = parseControl(child); if (nested) control.children.push(nested); }
        else report(child, 'DYNAMIC_LAYOUT', 'Control children must be static JSX controls.');
      }
      return control;
    };
    const statements = layoutFunction.body.statements;
    if (statements.length !== 1 || !ts.isReturnStatement(statements[0]) || !statements[0].expression) report(layoutFunction, 'DYNAMIC_LAYOUT', 'layout must contain one static JSX return statement.');
    else view.layout = parseControl(statements[0].expression);
    for (const [symbol, outlet] of outletSymbols) if (!used.has(symbol)) report(symbol.valueDeclaration, 'UNCONNECTED_OUTLET', `Outlet ${outlet.property.name} has no layout connection.`);
    result.views.push(view);
  }
  for (const declaration of classes) if (decorator(declaration, 'ViewClass') && viewFiles.includes(declaration.getSourceFile().fileName) && !paired.has(declaration)) report(declaration, 'VIEW_PAIR', 'View requires a same-name .tsx layout in its own directory.');
  result.services.sort((a, b) => a.symbol.id.localeCompare(b.symbol.id));
  return result;
}
