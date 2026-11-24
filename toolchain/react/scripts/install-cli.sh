#!/usr/bin/env bash
# Build the `lunarstorm` CLI (frontend tools included) and symlink it onto PATH.
# Usage: scripts/install-cli.sh [--debug]
# Env:   BIN_DIR (default ~/.local/bin)
# The link points into cargo's target dir, so later `cargo build`s update the command in place.
set -euo pipefail

cd "$(dirname "$0")/.."

profile=release
flag=--release
if [[ "${1:-}" == "--debug" ]]; then
  profile=debug
  flag=
fi

bin_dir="${BIN_DIR:-$HOME/.local/bin}"

cargo build $flag -p lunarstorm --bin lunarstorm

target_dir="$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
binary="$target_dir/$profile/lunarstorm"
[[ -x "$binary" ]] || { echo "install-cli.sh: $binary not found" >&2; exit 1; }

mkdir -p "$bin_dir"
ln -sfn "$binary" "$bin_dir/lunarstorm"
echo "lunarstorm -> $binary"

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) echo "install-cli.sh: $bin_dir is not on PATH" >&2 ;;
esac
