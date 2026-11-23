import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { generateViewModule, validateProject } from './validate';

const temporary: string[] = [];
afterEach(() => { for (const root of temporary.splice(0)) rmSync(root, { recursive: true, force: true }); });

function fixture(layout = '<Button outlet={view.button} onClick={view.submit} />', action = '@ViewAction', injection = '@Inject(CustomerService)'): string {
  const root = mkdtempSync(resolve(tmpdir(), 'ls-views-'));
  temporary.push(root);
  const write = (path: string, contents: string) => { const file = resolve(root, path); mkdirSync(resolve(file, '..'), { recursive: true }); writeFileSync(file, contents); };
  write('tsconfig.json', JSON.stringify({ compilerOptions: { target: 'ES2022', module: 'ESNext', moduleResolution: 'Bundler', jsx: 'preserve', experimentalDecorators: true }, include: ['common', 'views'] }));
  write('common/index.ts', `
export function ViewClass(target:any) {}
export function ViewOutlet(token:any) { return (target:any,key:any) => {}; }
export function ViewAction(target:any,key:any,descriptor:any) {}
export function Service(target:any) {}
export function Inject(token:any) { return (target:any,key:any) => {}; }
export class View {}
export class Button {}
`);
  write('views/MyDash/MyDash.ts', `
import {ViewClass, ViewOutlet, ViewAction, View, Button, Service, Inject} from '../../common';
@Service class CustomerService {}
@ViewClass export class MyDash extends View {
  @ViewOutlet(Button) declare button: Button;
  ${injection} declare service: CustomerService;
  ${action} submit():void {}
}`);
  write('views/MyDash/MyDash.tsx', `
import { Button } from '../../common';
import type { MyDash } from './MyDash';
export function layout(view:MyDash) { return ${layout}; }
`);
  return root;
}

describe('paired view validation', () => {
  test('resolves member connections and generates class/layout pairing', () => {
    const root = fixture();
    const result = validateProject(root);
    expect(result.errors).toEqual([]);
    expect(result.pairs).toHaveLength(1);
    expect(generateViewModule(root, result.pairs)).toContain('export const MyDashView = { view: View0, layout: Layout0 }');
  });
  test('rejects a callback without the action declaration', () => {
    expect(validateProject(fixture(undefined, '')).errors.join('\n')).toContain('must reference a @ViewAction');
  });
  test('rejects strings and duplicate connections', () => {
    const result = validateProject(fixture('<><Button outlet={view.button} /><Button outlet={view.button} onClick="submit" /></>'));
    expect(result.errors.join('\n')).toContain('connected only once');
    expect(result.errors.join('\n')).toContain('must reference a declared view member directly');
  });
  test('rejects missing pairs and unresolved outlets', () => {
    const root = fixture('<Button />');
    expect(validateProject(root).errors.join('\n')).toContain('has no layout connection');
    rmSync(resolve(root, 'views/MyDash/MyDash.tsx'));
    expect(validateProject(root).errors.join('\n')).toContain('requires a paired same-name');
  });
  test('rejects an outlet connected to a different component type', () => {
    const root = fixture('<View outlet={view.button} />');
    expect(validateProject(root).errors.join('\n')).toContain('must match its @ViewOutlet constructor token');
  });
  test('rejects service identifiers authored as strings', () => {
    expect(validateProject(fixture(undefined, undefined, '@Inject("CustomerService")')).errors.join('\n')).toContain('@Inject requires one @Service class symbol');
  });
});
