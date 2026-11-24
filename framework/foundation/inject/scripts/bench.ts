/** Run: bun scripts/bench.ts [--budget-us=3] [--nested-ratio=4]; --budget-us=1e-9 proves failure. */
import assert from 'node:assert/strict';
import { Container, createToken } from '@coldmind/inject';

/** Reads a positive explicit budget, rejecting misspelled or invalid options. */
function budget(name: string): number | undefined {
  const option = process.argv.slice(2).find(value => value.startsWith(`--${name}=`));
  if (!option) return undefined;
  const value = Number(option.slice(option.indexOf('=') + 1));
  assert(Number.isFinite(value) && value > 0, `${name} must be positive and finite`);
  return value;
}
for (const option of process.argv.slice(2)) assert(/^--(?:budget-us|nested-ratio)=/.test(option), `Unknown option: ${option}`);
const cachedBudget = budget('budget-us');
const nestedBudget = budget('nested-ratio');
const root = new Container();
const hot = createToken<number>('hot');
root.singleton(hot, () => 1);
assert.equal(await root.resolve(hot), 1);
const iterations = 1_000_000;
const samples: number[] = [];
for (let sample = 0; sample < 6; sample++) {
  const started = performance.now();
  let sum = 0;
  for (let index = 0; index < iterations; index++) sum += root.get(hot);
  assert.equal(sum, iterations);
  if (sample) samples.push((performance.now() - started) * 1000 / iterations);
}
samples.sort((a, b) => a - b);
const cachedMicros = samples[2]!;
console.log(`cached get median: ${cachedMicros.toFixed(3)} us/op; warmup=1 samples=5 iterations=${iterations}; explicit budget=${cachedBudget ?? 'none'}`);
let failed = cachedBudget !== undefined && cachedMicros > cachedBudget;
const nested = createToken<number>('nested');
const outer = createToken<number>('outer');
root.transient(nested, () => 1);
root.transient(outer, async c => (await c.resolve(nested)) + 1);
const timings: number[] = [];
for (const liveScopes of [0, 1_000, 10_000]) {
  const scopes = Array.from({ length: liveScopes }, () => root.createScope());
  const runs: number[] = [];
  for (let sample = 0; sample < 6; sample++) {
    const started = performance.now();
    let sum = 0;
    for (let index = 0; index < 2_000; index++) sum += await root.resolve(outer);
    assert.equal(sum, 4_000);
    if (sample) runs.push(performance.now() - started);
  }
  runs.sort((a, b) => a - b);
  timings.push(runs[2]!);
  console.log(`2000 nested resolves, ${liveScopes} live scopes: median=${runs[2]!.toFixed(3)} ms; warmup=1 samples=5`);
  await Promise.all(scopes.map(scope => scope.dispose()));
}
const ratio = Math.max(...timings) / timings[0]!;
console.log(`nested max/baseline ratio=${ratio.toFixed(3)}; explicit budget=${nestedBudget ?? 'none'}; runtime=${process.versions.bun ? `Bun ${process.versions.bun}` : process.version}`);
failed ||= nestedBudget !== undefined && ratio > nestedBudget;
await root.dispose();
if (failed) {
  console.error('Explicit benchmark budget exceeded');
  process.exitCode = 1;
}
