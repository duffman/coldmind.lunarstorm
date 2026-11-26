// Load client for the two workloads no HTTP load tool covers: pipelined HTTP and
// raw TCP echo. Always run with node, several processes at once, so the client
// side has more cores than the single-threaded server under test.
//
// usage: node client.js <echo|pipeline> <port> <connections> <seconds>
// prints: bytes echoed per second (echo) or requests per second (pipeline)
//
// Decision: each connection sends one batch and waits for all of it to come back
// before sending the next, so the number measures full round trips. One second
// of warm-up is not counted.
import net from 'node:net';

const mode = process.argv[2];
const [port, connections, seconds] = process.argv.slice(3).map(Number);
/** Requests per pipelined batch. */
const DEPTH = 16;
const request = Buffer.from('GET / HTTP/1.1\r\nhost: bench\r\n\r\n');
let rounds = 0;
let counting = false;

for (let i = 0; i < connections; i++) {
  const sock = net.connect(port, '127.0.0.1');
  let payload = mode === 'echo' ? Buffer.alloc(16384, 97) : request;
  // Bytes one batch brings back; for HTTP it is learned from the first response.
  let expect = mode === 'echo' ? payload.length : 0;
  let got = 0;
  let head = '';
  sock.setNoDelay(true);
  sock.on('connect', () => sock.write(payload));
  sock.on('data', (chunk) => {
    if (expect === 0) {
      head += chunk.latin1Slice();
      const end = head.indexOf('\r\n\r\n');
      if (end < 0) return;
      const total = end + 4 + Number(/content-length: (\d+)/i.exec(head)[1]);
      if (head.length < total) return;
      expect = total * DEPTH;
      payload = Buffer.concat(Array(DEPTH).fill(request));
      sock.write(payload);
      return;
    }
    got += chunk.length;
    if (got >= expect) {
      got -= expect;
      if (counting) rounds++;
      sock.write(payload);
    }
  });
}
setTimeout(() => { counting = true; }, 1000);
setTimeout(() => {
  console.log(Math.round((rounds * (mode === 'echo' ? 16384 : DEPTH)) / seconds));
  process.exit(0);
}, (seconds + 1) * 1000);
