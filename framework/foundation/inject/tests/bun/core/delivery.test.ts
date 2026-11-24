import { expect, test } from 'vitest';
import { Container, createToken, delay, defineModule, composeModules, lazy, provider, qualify, tokenName, DiGraphError, MutationConflictError } from '@coldmind/inject';
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
  const token=createToken<()=>number>('callable'); root.singleton(token,()=>()=>1); const handle=await override(root,token,{kind:'value',value:()=>2}); expect((await root.resolve(token))()).toBe(2); await handle.restore(); expect((await root.resolve(token))()).toBe(1); await root.dispose();
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
  class A { static $inject=[lazy(delay(() => B))]; constructor(readonly b: import('@coldmind/inject').Lazy<B>){} }
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
  const root=new Container(); const label=createToken<string>('label'); root.singleton(label,()=> 'fallback'); root.singleton(label,()=> 'one',{when:ctx=>ctx.requester?.tokenName===tokenName('one')}); root.singleton(label,()=> 'two',{when:ctx=>ctx.requester?.tokenName===tokenName('two')});
  root.singleton('one',{inject:[label] as const,factory:value=>value}); root.singleton('two',{inject:[label] as const,factory:value=>value}); expect(root.resolveSync('one')).toBe('one'); expect(root.resolveSync('two')).toBe('two'); expect(root.resolveSync(label)).toBe('fallback'); const graph=root.getDependencyGraphSnapshot(); const providers=graph.nodes.filter(node=>node.token===tokenName(label)); const one=graph.nodes.find(node=>node.token===tokenName('one'))!; expect(graph.edges.filter(edge=>edge.from===one.bindingId && providers.some(node=>node.bindingId===edge.to) && edge.type==='declared')).toHaveLength(3); await root.dispose();
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

test('snapshot restores external values without disposal and module ownership survives restoration', async () => {
  const root=new Container(); let disposed=0; const value={onDispose(){disposed++;}}; root.registerInstance('external',value,{ownership:'external'});
  const module=defineModule({name:'restore-module',register(c){c.singleton('module-value',()=>1);}}); await root.load(module); const snapshot=root.snapshot();
  await root.replace('external',()=>({onDispose(){disposed++;}}),0); await root.restore(snapshot); expect(root.get('external')).toBe(value); expect(disposed).toBe(0);
  const [handle]=await root.load(module); await handle!.unload(); expect(root.has('module-value')).toBe(false); await root.dispose(); expect(disposed).toBe(0);
});
test('module unload aggregates cleanup failures and still removes every binding', async () => {
  const root=new Container(); const module=defineModule({name:'cleanup',register(c){c.singleton('bad',()=>({onDispose(){throw new Error('cleanup');}}));c.singleton('good',()=>({}));}});
  const [handle]=await root.load(module); root.resolveSync('bad');root.resolveSync('good');await expect(handle!.unload()).rejects.toBeInstanceOf(AggregateError);expect(root.has('bad')).toBe(false);expect(root.has('good')).toBe(false);await root.dispose();
});
test('static conditional candidates remain subject to conservative captive validation', async () => {
  const root=new Container();root.singleton('choice',()=>1);root.request('choice',()=>2,{when:()=>false});root.singleton('consumer',()=>3,{deps:['choice']});expect(()=>root.validate()).toThrow(DiGraphError);await root.dispose();
});

test('module facade captures every lifetime, multi, supplied and class registration path', async () => {
  const root=new Container(); const scope=root.createTenantScope('all-methods').createRequestScope();
  const methods=['singleton','transient','scoped','tenant','request','singletonMulti','transientMulti','scopedMulti','tenantMulti','requestMulti'] as const;
  class Registered { static $injectable={}; }
  let retained: import('@coldmind/inject').IContainer | undefined;
  const [handle]=await root.load(defineModule({name:'every-registration',register(c){
    retained=c;
    for(const method of methods)c[method](method,()=>method);
    c.registerInstance('supplied',9,{ownership:'external'});c.registerClass(Registered);
  }}));
  for(const method of methods){if(method.endsWith('Multi'))expect(await scope.resolveAll(method)).toEqual([method]);else expect(await scope.resolve(method)).toBe(method);}
  expect(root.get('supplied')).toBe(9);expect(root.resolveSync(Registered)).toBeInstanceOf(Registered);
  expect(()=>retained!.singleton('expired',()=>1)).toThrow(/completed/);
  await handle!.unload();for(const method of methods)expect(root.has(method)).toBe(false);expect(root.has('supplied')).toBe(false);expect(root.has(Registered)).toBe(false);await root.dispose();
});
test('module facade rejects uncaptured scope and destructive mutation operations before changing the root', async () => {
  const root=new Container();root.singleton('untouched',()=>1);
  await root.load(defineModule({name:'restricted',register(c){
    expect(()=>c.createScope()).toThrow();expect(()=>c.createTenantScope()).toThrow();expect(()=>c.createRequestScope()).toThrow();
    expect(()=>c.snapshot()).toThrow();expect(()=>c.load()).toThrow();expect(()=>c.clearInstances()).toThrow();expect(()=>c.dispose()).toThrow();
    expect(()=>c.unregister('untouched')).toThrow();expect(()=>c.replace('untouched',()=>2,0)).toThrow();
  }}));expect(root.resolveSync('untouched')).toBe(1);await root.dispose();
});

