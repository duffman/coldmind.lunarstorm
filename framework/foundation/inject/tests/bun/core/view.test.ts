import { expect, test } from 'vitest';
import { Container, Injectable, Inject, ServiceLifetime, createToken, type IContainer, type RegisterOptions, type BindingHandle, type Constructor, type ServiceFactory, type ServiceLifecycle } from '@coldmind/inject';

/** A concrete argument-bearing class used to check forwarded constructor registration overloads. */
class Constructed { constructor(readonly value: number) {} }
/** All registration methods share these two public overloads. */
interface Registration {
  <T, D = void>(token: import('@coldmind/inject').InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, options?: RegisterOptions): BindingHandle<T, D>;
  <T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
}

test('factory facade delegates the public registration overloads with the actual target receiver', async () => {
  const root = new Container(); let context!: IContainer;
  root.singleton('view-capture', factory => { context = factory; return 1; }); root.resolveSync('view-capture');
  expect(context).toBeInstanceOf(Container); expect(context).not.toBe(root); expect(context.identity).toBe(root.identity);
  const registrations: Registration[] = [context.singleton.bind(context), context.transient.bind(context), context.scoped.bind(context), context.tenant.bind(context), context.request.bind(context), context.singletonMulti.bind(context), context.transientMulti.bind(context), context.scopedMulti.bind(context), context.tenantMulti.bind(context), context.requestMulti.bind(context)];
  for (const [index, register] of registrations.entries()) {
    const token = createToken<number>(`view-factory-${index}`); const handle = register(token, () => index);
    expect(root.isRegistered(token)).toBe(true); expect(handle.token).toBe(token);
    class Local extends Constructed {}
    const constructor = register(Local, { deps: [] }); expect(root.isRegistered(Local)).toBe(true);
    await constructor.unregister(); await handle.unregister();
  }
  context.buildTimeout = 50; expect(root.buildTimeout).toBe(50); expect(context.buildTimeout).toBe(50);
  context.autoInject = true; expect(root.autoInject).toBe(true); context.autoInject = false;
  const value = {}; const supplied = context.registerInstance('view-instance', value); expect(root.get('view-instance')).toBe(value);
  expect(context.get('view-instance')).toBe(value); expect(context.tryGet('view-instance')).toBe(value);
  expect(context.has('view-instance')).toBe(true); expect(context.hasRecursive('view-instance')).toBe(true);
  expect(context.isRegistered('view-instance')).toBe(true); expect(context.isRegisteredRecursive('view-instance')).toBe(true);
  expect(context.getRegisteredTokens().length).toBe(2); expect(context.bindingOf('view-instance')?.id).toBe(supplied.bindingId);
  expect(context.instanceState('view-instance')).toBe('ready'); expect(context.multiBindingsOf('view-instance')).toEqual([]);
  expect(context.cachedBindings().length).toBe(2); expect(context.ownershipRecords().length).toBe(2);
  expect(context.dependencyEdges()).toEqual([]); expect(context.getDependencyGraph()).toEqual([]);
  expect(context.getDependencyGraphSnapshot()).toEqual(root.getDependencyGraphSnapshot()); expect(context.getDependencyTree('view-instance')).toEqual(root.getDependencyTree('view-instance'));
  expect(context.getResourceGraphSnapshot().scopes).toEqual(root.getResourceGraphSnapshot().scopes);
  expect(context.getScopeIdentity()).toEqual(root.getScopeIdentity()); expect(context.getScopeId()).toBe(root.getScopeId());
  expect(context.getScopeChain()).toEqual(root.getScopeChain()); expect(context.getRegistryEpoch()).toBe(root.getRegistryEpoch()); expect(context.epochSnapshot().epoch).toBe(root.epochSnapshot().epoch);
  expect(context.graph()).toBe(root.graph()); expect(context.issuesObserved()).toEqual([]); context.validate(); expect(context.discover('missing')).toEqual([]);
  const stop = context.onInstance('missing', () => {}); stop();
  const interceptor: import('@coldmind/inject').Interceptor = { onBeforeResolve() {} };
  context.addInterceptor(interceptor); expect(context.removeInterceptor(interceptor)).toBe(true);
  expect(await context.healthCheck('view-instance')).toEqual(await root.healthCheck('view-instance')); expect((await context.healthCheckAll()).size).toBe(2);
  for (const scope of [context.createScope(), context.createTenantScope(), context.createRequestScope()]) { expect(scope.getScopeChain().length).toBe(2); await scope.dispose(); }
  const replacement = await context.replace('view-instance', () => ({}), ServiceLifetime.Singleton); await context.replaceBinding(replacement.bindingId, () => ({}));
  await context.unregister('view-instance'); expect(await context.unregisterBinding('missing')).toBe(false);
  class Declared { static $injectable = {}; }
  const declared = context.registerClass(Declared); await declared.unregister();
  if (context instanceof Container) await context.rollbackNewInstances(new Set(context.ownershipRecords().map(record => record.bindingId)));
  await context.initializeOwned(); await context.clearInstances();
  if (context instanceof Container) expect(await context.disposeWithTracking()).toEqual([]);
  await context.dispose(); expect(context.isDisposed).toBe(true);
});

/** Member completion through the two facade kinds must retain the same injection rules. */
const Word = createToken<string>('view-word');
@Injectable()
class MemberTarget { @Inject(Word) word!: string; }

test('factory facade implements synchronous and asynchronous construction/injection without losing context', async () => {
  const root = new Container(); root.registerInstance(Word, 'injected');
  root.singleton('view-construct-sync', context => {
    const constructed = context.instantiate(MemberTarget); const supplied = context.injectMembers(new MemberTarget());
    return [constructed.word, supplied.word];
  });
  root.singleton('view-construct-async', async context => {
    const constructed = await context.instantiateAsync(MemberTarget); const supplied = await context.injectMembersAsync(new MemberTarget());
    return [constructed.word, supplied.word];
  });
  expect(root.resolveSync('view-construct-sync')).toEqual(['injected', 'injected']);
  expect(await root.resolve('view-construct-async')).toEqual(['injected', 'injected']); await root.dispose();
});

test('authored static injection still rejects a nonwritable member instead of silently skipping it', async () => {
  class FrozenMember { static $members = { word: Word }; }
  const root = new Container(); root.registerInstance(Word, 'injected'); const supplied = Object.freeze(new FrozenMember());
  expect(() => root.injectMembers(supplied)).toThrow(TypeError); await root.dispose();
});
