#!/bin/bash
set -eu
ROOT="$(pwd)"
BUN="$ROOT/.cache/bun/bun-darwin-aarch64/bun"
NODE=/Users/patrik/.nvm/versions/node/v24.14.1/bin/node
MIN="$ROOT/.cache/node22-min/node-v22.0.0-darwin-arm64/bin/node"
TSC="$ROOT/node_modules/typescript/bin/tsc"
VITEST="$ROOT/node_modules/vitest/vitest.mjs"
E="$ROOT/.omx/evidence/prio-one/solo-final"
export PATH="$(dirname "$BUN"):$(dirname "$NODE"):$PATH"
run() { local name="$1"; shift; set +e; "$@" > "$E/$name.log" 2>&1; local status=$?; set -e; echo "$status" > "$E/$name.exit"; echo "$name: $status"; if [ "$status" -ne 0 ]; then tail -55 "$E/$name.log"; return "$status"; fi; }
run types-source "$BUN" "$TSC" --noEmit -p tsconfig.json
run types-delivery "$BUN" "$TSC" --noEmit -p tsconfig.delivery.json
run types-fixtures "$BUN" "$TSC" --noEmit -p tsconfig.types.json
# Generated output only; the two packages build without pre-existing declarations.
rm -rf "$ROOT/dist" "$ROOT/../decorate/dist"
run clean-decorate "$BUN" "$TSC" -p ../decorate/tsconfig.build.json
run clean-inject "$BUN" "$TSC" -p tsconfig.build.json
run types-dist-consumer "$NODE" "$TSC" -p "$E/consumer/tsconfig.json"
run node22-dist "$MIN" --test tests/node/core.test.mjs tests/node/static.test.mjs tests/node/server.test.mjs tests/node/features.test.mjs
run node24-dist "$NODE" --test tests/node/core.test.mjs tests/node/static.test.mjs tests/node/server.test.mjs tests/node/features.test.mjs
run bun env CMINJECT_EXPECT_RUNTIME=bun "$BUN" "$VITEST" run --config vitest.config.ts --reporter=default --reporter=json --outputFile="$E/bun-results.json"
run node env CMINJECT_EXPECT_RUNTIME=node "$NODE" "$VITEST" run --config vitest.config.ts --reporter=default --reporter=json --outputFile="$E/node-results.json"
run react-bun env CMINJECT_EXPECT_RUNTIME=bun INJECT_EXPECT_BUN=1 "$BUN" "$VITEST" run --config vitest.react.config.ts
run react-node env CMINJECT_EXPECT_RUNTIME=node "$NODE" "$VITEST" run --config vitest.react.config.ts
run compiler-fixtures env CMINJECT_EXPECT_RUNTIME=bun "$BUN" "$VITEST" run --typecheck.only --config vitest.types.config.ts
run coverage env CMINJECT_COVERAGE=1 CMINJECT_EXPECT_RUNTIME=node "$NODE" "$VITEST" run --config vitest.config.ts --coverage
run imports "$BUN" scripts/check-imports.ts --dist
run dag "$BUN" scripts/check-core-imports.ts
run browser "$BUN" scripts/check-browser.ts
run typedoc "$NODE" node_modules/typedoc/bin/typedoc --options typedoc.json
run compiler-real env BUN_BINARY="$BUN" "$NODE" ../../tools/ctsc/tests/real-inject.integration.mjs
run whitespace git diff --check -- . ../decorate ../../tools/ctsc ../../tools/split
