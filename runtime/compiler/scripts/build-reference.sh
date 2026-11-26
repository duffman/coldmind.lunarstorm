#!/bin/sh
# Build the vendored pinned engine directly; CMake and network are not needed.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
# lunarstorm patch: the reference is unmodified upstream QuickJS-NG with its C
# parser (vendor/quickjs-ng-upstream). jetbunny built it from the patched engine,
# whose parser it removed in 1588015, after which every reference run failed
# with "eval is not supported".
vendor="$root/vendor/quickjs-ng-upstream"
output="${QJS_REFERENCE_BUILD_DIR:-$root/build}"
revision=7b5f588e0f9b643e2996f291bfc861abeaceddc2
# The engine's REVISION file is a manifest; its "Revision:" line names the upstream commit.
if [ "$(sed -n 's/^Revision: //p' "$vendor/REVISION")" != "$revision" ]; then
    echo "error: unexpected QuickJS-NG revision" >&2
    exit 1
fi
mkdir -p "$output"
temporary=$(mktemp "$output/.qjs-reference-engine-bridge.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
# Keep compiler flags under caller control; the default is a normal release build.
# Intentional splitting for conventional CFLAGS/LDFLAGS values.
"${CC:-cc}" -std=c11 -D_GNU_SOURCE -DQUICKJS_NG_BUILD ${CFLAGS:--O2} -I"$vendor" \
    "$root/reference-engine-bridge/runner.c" \
    "$vendor/quickjs.c" "$vendor/dtoa.c" \
    "$vendor/libregexp.c" "$vendor/libunicode.c" \
    ${LDFLAGS:-} -lm -pthread -o "$temporary"
chmod +x "$temporary"
mv -f "$temporary" "$output/qjs-reference-engine-bridge"
printf '%s\n' "$output/qjs-reference-engine-bridge"
