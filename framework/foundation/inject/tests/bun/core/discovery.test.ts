import { expect, test } from 'vitest';
import { Container, defineDecorator, Injectable, ServiceLifetime } from '@coldmind/inject';

/** Method metadata shared by discovery characterizations. */
const Observe = defineDecorator<[string]>({ name: '@DiscoveryObserve', on: ['method'], apply(target, label) { target.method({ label }); } });
/** Wrapper proving discovered method access follows the completed instance. */
const Wrap = defineDecorator<[]>({ name: '@DiscoveryWrap', on: ['method'], apply(target) { target.wrap(original => (...args) => `wrapped:${original(...args)}`); } });

/** Tagged class with a discoverable method and generated instance wrapper. */
@Injectable({ lifetime: ServiceLifetime.Scoped, tags: ['discovery-tag'] })
class Observed {
  /** Value consumed through the bound method receiver. */
  value = 'value';
  /** Discoverable wrapped operation. */
  @Observe('operation') @Wrap() run() { return this.value; }
}

/** A second tagged class for scope-order assertions. */
@Injectable({ lifetime: ServiceLifetime.Scoped, tags: ['discovery-tag'] })
class LocalObserved {
  /** Discoverable child operation. */
  @Observe('local') run() { return 'local'; }
}

test('discovery matches tags and method keys without constructing instances', async () => {
  const root = new Container(); const handle = root.registerClass(Observed);
  const methods = root.discover('@DiscoveryObserve'); const tags = root.discover('discovery-tag');
  expect(methods).toHaveLength(1); expect(methods[0]!.ctor).toBe(Observed); expect(methods[0]!.binding.id).toBe(handle.bindingId);
  expect(methods[0]!.methods).toEqual([expect.objectContaining({ name: 'run', decorator: '@DiscoveryObserve', options: { label: 'operation' } })]);
  expect(tags[0]!.methods).toEqual([]); expect(tags[0]!.tags).toEqual(['discovery-tag']);
  expect(root.discover('discovery-absent')).toEqual([]); expect(root.cachedBindings()).toEqual([]); await root.dispose();
});

test('discovery inherits ancestor registrations in root-first order without exposing child classes upward', async () => {
  const root = new Container(); const child = root.createScope(); root.registerClass(Observed); child.registerClass(LocalObserved);
  expect(child.discover('discovery-tag').map(item => item.ctor)).toEqual([Observed, LocalObserved]);
  expect(root.discover('discovery-tag').map(item => item.ctor)).toEqual([Observed]); await root.dispose();
});

test('onInstance provides matching metadata and completed generated methods without duplicate cache-hit notifications', async () => {
  const root = new Container(); const child = root.createScope(); root.registerClass(Observed); const calls: object[] = [];
  child.onInstance('@DiscoveryObserve', (instance, context) => {
    calls.push(instance); expect(context.container.getScopeId()).toBe(child.getScopeId());
    expect(context.methods[0]!.options).toEqual({ label: 'operation' });
    expect(context.methods[0]!.get(instance)()).toBe('wrapped:value');
    expect(context.descriptor.tags).toContain('discovery-tag');
  });
  const instance = await child.resolve(Observed); expect(await child.resolve(Observed)).toBe(instance);
  expect(child.resolveSync(Observed)).toBe(instance); expect(calls).toEqual([instance]); await root.dispose();
});

test('onInstance inherits root hooks before child hooks and unsubscribe affects only future instances', async () => {
  const root = new Container(); const child = root.createScope(); const sibling = root.createScope(); const events: string[] = [];
  root.registerClass(Observed); root.onInstance('discovery-tag', (_instance, context) => { expect(context.methods).toEqual([]); events.push('root'); });
  const unsubscribe = child.onInstance('discovery-tag', () => { events.push('child'); });
  await child.resolve(Observed); expect(events).toEqual(['root', 'child']);
  unsubscribe(); unsubscribe(); await sibling.resolve(Observed); expect(events).toEqual(['root', 'child', 'root']);
  await child.clearInstances(); await child.resolve(Observed); expect(events).toEqual(['root', 'child', 'root', 'root']); await root.dispose();
});

test('asynchronous onInstance hooks finish before callers receive their instance', async () => {
  const root = new Container(); const child = root.createScope(); root.registerClass(Observed); const events: string[] = [];
  root.onInstance('@DiscoveryObserve', async () => { events.push('before'); await Promise.resolve(); events.push('after'); });
  const instance = await child.resolve(Observed); expect(instance.run()).toBe('wrapped:value'); expect(events).toEqual(['before', 'after']); await root.dispose();
});

test('discovered method access is bound to each instance and metadata snapshots cannot mutate class tags', async () => {
  const root = new Container(); const first = root.createScope(); const second = root.createScope(); root.registerClass(Observed);
  const a = await first.resolve(Observed); const b = await second.resolve(Observed); a.value = 'a'; b.value = 'b';
  const method = root.discover('@DiscoveryObserve')[0]!.methods[0]!;
  expect(method.get(a)()).toBe('wrapped:a'); expect(method.get(b)()).toBe('wrapped:b');
  const snapshot = root.discover('discovery-tag'); snapshot[0]!.tags.push('mutation');
  expect(root.discover('mutation')).toEqual([]); expect(root.discover('discovery-tag')[0]!.tags).toEqual(['discovery-tag']); await root.dispose();
});
