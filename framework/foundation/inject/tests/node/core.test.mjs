import assert from 'node:assert/strict';
import { test } from 'node:test';

assert.match(import.meta.resolve('@coldmind/inject'), /\/dist\/.*\.js$/, 'Node smoke requires compiled inject exports');
const { Container, LifecycleError } = await import('@coldmind/inject');

test('compiled core keeps synchronous singleton, scoped and transient ownership', async t => {
  const root = new Container();
  t.after(() => root.dispose());
  let singletonBuilds = 0;
  root.singleton('singleton', () => ({ build: ++singletonBuilds }));
  root.scoped('scoped', () => ({}));
  root.transient('transient', () => ({}));
  const first = root.createScope(); const second = root.createScope();
  const singleton = first.resolveSync('singleton');
  assert.equal(root.get('singleton'), singleton);
  assert.equal(second.resolveSync('singleton'), singleton);
  assert.equal(singletonBuilds, 1);
  assert.equal(first.resolveSync('scoped'), first.resolveSync('scoped'));
  assert.notEqual(first.resolveSync('scoped'), second.resolveSync('scoped'));
  assert.notEqual(first.resolveSync('transient'), first.resolveSync('transient'));
});

test('compiled core joins async singleton construction and isolates async scoped instances', async t => {
  const root = new Container();
  t.after(() => root.dispose());
  let builds = 0;
  root.singleton('async', async () => { builds++; await Promise.resolve(); return {}; });
  root.scoped('scoped', async () => { await Promise.resolve(); return {}; });
  root.transient('transient', async () => { await Promise.resolve(); return {}; });
  const first = root.createScope(); const second = root.createScope();
  const [one, two] = await Promise.all([first.resolve('async'), second.resolve('async')]);
  assert.equal(one, two); assert.equal(builds, 1);
  assert.equal(await first.resolve('scoped'), await first.resolve('scoped'));
  assert.notEqual(await first.resolve('scoped'), await second.resolve('scoped'));
  const transients = await Promise.all([first.resolve('transient'), first.resolve('transient')]);
  assert.notEqual(transients[0], transients[1]);
});

test('compiled multi resolution preserves parent-first registration order and cached identities', async t => {
  const root = new Container();
  t.after(() => root.dispose());
  const order = [];
  root.singletonMulti('items', async () => { order.push('parent-async'); return { id: 1 }; });
  root.singletonMulti('items', () => { order.push('parent-sync'); return { id: 2 }; });
  const child = root.createScope();
  child.singletonMulti('items', () => { order.push('child'); return { id: 3 }; });
  const items = await child.resolveAll('items');
  assert.deepEqual(items.map(item => item.id), [1, 2, 3]);
  assert.deepEqual(order, ['parent-async', 'parent-sync', 'child']);
  const again = await child.resolveAll('items');
  again.forEach((item, index) => assert.equal(item, items[index]));
  assert.deepEqual(await child.resolveAll('missing'), []);
});

test('compiled disposal orders child before parent and preserves external ownership', async t => {
  const root = new Container();
  t.after(() => root.dispose());
  const disposed = [];
  root.singleton('parent', () => ({ onDispose() { disposed.push('parent'); } }));
  root.registerInstance('external', { onDispose() { disposed.push('external'); } }, { ownership: 'external' });
  const child = root.createScope();
  child.singleton('child', () => ({ async onDispose() { await Promise.resolve(); disposed.push('child'); } }));
  root.resolveSync('parent'); child.resolveSync('child');
  await root.dispose(); await root.dispose();
  assert.deepEqual(disposed, ['child', 'parent']);
  assert.equal(root.isDisposed, true); assert.equal(child.isDisposed, true);
  assert.throws(() => child.resolveSync('child'), LifecycleError);
});

test('compiled disposal cancels a pending factory through its build signal', { timeout: 2000 }, async t => {
  const root = new Container();
  t.after(() => root.dispose());
  let started;
  const entered = new Promise(resolve => { started = resolve; });
  let signal; let aborts = 0;
  root.singleton('pending', context => {
    signal = context.signal;
    started();
    return new Promise(resolve => signal.addEventListener('abort', () => { aborts++; resolve({}); }, { once: true }));
  });
  const rejected = assert.rejects(root.resolve('pending'), error => error.code === 'DI_BUILD_CANCELLED');
  await entered;
  await root.dispose(); await rejected;
  assert.equal(signal.aborted, true); assert.equal(aborts, 1);
});
