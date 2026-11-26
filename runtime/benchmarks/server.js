// Benchmark servers. The same file runs on lunarstorm, node and bun.
//
// usage: <runtime> server.js <http|echo> <port>
//   http  answers every request with a 13-byte body and an explicit length
//   echo  writes back every chunk it receives
import http from 'node:http';
import net from 'node:net';

const [mode, port] = process.argv.slice(2);
const body = 'Hello, World!';
const server =
  mode === 'echo'
    ? net.createServer((sock) => {
        sock.on('data', (chunk) => sock.write(chunk));
        sock.on('error', () => {});
      })
    : http.createServer((req, res) => {
        res.writeHead(200, { 'content-type': 'text/plain', 'content-length': body.length });
        res.end(body);
      });
server.listen(Number(port), '127.0.0.1');
