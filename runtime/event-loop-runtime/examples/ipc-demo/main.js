// The program under demo: talks to the host over `lunarstorm:ipc` and farms
// summing out to a `lunarstorm:worker`.
import { send, onMessage } from 'lunarstorm:ipc';
import { spawn } from 'lunarstorm:worker';

const text = new TextDecoder();
const bytes = new TextEncoder();
const reply = (line) => send(bytes.encode(line));

const worker = spawn('worker.js');
let pending = null;
worker.onMessage((m) => {
  reply(`worker: sum of ${m.count} numbers = ${m.sum}`);
  pending?.();
});

onMessage((message) => {
  const line = text.decode(message);
  if (line === 'quit') {
    worker.postMessage({ quit: true });
    worker.exited.then(({ state }) => {
      reply(`worker: ${state}`);
      onMessage(null); // releases the channel: the program ends
    });
  } else if (line.startsWith('sum ')) {
    worker.postMessage({ numbers: line.slice(4).split(/\s+/).map(Number) });
  } else {
    reply(`echo: ${line.toUpperCase()}`);
  }
});
reply('program: ready');
