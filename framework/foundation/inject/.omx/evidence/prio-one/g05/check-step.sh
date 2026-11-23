#!/usr/bin/env bash
set -eu
step="$1"
evidence=.omx/evidence/prio-one/g05
run() {
  name="$1"; shift
  set +e
  "$@" > "$evidence/$name.log" 2>&1
  result=$?
  set -e
  printf '%s\n' "$result" > "$evidence/$name.exit"
  printf '%s %s\n' "$name" "$result"
  if [ "$result" -ne 0 ]; then cat "$evidence/$name.log"; return "$result"; fi
}
for label in node bun; do
  if [ "$label" = node ]; then runtime=/Users/patrik/.nvm/versions/node/v24.14.1/bin/node; else runtime=.cache/bun/bun-darwin-aarch64/bun; fi
  run "step$step-types-$label" env CMINJECT_EXPECT_RUNTIME="$label" "$runtime" node_modules/vitest/vitest.mjs run --typecheck.only --config vitest.types.config.ts
  if [ "$label" = bun ]; then
    run "step$step-react-$label" env CMINJECT_EXPECT_RUNTIME=bun INJECT_EXPECT_BUN=1 "$runtime" node_modules/vitest/vitest.mjs run --config vitest.react.config.ts
  else
    run "step$step-react-$label" env CMINJECT_EXPECT_RUNTIME=node "$runtime" node_modules/vitest/vitest.mjs run --config vitest.react.config.ts
  fi
  run "step$step-source-$label" "$runtime" node_modules/typescript/bin/tsc --noEmit -p tsconfig.json
done
run "step$step-browser" .cache/bun/bun-darwin-aarch64/bun scripts/check-browser.ts
run "step$step-imports" .cache/bun/bun-darwin-aarch64/bun scripts/check-core-imports.ts
