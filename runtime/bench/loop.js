// Event-loop microbenchmarks: how fast each scheduling primitive turns over.
// The same file runs on lunarstorm, node and bun. Prints `<name> <ops per second>`.
//
// These numbers are dominated by the JS engine and the loop, with no I/O.
const N = 1_000_000;
/**
 * @param {number} count operations done
 * @param {number} since `performance.now()` at the start
 * @return {number} operations per second
 */
const rate = (count, since) => Math.round(count / ((performance.now() - since) / 1000));
/**
 * Runs `count` callbacks one after another through a scheduler.
 * @param {function(Function): void} schedule queues one callback
 * @param {number} count how many to run
 * @return {Promise<void>} resolves after the last one
 */
const chain = (schedule, count) =>
  new Promise((done) => {
    let i = 0;
    const next = () => (++i < count ? schedule(next) : done());
    next();
  });

let t = performance.now();
for (let i = 0; i < N; i++) await null;
console.log('promise-jobs', rate(N, t));

t = performance.now();
await chain(process.nextTick, N);
console.log('nextTick', rate(N, t));

t = performance.now();
await chain(setImmediate, N / 4);
console.log('setImmediate-loop-turns', rate(N / 4, t));

t = performance.now();
await new Promise((done) => {
  let left = N / 4;
  for (let i = 0; i < N / 4; i++) setTimeout(() => --left === 0 && done(), 0);
});
console.log('timers', rate(N / 4, t));
