import assert from 'node:assert/strict';
import { test } from 'node:test';

assert.match(import.meta.resolve('@coldmind/inject'), /\/dist\/.*\.js$/, 'Node smoke requires compiled inject exports');
assert.match(import.meta.resolve('@coldmind/inject/server'), /\/dist\/.*\.js$/, 'Node smoke requires compiled server exports');
const { Container, CircularDependencyError } = await import('@coldmind/inject');
const { DiRuntime, runInContext, currentContext } = await import('@coldmind/inject/server');

test('compiled runtime completes tenant/request roundtrip with once-per-tenant configuration', async t => {
  const root = new Container();
  const configured = []; const disposed = [];
  root.tenant('tenant', () => ({ onDispose() { disposed.push('tenant'); } }));
  root.request('request', async () => ({ onDispose() { disposed.push('request'); } }));
  const runtime = new DiRuntime(root, { configureTenant(id) { configured.push(id); } });
  t.after(() => runtime.dispose());
  assert.equal(runtime.state, 'created'); await runtime.start(); assert.equal(runtime.state, 'ready');
  const [first, second] = await Promise.all([runtime.acquireRequest('acme'), runtime.acquireRequest('acme')]);
  try {
    const one = await first.run(async scope => {
      assert.equal(currentContext(), scope);
      const tenant = await scope.resolve('tenant'); const request = await scope.resolve('request');
      assert.equal(currentContext(), scope); assert.equal(await scope.resolve('request'), request);
      return { tenant, request };
    });
    const two = await second.run(async scope => ({ tenant: await scope.resolve('tenant'), request: await scope.resolve('request') }));
    assert.equal(one.tenant, two.tenant); assert.notEqual(one.request, two.request);
    assert.deepEqual(configured, ['acme']); assert.equal(currentContext(), undefined);
  } finally { await first.release(); await first.release(); await second.release(); }
  assert.deepEqual(disposed, ['request', 'request']);
  await runtime.drainTenant('acme'); assert.deepEqual(disposed, ['request', 'request', 'tenant']);
  await runtime.dispose(); assert.equal(runtime.state, 'disposed'); assert.equal(root.isDisposed, true);
});

test('compiled runInContext isolates overlapping async branches and restores nested contexts', async t => {
  const root = new Container();
  t.after(() => root.dispose());
  const first = root.createScope(); const second = root.createScope();
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  let entered = 0;
  assert.equal(currentContext(), undefined);
  await Promise.all([first, second].map(scope => runInContext(scope, async () => {
    assert.equal(currentContext(), scope);
    if (++entered === 2) release();
    await gate; await Promise.resolve();
    assert.equal(currentContext(), scope);
    await runInContext(root, async () => { await Promise.resolve(); assert.equal(currentContext(), root); });
    assert.equal(currentContext(), scope);
  })));
  assert.equal(currentContext(), undefined);
  const failure = new Error('context failure');
  await assert.rejects(runInContext(first, async () => { await Promise.resolve(); throw failure; }), error => error === failure);
  assert.equal(currentContext(), undefined);
});

test('compiled server ALS detects a captured-container dependency cycle after await', { timeout: 2000 }, async t => {
  const root = new Container();
  const runtime = new DiRuntime(root);
  t.after(() => runtime.dispose());
  await runtime.start();
  root.singleton('first', async () => { await Promise.resolve(); return root.resolve('second'); });
  root.singleton('second', async () => { await Promise.resolve(); return root.resolve('first'); });
  await assert.rejects(runtime.withRequest('acme', scope => scope.resolve('first')), error => {
    assert.ok(error instanceof CircularDependencyError);
    assert.match(error.message, /first/); assert.match(error.message, /second/);
    return true;
  });
  assert.equal(currentContext(), undefined);
});
