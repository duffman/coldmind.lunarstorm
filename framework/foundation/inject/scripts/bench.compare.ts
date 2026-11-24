/** Informational comparison: bun scripts/bench.compare.ts; writes measured bench/results.json. */
import 'reflect-metadata';
import assert from 'node:assert/strict';
import { cpus, platform, arch, release, totalmem } from 'node:os';
import { createRequire } from 'node:module';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { Container } from '@coldmind/inject';
import { Container as Inversify } from 'inversify';
import { container, Lifecycle, instanceCachingFactory, type DependencyContainer } from 'tsyringe';
import { createContainer, asFunction, type AwilixContainer } from 'awilix';

/** Shared semantic payload. */
interface Value { value: number }
/** Synchronous graph factory used by every implementation. */
type Factory = (get: (name: string) => Value) => Value;
/** Minimal adapter; absence means genuinely unsupported native semantics. */
interface Adapter {
  bind(name: string, factory: Factory): void;
  bindAsync?(name: string, factory: (get: (name: string) => Promise<Value>) => Promise<Value>): void;
  scoped?(name: string, create: () => Value, dispose: () => void): void;
  get(name: string): Value;
  cachedGet?(name: string): Value;
  getAsync?(name: string): Promise<Value>;
  scope(): Adapter;
  dispose(): Promise<void>;
}
/** Wraps cminject without moving container work into the harness. */
function injectAdapter(c = new Container()): Adapter {
  return {
    bind: (name, factory) => { c.singleton(name, r => factory(key => r.resolveSync<Value>(key))); },
    bindAsync: (name, factory) => { c.singleton(name, r => factory(key => r.resolve<Value>(key))); },
    scoped: (name, create, dispose) => { c.request(name, { factory: create, onDispose: dispose }); },
    get: name => c.resolveSync<Value>(name), cachedGet: name => c.get<Value>(name), getAsync: name => c.resolve<Value>(name),
    scope: () => injectAdapter(c.createRequestScope()), dispose: () => c.dispose(),
  };
}
/** Inversify request lifetime is one resolution graph, not an explicit multi-get request scope. */
function inversifyAdapter(c = new Inversify()): Adapter {
  return {
    bind: (name, factory) => { c.bind<Value>(name).toDynamicValue(r => factory(key => r.get<Value>(key))).inSingletonScope(); },
    bindAsync: (name, factory) => { c.bind<Value>(name).toDynamicValue(r => factory(key => r.getAsync<Value>(key))).inSingletonScope(); },
    get: name => c.get<Value>(name), getAsync: name => c.getAsync<Value>(name),
    scope: () => inversifyAdapter(new Inversify({ parent: c })), dispose: async () => { await c.unbindAll(); },
  };
}
/** tsyringe tracks class disposal; factory caching is used only for synchronous graph scenarios. */
function tsyringeAdapter(c: DependencyContainer = container.createChildContainer()): Adapter {
  return {
    bind: (name, factory) => { c.register(name, { useFactory: instanceCachingFactory(r => factory(key => r.resolve<Value>(key))) }); },
    scoped: (name, create, dispose) => {
      /** Native ContainerScoped class with deterministic disposal. */
      class Service {
        /** Builds the identical scalar service payload. */
        value = create().value;
        /** Records native container disposal. */
        dispose(): void { dispose(); }
      }
      c.register(name, { useClass: Service }, { lifecycle: Lifecycle.ContainerScoped });
    },
    get: name => c.resolve<Value>(name), scope: () => tsyringeAdapter(c.createChildContainer()),
    dispose: async () => { await c.dispose(); },
  };
}
/** Awilix cradle performs native dependency resolution; async graph semantics are not emulated. */
function awilixAdapter(c: AwilixContainer = createContainer()): Adapter {
  return {
    bind: (name, factory) => { c.register(name, asFunction((cradle: Record<string, Value>) => factory(key => cradle[key]!)).singleton()); },
    scoped: (name, create, dispose) => { c.register(name, asFunction(create).scoped().disposer(dispose)); },
    get: name => c.resolve<Value>(name), scope: () => awilixAdapter(c.createScope()), dispose: () => c.dispose(),
  };
}
/** Retrieves installed versions even when a package hides package.json behind exports. */
function version(name: string): string {
  const require = createRequire(import.meta.url);
  let directory = dirname(require.resolve(name));
  for (;;) {
    try {
      const pkg = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
      if (pkg.name === name) return pkg.version;
    } catch { /* Continue to the package root. */ }
    const parent = dirname(directory);
    assert.notEqual(parent, directory, `Version unavailable: ${name}`);
    directory = parent;
  }
}
/** Executes an operation with equivalent work assertions before and during measurement. */
type Run = () => Promise<number>;
/** Scenario setup returns timing-only work and cleanup outside the timed interval. */
interface Work { run: Run; close(): Promise<void> }
/** Cached singleton resolution includes a checksum and one construction. */
async function cached(make: () => Adapter): Promise<Work> {
  const c = make();
  let builds = 0;
  c.bind('hot', () => { builds++; return { value: 1 }; });
  const first = c.get('hot');
  const get = c.cachedGet ?? c.get;
  assert.equal(get('hot'), first);
  return { run: async () => {
    const start = performance.now();
    let sum = 0;
    for (let i = 0; i < 1_000_000; i++) sum += get('hot').value;
    const elapsed = performance.now() - start;
    assert.equal(sum, 1_000_000); assert.equal(builds, 1);
    return elapsed;
  }, close: () => c.dispose() };
}
/** A cold chain registers and resolves exactly 100 nodes; setup and teardown are excluded. */
async function cold(make: () => Adapter, depth: number, asynchronous: boolean): Promise<Work> {
  return { run: async () => {
    const c = make();
    let builds = 0;
    for (let i = 0; i < depth; i++) {
      const previous = `node${i - 1}`;
      if (asynchronous) c.bindAsync!(`node${i}`, async get => {
        await Promise.resolve(); builds++;
        return { value: i ? (await get(previous)).value + 1 : 1 };
      });
      else c.bind(`node${i}`, get => { builds++; return { value: i ? get(previous).value + 1 : 1 }; });
    }
    const start = performance.now();
    const value = asynchronous ? await c.getAsync!(`node${depth - 1}`) : c.get(`node${depth - 1}`);
    const elapsed = performance.now() - start;
    assert.equal(value.value, depth); assert.equal(builds, depth);
    await c.dispose();
    return elapsed;
  }, close: async () => {} };
}
/** Empty scope churn or five native scope-cached disposable services per request. */
async function scopes(make: () => Adapter, requests: boolean): Promise<Work> {
  const root = make();
  let builds = 0;
  let disposed = 0;
  if (requests) for (let i = 0; i < 5; i++) root.scoped!(`service${i}`, () => { builds++; return { value: i + 1 }; }, () => { disposed++; });
  return { run: async () => {
    builds = 0; disposed = 0;
    let sum = 0;
    const count = requests ? 1_000 : 10_000;
    const start = performance.now();
    for (let i = 0; i < count; i++) {
      const scope = root.scope();
      if (requests) for (let j = 0; j < 5; j++) {
        const first = scope.get(`service${j}`);
        assert.equal(scope.get(`service${j}`), first);
        sum += first.value;
      }
      await scope.dispose();
    }
    const elapsed = performance.now() - start;
    assert.equal(sum, requests ? 15_000 : 0);
    assert.equal(builds, requests ? 5_000 : 0);
    assert.equal(disposed, requests ? 5_000 : 0);
    return elapsed;
  }, close: () => root.dispose() };
}
const warmup = 2;
const samples = 7;
const metadata = {
  recordedAt: new Date().toISOString(), runtime: process.versions.bun ? `Bun ${process.versions.bun}` : `Node ${process.version}`,
  nodeCompatibilityVersion: process.versions.node, hardware: { platform: platform(), arch: arch(), release: release(), cpu: cpus()[0]?.model, logicalCpus: cpus().length, memoryBytes: totalmem() },
  warmup, samples, semanticPreflightRuns: 1, statistic: 'median milliseconds per scenario batch', informational: true,
  timing: 'setup/registration and root teardown excluded; scope creation/disposal included; sequential, no forced GC',
  semantics: 'cached=1M gets/one build; cold=100-node chain/100 builds; async=10 awaited nodes/10 builds; scopes=10k empty children; requests=1k children/5 services/5k builds+disposals, two gets per service',
  sources: ['https://inversify.io/docs/api/container/', 'https://github.com/microsoft/tsyringe#readme', 'https://github.com/jeffijoe/awilix#readme'],
};
const libraries = [
  { name: '@coldmind/inject', make: injectAdapter }, { name: 'inversify', make: inversifyAdapter },
  { name: 'tsyringe', make: tsyringeAdapter }, { name: 'awilix', make: awilixAdapter },
];
const scenarios = ['cached singleton get', 'cold 100-node graph', 'async depth 10', '10k scope create/dispose', '1000 requests x 5 services'] as const;
const results: { library: string; version: string; scenario: string; status: string; reason?: string; medianMs?: number; samplesMs?: number[] }[] = [];
for (const library of libraries) {
  const probe = library.make();
  const asyncSupported = !!probe.bindAsync;
  const requestSupported = !!probe.scoped;
  await probe.dispose();
  for (const [index, scenario] of scenarios.entries()) {
    const base = { library: library.name, version: version(library.name), scenario };
    if ((index === 2 && !asyncSupported) || (index === 4 && !requestSupported)) {
      results.push({ ...base, status: 'N/A', reason: index === 2 ? 'No native async factory graph resolution; promise-valued services are not equivalent.' : 'Request lifetime is per resolution graph, not a disposable multi-get request scope.' });
      continue;
    }
    const work = index === 0 ? await cached(library.make) : index === 1 ? await cold(library.make, 100, false) : index === 2 ? await cold(library.make, 10, true) : await scopes(library.make, index === 4);
    try {
      await work.run();
      for (let i = 0; i < warmup; i++) await work.run();
      const values: number[] = [];
      for (let i = 0; i < samples; i++) values.push(await work.run());
      const sorted = [...values].sort((a, b) => a - b);
      results.push({ ...base, status: 'supported', medianMs: sorted[Math.floor(samples / 2)], samplesMs: values });
    } finally { await work.close(); }
  }
}
console.log(JSON.stringify(metadata, null, 2));
console.log('| Library/version | Scenario | Median ms/batch | Status |');
console.log('|---|---|---:|---|');
for (const row of results) console.log(`| ${row.library}@${row.version} | ${row.scenario} | ${row.medianMs?.toFixed(3) ?? '—'} | ${row.reason ?? row.status} |`);
const output = new URL('../bench/results.json', import.meta.url);
mkdirSync(dirname(output.pathname), { recursive: true });
let previousRuns: { metadata: unknown; results: unknown }[] = [];
try {
  const previous = JSON.parse(readFileSync(output, 'utf8'));
  assert(previous.metadata && Array.isArray(previous.results), 'Invalid previous benchmark results');
  previousRuns = [...(previous.previousRuns ?? []), { metadata: previous.metadata, results: previous.results }];
} catch (error) {
  if (!(error instanceof Error && 'code' in error && error.code === 'ENOENT')) throw error;
}
writeFileSync(output, `${JSON.stringify({ metadata, results, previousRuns }, null, 2)}\n`);
