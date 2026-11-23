#!/bin/sh
# Throughput benchmark: lunarstorm against node and bun on this machine.
#
# usage: bench/run.sh [seconds per measurement, default 10]
# Prints one block per runtime and appends the same text to bench/results.txt.
#
# Workloads (server is always one process; load comes from other processes):
#   http        oha, 64 keep-alive connections, one request at a time each
#   pipelined   4 node clients x 16 connections, 16 requests per batch
#   echo        4 node clients x 16 connections, 16 KiB round trips
#   loop        bench/loop.js, no I/O
here=$(cd "$(dirname "$0")" && pwd)
secs=${1:-10}
port=18080
out="$here/results.txt"

# Starts a server and waits until it accepts connections. Sets $pid.
serve() { "$1" "$here/server.js" "$2" $port & pid=$!; until nc -z 127.0.0.1 $port 2>/dev/null; do sleep 0.05; done; }
# Stops the server and frees the port.
stop() { kill $pid 2>/dev/null; wait $pid 2>/dev/null; port=$((port + 1)); }
# Runs four client processes and sums what they print.
clients() { for n in 1 2 3 4; do node "$here/client.js" "$1" $port 16 "$secs" & done | awk '{ s += $1 } END { print s }'; }

{
  echo "== $(date '+%Y-%m-%d %H:%M') $(sysctl -n machdep.cpu.brand_string), ${secs}s per measurement"
  for rt in "$here/../../_build/lunarstorm" node bun; do
    command -v "$rt" >/dev/null || continue
    echo "-- $(basename "$rt") $("$rt" --version 2>/dev/null | head -1)"
    serve "$rt" http
    oha -z "${secs}s" -c 64 --no-tui --output-format json "http://127.0.0.1:$port/" |
      node -e 'const r = JSON.parse(require("fs").readFileSync(0)); const ms = (s) => (s * 1000).toFixed(2); console.log("http           ", Math.round(r.summary.requestsPerSec), "req/s   p50", ms(r.latencyPercentiles.p50), "ms  p99", ms(r.latencyPercentiles.p99), "ms  ok", (r.summary.successRate * 100).toFixed(1) + "%")'
    echo "http pipelined  $(clients pipeline) req/s"
    echo "server rss      $(($(ps -o rss= -p $pid) / 1024)) MiB after the http runs"
    stop
    serve "$rt" echo
    echo "tcp echo        $(($(clients echo) / 1048576)) MiB/s"
    stop
    "$rt" "$here/loop.js" | awk '{ printf "loop %-24s %d ops/s\n", $1, $2 }'
  done
} | tee -a "$out"
