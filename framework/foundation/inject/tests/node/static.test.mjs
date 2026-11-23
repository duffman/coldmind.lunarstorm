import assert from 'node:assert/strict';
import { test } from 'node:test';

assert.match(import.meta.resolve('@coldmind/inject'), /\/dist\/.*\.js$/, 'Node smoke requires compiled inject exports');
assert.match(import.meta.resolve('@coldmind/decorate'), /\/dist\/.*\.js$/, 'Node smoke requires compiled decorate exports');
const { Container, ServiceLifetime, Singleton, Inject, InjectOptional, InjectAll, describe } = await import('@coldmind/inject');

test('plain JavaScript static declarations inject explicit constructor and member dependencies', async t => {
  /** Decorator-free class with an explicit dependency graph. */
  class Service {
    static $injectable = { token: 'service', lifetime: ServiceLifetime.Singleton };
    static $inject = ['database'];
    static $members = { config: 'config', optional: { token: 'missing', optional: true }, items: { token: 'items', all: true } };
    /** Stores the explicitly declared constructor dependency. */
    constructor(database) { this.database = database; }
    /** Checks that member injection precedes initialization. */
    onInit() { assert.equal(this.config, 42); assert.deepEqual(this.items, [1, 2]); assert.equal(this.optional, undefined); }
  }
  const root = new Container();
  t.after(() => root.dispose());
  const database = {};
  root.registerInstance('database', database); root.registerInstance('config', 42);
  root.singletonMulti('items', () => 1); root.singletonMulti('items', () => 2);
  root.registerClass(Service); root.validate();
  const service = await root.resolve('service');
  assert.ok(service instanceof Service); assert.equal(service.database, database);
  assert.equal(await root.resolve('service'), service);
  assert.equal(describe(Service).params[0].token, 'database');
});

test('plain JavaScript invokes legacy decorator functions without decorator syntax or emitted metadata', async t => {
  /** Decorated via ordinary JavaScript function calls. */
  class Service {
    /** Stores the decorated constructor dependency. */
    constructor(database) { this.database = database; }
  }
  Inject('database')(Service, undefined, 0);
  Inject('config')(Service.prototype, 'config');
  InjectOptional('missing')(Service.prototype, 'optional');
  InjectAll('items')(Service.prototype, 'items');
  Singleton('decorated')(Service);
  const root = new Container();
  t.after(() => root.dispose());
  const database = {};
  root.registerInstance('database', database); root.registerInstance('config', 'ready');
  root.singletonMulti('items', () => 'first'); root.singletonMulti('items', () => 'second');
  root.registerClass(Service); root.validate();
  const service = await root.resolve('decorated');
  assert.ok(service instanceof Service); assert.equal(service.database, database);
  assert.equal(service.config, 'ready'); assert.equal(service.optional, undefined);
  assert.deepEqual(service.items, ['first', 'second']);
  assert.equal(await root.resolve('decorated'), service);
});
