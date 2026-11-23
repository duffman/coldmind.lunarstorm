import { Container, createToken, defineModule, composeModules, lazy, provider, type RegistrySnapshot } from '@coldmind/inject';
import { override, createTestContainer } from '@coldmind/inject/testing';
import { DiRuntime } from '@coldmind/inject/server';
const N=createToken<number>('number'), S=createToken<string>('string');const root=new Container();
root.singleton(N,()=>1);root.singleton(S,{inject:[N,lazy(N),provider(N)] as const,factory:(n,l,p)=>{const value:Promise<number>=p();return n.toFixed()+l.getSync();}});
// @ts-expect-error A tuple factory cannot omit required parameters.
root.singleton(S,{inject:[N] as const,factory:()=>''});
// @ts-expect-error A tuple factory cannot substitute incompatible parameter types.
root.singleton(S,{inject:[N] as const,factory:(value:string)=>value});
// @ts-expect-error Registry snapshots cannot be forged structurally.
const forged:RegistrySnapshot={};
const db=defineModule<{db:number}>()({name:'db',register(c){c.singleton('db',()=>1);}});
const users=defineModule<{}, {db:number}>()({name:'users',register(){}});
composeModules(db,users);
// @ts-expect-error The required capability is missing.
composeModules(users);
const scope=await createTestContainer(db);const handle=await override(scope,N,{kind:'factory',factory:()=>2});await handle.restore();await scope.dispose();await new DiRuntime(root).dispose();
