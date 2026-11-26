#!/bin/sh
# Builds the `lunar` command line in release and installs it on PATH.
#
#   platform/cli/app/install.sh                       # features: runtime
#   LUNAR_FEATURES=runtime,build,frontend platform/cli/app/install.sh
#   LUNAR_BIN_DIR=/usr/local/bin platform/cli/app/install.sh
#
# The binary is copied to <repo>/_build/lunar and to $LUNAR_BIN_DIR (default
# ~/.local/bin). If that directory is not on PATH, a line adding it is appended
# to ~/.zshrc.
set -eu

root=$(cd "$(dirname "$0")/../../.." && pwd)
features=${LUNAR_FEATURES:-runtime}
bin_dir=${LUNAR_BIN_DIR:-$HOME/.local/bin}

cd "$root"
cargo build --release -p lunarstorm --bin lunar --no-default-features --features "$features"

target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
mkdir -p "$root/_build" "$bin_dir"
cp "$target/release/lunar" "$root/_build/lunar"
cp "$target/release/lunar" "$bin_dir/lunar"

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *)
    echo "export PATH=\"$bin_dir:\$PATH\"" >> "$HOME/.zshrc"
    echo "added $bin_dir to PATH in ~/.zshrc; open a new shell"
    ;;
esac

echo "installed $bin_dir/lunar ($("$bin_dir/lunar" --version))"
