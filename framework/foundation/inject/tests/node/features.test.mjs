import assert from 'node:assert/strict';
import { test } from 'node:test';
assert.match(import.meta.resolve('@coldmind/inject'), /\/dist\/.*\.js$/);
const { Container, defineModule, createToken, lazy, provider, qualify, tokenName } = await import('@coldmind/inject');
const { override } = await import('@coldmind/inject/testing');

test('compiled modules, overrides and snapshots preserve ownership across fresh generations', async t => {
  const root=new Container();t.after(()=>root.dispose());const token=createToken('value');
  const module=defineModule({name:'compiled',register(c){c.singleton(token,()=>({value:1}));}});
  const [handle]=await root.load(module);assert.equal((await root.resolve(token)).value,1);
  const change=await override(root,token,{value:2});assert.equal((await root.resolve(token)).value,2);await change.restore();assert.equal((await root.resolve(token)).value,1);
  const snapshot=root.snapshot();root.singleton('extra',()=>1);await root.restore(snapshot);assert.equal(root.has('extra'),false);
  const [restoredHandle]=await root.load(module);await restoredHandle.unload();assert.equal(root.has(token),false);await handle.unload();
});
test('compiled typed factories, deferred providers and conditional candidates work without TypeScript', async t => {
  const root=new Container();t.after(()=>root.dispose());let count=0;const serial=createToken('serial'),label=createToken('label');root.transient(serial,()=>++count);
  root.singleton(label,()=> 'default');root.singleton(label,()=> 'selected',{when:ctx=>ctx.requester?.tokenName===tokenName('consumer')});
  root.singleton('consumer',{inject:[lazy(serial),provider(serial),label],async factory(access,next,name){assert.equal(access.peek(),undefined);return `${access.getSync()},${await next()}:${name}`;}});
  assert.equal(await root.resolve('consumer'),'1,2:selected');assert.equal(await root.resolve(label),'default');
});
test('compiled qualified tokens and explicit graph declarations retain identity', async t => {
  const root=new Container();t.after(()=>root.dispose());const db=createToken('Db'),replica=qualify(db,'replica');assert.equal(replica,qualify(db,'replica'));assert.equal(tokenName(replica),'Db#replica');root.singleton(replica,()=>7,{deps:[]});root.validate({requireDeps:true});assert.equal(root.resolveSync(replica),7);
});
