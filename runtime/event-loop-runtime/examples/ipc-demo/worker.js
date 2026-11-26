// A worker: its own thread and VM. Sums what the parent sends, quits on request.
import { onMessage, postMessage, close } from 'lunarstorm:worker';

onMessage((m) => {
  if (m.quit) return close();
  postMessage({ count: m.numbers.length, sum: m.numbers.reduce((a, b) => a + b, 0) });
});
