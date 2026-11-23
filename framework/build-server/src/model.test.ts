import { afterEach, describe, expect, test } from 'bun:test';
import { cpSync, mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { extractProject, hasErrors, type ControlModel } from './model';

const temporary: string[] = [];
const template = resolve(import.meta.dir, '../../templates/lsapp');
function fixture(): string {
  const root = mkdtempSync(resolve(tmpdir(), 'ls-model-'));
  temporary.push(root);
  cpSync(template, root, { recursive: true, filter: path => !path.includes('/dist') && !path.includes('node_modules') });
  return root;
}
function change(root: string, file: string, old: string, next: string): void {
  const path = resolve(root, file);
  writeFileSync(path, readFileSync(path, 'utf8').replace(old, next));
}
function flatten(node: ControlModel): ControlModel[] { return [node, ...node.children.flatMap(flatten)]; }
afterEach(() => { for (const root of temporary.splice(0)) rmSync(root, { recursive: true, force: true }); });

describe('static project extraction', () => {
  test('extracts paired views, literal properties, symbol connections and service dependencies', () => {
    const model = extractProject(fixture());
    expect(model.diagnostics).toEqual([]);
    expect(model.config?.entry).toBe('application.main.ts');
    expect(model.views).toHaveLength(1);
    const view = model.views[0]!;
    expect(view.properties).toEqual({ title: 'Counter', width: 680, height: 480 });
    expect(view.outlets).toHaveLength(4);
    const button = flatten(view.layout!).find(node => node.properties.caption === 'Increase')!;
    expect(button.component?.id).toBe('common/controls.ts#Button');
    expect(button.events.onClick?.id).toBe('views/MyDash/MyDash.ts#MyDash.increment');
    expect(button.outlet?.name).toBe('incrementButton');
    expect(view.injections[0]?.service?.id).toBe('services/CounterService.ts#CounterService');
    expect(view.serviceCalls.map(call => call.target.name)).toEqual(['increment', 'reset']);
    expect(view.serviceAccesses.map(access => [access.target.id, access.operation])).toEqual([
      ['services/CounterService.ts#CounterService.value', 'read'],
      ['services/CounterService.ts#CounterService.value', 'read'],
    ]);
    expect(model.services[0]?.methods.map(method => method.symbol.name)).toEqual(['increment', 'reset']);
    expect(button.location.file).toBe('views/MyDash/MyDash.tsx');
    expect(button.location.line).toBeGreaterThan(0);
  });

  test('resolves imported symbol aliases without relying on decorator spelling', () => {
    const root = fixture();
    change(root, 'views/MyDash/MyDash.ts', 'ViewAction, ViewClass, ViewOutlet,', 'ViewAction as Action, ViewClass, ViewOutlet,');
    const path = resolve(root, 'views/MyDash/MyDash.ts');
    writeFileSync(path, readFileSync(path, 'utf8').replaceAll('@ViewAction', '@Action'));
    const model = extractProject(root);
    expect(model.diagnostics).toEqual([]);
    expect(model.views[0]?.actions.map(action => action.name)).toEqual(['increment', 'reset']);
  });

  test('never executes application modules while analysing them', () => {
    const root = fixture();
    writeFileSync(resolve(root, 'application.main.ts'), 'throw new Error("Project code must never execute during extraction");');
    expect(hasErrors(extractProject(root))).toBe(false);
  });

  test('rejects computed layout properties and callback connections with source locations', () => {
    const root = fixture();
    change(root, 'views/MyDash/MyDash.tsx', 'caption="Increase" onClick={view.increment}', 'caption={String(42)} onClick={() => view.increment()}');
    const model = extractProject(root);
    expect(hasErrors(model)).toBe(true);
    expect(model.diagnostics.map(item => item.code)).toContain('DYNAMIC_PROPERTY');
    expect(model.diagnostics.map(item => item.code)).toContain('ACTION_REFERENCE');
    expect(model.diagnostics.every(item => item.location?.file === 'views/MyDash/MyDash.tsx')).toBe(true);
  });

  test('rejects missing pairs and string injection targets', () => {
    const root = fixture();
    change(root, 'views/MyDash/MyDash.ts', '@Inject(CounterService)', '@Inject("CounterService")');
    expect(extractProject(root).diagnostics.map(item => item.code)).toContain('SERVICE_TOKEN');
    unlinkSync(resolve(root, 'views/MyDash/MyDash.tsx'));
    expect(extractProject(root).diagnostics.map(item => item.code)).toContain('VIEW_PAIR');
  });

  test('distinguishes service property writes from reads and compound updates', () => {
    const root = fixture();
    change(root, 'services/CounterService.ts', 'private count = 0;', 'count = 0;');
    change(root, 'views/MyDash/MyDash.ts', 'this.counter.increment();', 'this.counter.count = 2; this.counter.count += 1; this.counter.count++;');
    const view = extractProject(root).views[0]!;
    expect(view.serviceAccesses.filter(access => access.target.name === 'count').map(access => access.operation)).toEqual(['write', 'read-write', 'read-write']);
    expect(view.serviceAccesses[0]?.source.file).toBe('views/MyDash/MyDash.ts');
  });

  test('reports malformed config and refuses paths outside the project', () => {
    const root = fixture();
    writeFileSync(resolve(root, 'lsapp.project.json'), '{');
    expect(extractProject(root).diagnostics[0]?.code).toBe('PROJECT_CONFIG');
    writeFileSync(resolve(root, 'lsapp.project.json'), JSON.stringify({ name: 'Test', entry: '../outside.ts', views: 'views', output: 'dist' }));
    expect(extractProject(root).diagnostics[0]?.code).toBe('PROJECT_PATH');
  });
});
