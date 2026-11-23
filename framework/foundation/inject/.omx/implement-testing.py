from pathlib import Path
# Targeted registry checkpoints let overrides restore only their token, not unrelated mutations.
f=Path('src/core/cminject.container.ts');s=f.read_text().replace('  owner: Container;\n  scopes:', '  owner: Container;\n  target?: symbol;\n  scopes:')
pos=s.index('  async restore(snapshot: RegistrySnapshot)');s=s[:pos]+'''  /** Internal token-local checkpoint used by the testing entry. */
  snapshotBinding(token: InjectionToken): RegistrySnapshot {
    const key = lookupToken(token) ?? internToken(token);
    const owner = this.ancestors().reverse().find(scope => scope.order.some(binding => binding.key === key)) ?? this;
    const snapshot = owner.snapshot(); const captured = registrySnapshots.get(snapshot)!;
    captured.target = key; captured.modules = undefined;
    for (const state of captured.scopes) {
      state.definitions = state.definitions.filter(binding => binding.key === key);
      state.records = new Map([...state.records].filter(([binding]) => binding.key === key));
    }
    return snapshot;
  }
  /** Internal owner routing is limited to a genuine checkpoint from this hierarchy. */
  restoreBinding(snapshot: RegistrySnapshot): Promise<void> {
    const captured = registrySnapshots.get(snapshot);
    if (!captured || captured.owner.rootContainer !== this.rootContainer || captured.target === undefined) throw new MutationConflictError('Foreign binding checkpoint');
    return captured.owner.restore(snapshot);
  }
'''+s[pos:]
s=s.replace('for (const binding of [...scope.order].reverse()) if (!keep.has(binding))', 'for (const binding of [...scope.order].reverse()) if ((captured.target === undefined || binding.key === captured.target) && !keep.has(binding))')
s=s.replace("if (state?.records.get(record.binding) !== record)","if ((captured.target === undefined || record.binding.key === captured.target) && state?.records.get(record.binding) !== record)")
s=s.replace('scope.epoch = state.epoch; scope.observation.clearRuntime();', 'if (captured.target === undefined) scope.epoch = state.epoch;\n        scope.observation.clearRuntime();')
s=s.replace('      else this.rootContainer.moduleRegistry?.reset();','      else if (captured.target === undefined) this.rootContainer.moduleRegistry?.reset();')
f.write_text(s)
Path('src/cminject.testing.ts').write_text('''import { Container } from '@coldmind/inject/container';
import { ContainerView } from '@coldmind/inject/internal/view';
import { MutationConflictError } from '@coldmind/inject/errors';
import { ServiceLifetime, type IContainer, type ServiceFactory, type ServiceLifecycle } from '@coldmind/inject/contracts';
import { lookupToken, type InjectionToken } from '@coldmind/inject/token';
import type { ModuleDefinition } from '@coldmind/inject/module';

/** An awaited override; restore is idempotent and nested overrides restore in LIFO order. */
export interface OverrideHandle { restore(): Promise<void>; }
/** Explicit factory form also supports services whose value itself is callable. */
export interface FactoryOverride<T> { readonly factory: ServiceFactory<T> | ServiceLifecycle<T>; }
/** Explicit value form disambiguates callable service values. */
export interface ValueOverride<T> { readonly value: T; }
const stacks = new WeakMap<Container, Map<symbol, OverrideHandle[]>>();
/** Replaces one definition after saving it, never reuses a disposed factory-produced instance. */
export async function override<T>(container: IContainer, token: InjectionToken<T>, replacement: T | ServiceFactory<T> | FactoryOverride<T> | ValueOverride<T>): Promise<OverrideHandle> {
  const target = container instanceof ContainerView ? container.target : container;
  if (!(target instanceof Container)) throw new TypeError('Overrides require a cminject container');
  const old = target.bindingOf(token);
  if (old?.kindTag === 'registerInstance' && old.owner.ownership === 'owned') throw new MutationConflictError('Cannot override an owned supplied instance: its disposed value cannot be reconstructed');
  const snapshot = target.snapshotBinding(token);
  let factory: ServiceFactory<T> | ServiceLifecycle<T>;
  if (replacement && typeof replacement === 'object' && 'value' in replacement) factory = () => replacement.value as T;
  else if (replacement && typeof replacement === 'object' && 'factory' in replacement) factory = replacement.factory as ServiceFactory<T> | ServiceLifecycle<T>;
  else if (typeof replacement === 'function') factory = replacement as ServiceFactory<T>;
  else factory = () => replacement as T;
  await target.replace(token, factory, old?.lifetime ?? ServiceLifetime.Singleton, { retainPrevious: false });
  const key = lookupToken(token)!;
  let byToken = stacks.get(target); if (!byToken) stacks.set(target, byToken = new Map());
  let stack = byToken.get(key); if (!stack) byToken.set(key, stack = []);
  let done = false; let restoring: Promise<void> | undefined;
  const handle: OverrideHandle = { restore() {
    if (done) return Promise.resolve();
    if (restoring) return restoring;
    if (stack!.at(-1) !== handle) return Promise.reject(new MutationConflictError('Overrides must restore in LIFO order'));
    restoring = target.restoreBinding(snapshot).then(() => { done = true; stack!.pop(); if (!stack!.length) byToken!.delete(key); }, error => { restoring = undefined; throw error; });
    return restoring;
  } };
  stack.push(handle); return handle;
}
/** Isolated root with bounded builds; module registration is awaited before the root is exposed. */
export async function createTestContainer(...modules: ModuleDefinition[]): Promise<Container> {
  const root = new Container(); root.buildTimeout = 2000;
  try { await root.load(...modules); return root; }
  catch (error) { await root.dispose(); throw error; }
}
''')
# Basic integrated tests, written now; executed only in the final phase.
Path('tests/bun/core/delivery.test.ts').write_text('''import { expect, test } from 'vitest';
import { Container, createToken, defineModule, composeModules, lazy, provider, qualify, tokenName, DiGraphError, MutationConflictError } from '@coldmind/inject';
import { override, createTestContainer } from '@coldmind/inject/testing';

test('module callbacks capture sync and awaited registrations, preserving unrelated work', async () => {
  const root = new Container(); let release!: () => void; const gate = new Promise<void>(done => { release = done; });
  const module = defineModule({ name: 'async', async register(c) { c.singleton('a', () => 1); await gate; c.singleton('b', () => 2); } });
  const pending = root.load(module); await Promise.resolve(); await Promise.resolve(); root.singleton('outside', () => 3); release();
  const [handle] = await pending; expect(root.resolveSync('a')).toBe(1); expect(root.resolveSync('b')).toBe(2);
  await handle!.unload(); expect(root.has('a')).toBe(false); expect(root.has('b')).toBe(false); expect(root.resolveSync('outside')).toBe(3); await root.dispose();
});
test('concurrent module loads join and repeated loads return one handle', async () => {
  const root = new Container(); let calls = 0; const module = defineModule({ name: 'once', async register(c) { calls++; await Promise.resolve(); c.singleton('once', () => 1); } });
  const [a,b] = await Promise.all([root.load(module),root.load(module)]); expect(calls).toBe(1); expect(a[0]).toBe(b[0]); await a[0]!.unload(); await root.dispose();
});
test('failed modules roll back and retry', async () => {
  const root = new Container(); let fail = true; const module = defineModule({ name: 'retry', async register(c) { c.singleton('retry', () => 1); await Promise.resolve(); if (fail) throw new Error('bad'); } });
  await expect(root.load(module)).rejects.toThrow('bad'); expect(root.has('retry')).toBe(false); fail=false; await root.load(module); expect(root.resolveSync('retry')).toBe(1); await root.dispose();
});
test('shared imports survive until the last dependent unloads', async () => {
  const root = new Container(); const shared=defineModule({ name:'shared',register(c){c.singleton('shared',()=>1);} });
  const a=defineModule({name:'a',imports:[shared],register(){}}); const b=defineModule({name:'b',imports:[shared],register(){}});
  const [ha,hb]=await root.load(a,b); await ha!.unload(); expect(root.has('shared')).toBe(true); await hb!.unload(); expect(root.has('shared')).toBe(false); await root.dispose();
});
test('module import cycles are diagnosed', async () => {
  const root=new Container(); const imports: ReturnType<typeof defineModule>[]=[]; const a=defineModule({name:'cycle',imports,register(){}}); imports.push(a);
  await expect(root.load(a)).rejects.toMatchObject({issues:[{kind:'module-cycle'}]}); await root.dispose();
});
test('descendant module loads register at the root and annotate missing-edge diagnostics', async () => {
  const root=new Container(); const child=root.createScope(); await child.load(defineModule({name:'labelled',register(c){c.singleton('broken',()=>1,{deps:['absent']});}}));
  expect(root.bindingOf('broken')?.module).toBe('labelled'); expect(root.graph().issues()[0]?.message).toContain('labelled'); await root.dispose();
});
test('snapshots remove later definitions and dispose only later instances', async () => {
  const root=new Container(); let disposals=0; root.singleton('before',()=>({})); const value=root.resolveSync('before'); const snapshot=root.snapshot();
  root.scoped('later',()=>({onDispose(){disposals++;}})); root.resolveSync('later'); await root.restore(snapshot);
  expect(root.has('later')).toBe(false); expect(root.get('before')).toBe(value); expect(disposals).toBe(1); await root.dispose();
});
test('foreign snapshots are rejected without mutation', async () => {
  const first=new Container(),second=new Container(); const snapshot=first.snapshot(); second.singleton('safe',()=>1); await expect(second.restore(snapshot)).rejects.toBeInstanceOf(MutationConflictError); expect(second.resolveSync('safe')).toBe(1); await first.dispose(); await second.dispose();
});
test('owned supplied instances preserve live identity and reject disposed snapshot restoration atomically', async () => {
  const root=new Container(); let disposed=0; const value={onDispose(){disposed++;}}; root.registerInstance('owned',value); const snapshot=root.snapshot(); await root.restore(snapshot); expect(root.get('owned')).toBe(value);
  await root.unregister('owned'); root.singleton('other',()=>2); const epoch=root.getRegistryEpoch(); await expect(root.restore(snapshot)).rejects.toBeInstanceOf(MutationConflictError); expect(root.getRegistryEpoch()).toBe(epoch); expect(root.resolveSync('other')).toBe(2); expect(disposed).toBe(1); await root.dispose();
});
test('nested overrides enforce LIFO, rebuild originals, and keep unrelated registrations', async () => {
  const root=new Container(); let builds=0; root.singleton('clock',()=>({id:++builds})); root.resolveSync('clock'); const first=await override(root,'clock',{id:9}); const second=await override(root,'clock',{id:10});
  await expect(first.restore()).rejects.toBeInstanceOf(MutationConflictError); expect((await root.resolve<{id:number}>('clock')).id).toBe(10); await second.restore(); expect((await root.resolve<{id:number}>('clock')).id).toBe(9);
  root.singleton('unrelated',()=>7); await first.restore(); expect((await root.resolve<{id:number}>('clock')).id).toBe(2); expect(root.resolveSync('unrelated')).toBe(7); await root.dispose();
});
test('owned instance overrides reject before disposal and callable values use explicit value tags', async () => {
  const root=new Container(); let count=0; const value={onDispose(){count++;}}; root.registerInstance('owned',value); await expect(override(root,'owned',{})).rejects.toBeInstanceOf(MutationConflictError); expect(root.get('owned')).toBe(value); expect(count).toBe(0);
  const token=createToken<()=>number>('callable'); root.singleton(token,()=>()=>1); const handle=await override(root,token,{value:()=>2}); expect((await root.resolve(token))()).toBe(2); await handle.restore(); expect((await root.resolve(token))()).toBe(1); await root.dispose();
});
test('test roots await modules and inherit a two-second timeout', async () => {
  const root=await createTestContainer(defineModule({name:'test',async register(c){await Promise.resolve();c.singleton('ready',()=>1);}})); expect(root.buildTimeout).toBe(2000); expect(root.createScope().buildTimeout).toBe(2000); expect(root.resolveSync('ready')).toBe(1); await root.dispose();
});
test('typed factories preserve argument order and providers preserve transient lifetime', async () => {
  const root=new Container(); const N=createToken<number>('N'), S=createToken<string>('S'), T=createToken<{n:number}>('T'), Out=createToken<string>('Out'); let next=0;
  root.singleton(N,()=>3); root.singleton(S,()=> 'x'); root.transient(T,()=>({n:++next})); root.singleton(Out,{inject:[N,S,provider(T)] as const,async factory(n,s,make){return `${n}${s}:${(await make()).n},${(await make()).n}`;}});
  expect(await root.resolve(Out)).toBe('3x:1,2'); await root.dispose();
});
test('lazy slots break eager constructor cycles and stay bound to their child scope', async () => {
  class A { static $inject=[lazy(B)]; constructor(readonly b: import('@coldmind/inject').Lazy<B>){} }
  class B { static $inject=[A]; constructor(readonly a:A){} }
  const root=new Container(); root.scoped(A); root.scoped(B); const child=root.createScope(); const a=child.resolveSync(A); expect(a.b.peek()).toBeUndefined(); expect(a.b.getSync().a).toBe(a); await child.dispose(); await expect(a.b.get()).rejects.toThrow(); await root.dispose();
});
test('lazy captive edges are still reported', async () => {
  const root=new Container(); root.request('request',()=>1); root.singleton('single',{inject:[lazy('request')] as const,factory: value=>value}); expect(()=>root.validate()).toThrow(DiGraphError); await root.dispose();
});
test('qualified tokens retain identity and readable names', async () => {
  const root=new Container(); const Db=createToken<string>('Db'); const replica=qualify(Db,'replica'); expect(qualify(Db,'replica')).toBe(replica); expect(tokenName(replica)).toBe('Db#replica'); root.singleton(Db,()=> 'main'); root.singleton(replica,()=> 'replica'); expect(root.resolveSync(replica)).toBe('replica'); await root.dispose();
});
test('conditions are evaluated in order before fallback with isolated candidate caches', async () => {
  const root=new Container(); const label=createToken<string>('label'); root.singleton(label,()=> 'fallback'); root.singleton(label,()=> 'one',{when:ctx=>ctx.requester?.tokenName==='one'}); root.singleton(label,()=> 'two',{when:ctx=>ctx.requester?.tokenName==='two'});
  root.singleton('one',{inject:[label] as const,factory:value=>value}); root.singleton('two',{inject:[label] as const,factory:value=>value}); expect(root.resolveSync('one')).toBe('one'); expect(root.resolveSync('two')).toBe('two'); expect(root.resolveSync(label)).toBe('fallback'); await root.dispose();
});
test('no matching condition rejects with a diagnostic while static graph never invokes predicates', async () => {
  const root=new Container(); let calls=0; root.singleton('conditional',()=>1,{when(){calls++;return false;}}); root.graph().issues(); expect(calls).toBe(0); await expect(root.resolve('conditional')).rejects.toThrow(/conditions/); await root.dispose();
});
test('undeclared dependencies are opt-in and observed drift is reported', async () => {
  const root=new Container(); root.singleton('b',()=>1,{deps:[]}); root.singleton('a',c=>c.resolveSync('b')); root.validate(); expect(()=>root.validate({requireDeps:true})).toThrow(DiGraphError); root.resolveSync('a'); expect(()=>root.validate({includeObserved:true})).toThrow(/undeclared/); await root.dispose();
});
test('typed module composition loads real imports', async () => {
  const root=new Container(); const provider=defineModule<{db:number},{}>()({name:'provider',register(c){c.singleton('db',()=>7);}}); const consumer=defineModule<{}, {db:number}>()({name:'consumer',register(){}}); const composed=composeModules(provider,consumer); await root.load(composed); expect(root.resolveSync('db')).toBe(7); await root.dispose();
});
''')
# Prepared leaf/compiler fixtures become active rather than remaining drafts.
for source,dest in [('module-definition.test.ts','tests/bun/core/module-definition.test.ts'),('module-types.test-d.ts','tests/types/module-types.test-d.ts')]:
 p=Path('.omx/prepared-tests')/source
 if p.exists():
  text=p.read_text();text=text.replace("../../src/core/cminject.module.ts",'@coldmind/inject/module').replace('@coldmind/inject/src/core/cminject.module.ts','@coldmind/inject/module');Path(dest).write_text(text)
# All guide examples execute in the final suite.
manifest=__import__('json').loads(Path('.omx/evidence/prio-one/examples-prep/manifest.json').read_text());Path('tests/bun/docs').mkdir(parents=True,exist_ok=True)
lines=["import { expect, test } from 'vitest';"]
for i,item in enumerate(manifest['examples']):lines.append(f"import {{ runExample as example{i} }} from '../../../{item['filename']}';")
for i,item in enumerate(manifest['examples']):lines.append(f"test({item['guide']!r}, async () => {{ expect(await example{i}()).toBe({item['expected']!r}); }});")
Path('tests/bun/docs/examples.test.ts').write_text('\n'.join(lines)+'\n')
print('Wrote testing utilities and basic integrated tests; none executed.')