test('root disposal rejects pending module loads even when registration ignores shutdown', async () => {
  const root=new Container();let entered!:()=>void;const started=new Promise<void>(resolve=>{entered=resolve;});
  const module=defineModule({name:'pending-dispose',async register(c){c.singleton('pending-module-value',()=>1);entered();await new Promise(()=>{});}});
  const pending=root.load(module);const rejected=expect(pending).rejects.toThrow(/disposed/);await started;await root.dispose();await rejected;
});

test('registration views resolve their captured registrations with real scope state', async () => {
  const root=new Container();
  class Manual { static $inject=['sync'];constructor(readonly value:number){} }
  await root.load(defineModule({name:'resolve-in-module',async register(c){
    c.singleton('sync',()=>4);c.singleton('async',async()=>5);c.singletonMulti('many',()=>6);
    expect(c.resolveSync('sync')).toBe(4);expect(await c.resolve('async')).toBe(5);expect(await c.resolveAll('many')).toEqual([6]);
    expect(c.instantiate(Manual).value).toBe(4);expect(c.signal.aborted).toBe(false);expect(c.dependencyDrift()).toEqual([]);
  }}));await root.dispose();
});

test('observed-only dependency cycles are included only when requested', async () => {
  const root=new Container();root.singleton('observed-b',()=>1);root.singleton('observed-a',c=>c.resolveSync('observed-b'));root.resolveSync('observed-a');
  await root.replace('observed-b',c=>{c.resolveSync('observed-a');return 2;},0);root.validate();
  let issue=false;try{root.validate({includeObserved:true});}catch(error){if(!(error instanceof DiGraphError))throw error;issue=error.issues.some(item=>item.kind==='cycle');}expect(issue).toBe(true);await root.dispose();
});
test('graph diagnoses ambiguous unconditional candidates while registry duplicate rejection is atomic', async () => {
  const { DiGraph, ServiceLifetime, normalizeToken }=await import('@coldmind/inject');const token=createToken<number>('ambiguous');const key=normalizeToken(token);const graph=new DiGraph({onIssue(){}});
  for(let index=0;index<2;index++)graph.add({key,identity:Symbol(),token,lifetime:ServiceLifetime.Singleton,deps:[],declared:true});expect(graph.issues().some(item=>item.kind==='ambiguous')).toBe(true);
  const root=new Container();root.singleton(token,()=>1);expect(()=>root.singleton(token,()=>2)).toThrow(MutationConflictError);expect(root.resolveSync(token)).toBe(1);await root.dispose();
});

test('override discriminants do not reinterpret ordinary service properties', async () => {
  const root=new Container();root.singleton('number',()=>1);const factory=await override(root,'number',{kind:'factory',factory:()=>2});expect(await root.resolve('number')).toBe(2);await factory.restore();expect(await root.resolve('number')).toBe(1);
  root.singleton('object',()=>({value:1,factory:()=>1}));const value={value:2,factory:()=>2};const handle=await override(root,'object',value);expect(await root.resolve('object')).toBe(value);await handle.restore();await root.dispose();
});

test('snapshot cleanup cannot reinstall definitions after reentrant root disposal',async()=>{
  const root=new Container();root.singleton('original',()=>1);const snapshot=root.snapshot();root.singleton('dispose-during-restore',()=>({async onDispose(){await root.dispose();}}));root.resolveSync('dispose-during-restore');
  await expect(root.restore(snapshot)).rejects.toThrow(/disposed/);expect(root.isDisposed).toBe(true);expect(()=>root.resolveSync('original')).toThrow(/disposed/);await root.dispose();
});

test('a failed batch cannot hide another independently pending module from snapshot preflight',async()=>{
  const root=new Container();let release!:()=>void;const gate=new Promise<void>(resolve=>{release=resolve;});
  const failed=defineModule({name:'failed-batch',register(){throw new Error('batch failure');}});const pending=defineModule({name:'pending-batch',async register(c){await gate;c.singleton('batch-value',()=>1);}});
  await expect(root.load(failed,pending)).rejects.toThrow('batch failure');expect(()=>root.snapshot()).toThrow(MutationConflictError);release();await root.load(pending);expect(root.resolveSync('batch-value')).toBe(1);await root.dispose();
});
