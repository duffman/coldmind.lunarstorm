#!/bin/sh
# Builds, launches and drives the Lunarstorm binaries. Run from anywhere in the repo.
#
#   driver.sh build                     debug build of lunar, lunarstorm-web, lunarstorm-debug
#   driver.sh cli                       lunar smoke: --version, -e, a script file using node:fs
#   driver.sh pipeline <project> [out]  lunar build -p <project> -o <out> (default .scratch/run-lunarstorm/dist)
#   driver.sh web <dir> [steps...]      serve a built dir (zebra.json) with lunarstorm-web, keep it
#                                       running, and run driver.mjs steps against it (open is implied)
#   driver.sh desktop <dir> [png]       open <dir> in lunarstorm-debug, wait for its window, capture it
#   driver.sh smoke                     all of the above on the example app, screenshots included
#   driver.sh stop                      stop what web/desktop started
#
# State (pids, url, logs, screenshots) lives in <repo>/.scratch/run-lunarstorm/.
set -eu

repo=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$repo"
state=.scratch/run-lunarstorm
mkdir -p "$state"
bin="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug"
here=.claude/skills/run-lunarstorm

# Prints the on-screen window ids of a process, one per line (CoreGraphics, layer 0).
window_ids() {
  swift - "$1" <<'EOF'
import CoreGraphics
let pid = Int(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
for w in list where (w[kCGWindowOwnerPID as String] as? Int) == pid && (w[kCGWindowLayer as String] as? Int) == 0 {
  print(w[kCGWindowNumber as String]!)
}
EOF
}

# Kills the process whose pid is in file $1, if any.
stop_pid() {
  [ -f "$1" ] && kill "$(cat "$1")" 2>/dev/null || true
  rm -f "$1"
}

cmd=${1:-}
[ $# -gt 0 ] && shift
case "$cmd" in
  build)
    cargo build --bin lunar --bin lunarstorm-web --bin lunarstorm-debug
    ;;
  cli)
    "$bin/lunar" --version
    "$bin/lunar" -e 'console.log("eval", 1 + 2)'
    printf 'import { readFileSync } from "node:fs";\nconsole.log("file", readFileSync("package.json", "utf8").length > 0);\n' > "$state/smoke.mjs"
    "$bin/lunar" run "$state/smoke.mjs"
    ;;
  pipeline)
    "$bin/lunar" build -p "$1" -o "${2:-$state/dist}"
    ;;
  web)
    dir=$1; shift
    stop_pid "$state/web.pid"
    "$bin/lunarstorm-web" "$dir" --port 0 > "$state/web.url" 2> "$state/web.log" &
    echo $! > "$state/web.pid"
    i=0; while [ ! -s "$state/web.url" ] && [ $i -lt 40 ]; do sleep 0.25; i=$((i + 1)); done
    [ -s "$state/web.url" ] || { cat "$state/web.log"; stop_pid "$state/web.pid"; exit 1; }
    url=$(cat "$state/web.url")
    echo "serving $dir at $url (pid $(cat "$state/web.pid"))"
    if [ $# -gt 0 ]; then node "$here/driver.mjs" open "$url/" "$@"; fi
    ;;
  desktop)
    dir=$1; png=${2:-$state/desktop.png}
    stop_pid "$state/desktop.pid"
    "$bin/lunarstorm-debug" "$dir" > "$state/desktop.log" 2>&1 &
    echo $! > "$state/desktop.pid"
    id=; i=0
    while [ -z "$id" ] && [ $i -lt 30 ]; do sleep 0.5; id=$(window_ids "$(cat "$state/desktop.pid")" | head -1); i=$((i + 1)); done
    [ -n "$id" ] || { echo "no window after 15 s"; cat "$state/desktop.log"; exit 1; }
    echo "window $id open (pid $(cat "$state/desktop.pid")); log: $state/desktop.log"
    if screencapture -x -o -l"$id" "$png" 2>/dev/null; then
      echo "shot $png"
    else
      echo "screencapture refused: grant Screen Recording to this terminal (System Settings > Privacy & Security)"
      exit 3
    fi
    ;;
  smoke)
    "$0" build
    "$0" cli
    "$0" pipeline templates/starters/desktop
    "$0" web platform/examples/app \
      wait .vcl-statusbar Ready shot "$state/web-before.png" \
      type input.vcl-edit ' 2' wait .vcl-statusbar Modified \
      click text=Save wait .vcl-statusbar Saved shot "$state/web-after.png"
    "$0" stop
    "$0" desktop platform/examples/app || echo "desktop: exit $?"
    "$0" stop
    ;;
  stop)
    stop_pid "$state/web.pid"
    stop_pid "$state/desktop.pid"
    echo stopped
    ;;
  *)
    sed -n '2,15p' "$0"
    exit 2
    ;;
esac
