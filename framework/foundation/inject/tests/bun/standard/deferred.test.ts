import { expect, test } from 'vitest';
import { Container, createToken, Inject, lazy, provider, type Lazy, type Provider } from '@coldmind/inject';
test('standard fields and static members accept deferred declarations', async () => {
  const Value=createToken<number>('value');class Standard { @Inject(lazy(Value)) value!: Lazy<number>; @Inject(provider(Value)) next!: Provider<number>; }
  class Static { static $members={value:lazy(Value)};value!:Lazy<number>; }
  const root=new Container();let calls=0;root.transient(Value,()=>++calls);root.scoped(Standard);root.scoped(Static);const scope=root.createScope();const standard=scope.resolveSync(Standard);expect(calls).toBe(0);expect(await standard.next()).toBe(1);expect(standard.value.getSync()).toBe(2);expect(scope.resolveSync(Static).value.getSync()).toBe(3);await root.dispose();
});
test('legacy invocation preserves marker identity rather than unwrapping it eagerly',async()=>{
  const Value=createToken<number>('legacy-value');class Legacy { value!:Lazy<number>; }
  Inject(lazy(Value))(Legacy.prototype,'value');const root=new Container();root.singleton(Value,()=>7);root.scoped(Legacy);const service=root.resolveSync(Legacy);expect(service.value.peek()).toBeUndefined();expect(service.value.getSync()).toBe(7);await root.dispose();
});
