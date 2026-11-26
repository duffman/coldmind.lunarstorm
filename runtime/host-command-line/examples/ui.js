// Demo of the terminal UI module. Run it in a terminal:
//
//     lunarstorm examples/ui.js

import { style, progress, spinner } from 'lunarstorm:ui';

/**
 * @param {number} ms how long to wait
 * @return {Promise<void>} resolves after the delay
 */
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

console.log('\n  ' + style.gradient('☾ lunarstorm ui') + style.dim('  colors, progress bars, spinners') + '\n');

const resolving = spinner('Resolving dependencies');
await sleep(900);
resolving.succeed('Resolved 42 dependencies');

// Three bars at once; console.log lines scroll above them.
const downloads = ['runtime.tar.gz', 'stdlib.tar.gz ', 'docs.tar.gz   '].map((label, i) => {
  const total = (i + 2) * 4_000_000;
  return { total, speed: 90_000 + i * 60_000, bar: progress({ label, total, bytes: true }) };
});
for (let turn = 1; downloads.some(({ bar, total }) => bar.value < total); turn++) {
  for (const { bar, speed } of downloads) bar.tick(speed * (0.5 + Math.random()));
  if (turn % 25 === 0) console.log(style.dim('  log:'), 'checkpoint', turn / 25, { ok: true });
  await sleep(30);
}

const building = spinner('Linking');
await sleep(700);
building.text('Optimizing');
await sleep(700);
building.succeed('Built in release mode');

console.log('\n  ' + style.green('✔') + ' ' + style.bold('All done') + '\n');
