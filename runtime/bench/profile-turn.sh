#!/bin/sh
# Samples lunar while one setImmediate chain runs (bench/turn.js), so the time
# of each loop turn can be attributed to Rust or JS frames. macOS only: uses `sample`.
#
# usage: bench/profile-turn.sh [sampling seconds, default 5]
# Needs a fresh build in _build/ (run.sh uses the same binary). Writes the call-graph
# report to .scratch/turn-sample.txt in the repo, which is git-ignored.
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
secs=${1:-5}
out="$repo/.scratch/turn-sample.txt"
mkdir -p "$repo/.scratch"
"$repo/_build/lunar" "$here/turn.js" &
pid=$!
# The chain starts at once; sample after one second so the loop is in steady state.
sleep 1
sample "$pid" "$secs" -file "$out" >/dev/null 2>&1
wait "$pid"
echo "wrote $out"
